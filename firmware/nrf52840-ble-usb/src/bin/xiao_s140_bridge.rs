#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! Seeed XIAO nRF52840 Sense bridge preserving the factory S140 7.3.0.

use core::{
    panic::PanicInfo,
    slice,
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_executor::Spawner;
use embassy_futures::{join::join, yield_now};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use nrf_softdevice::{
    RawError, SocEvent, Softdevice,
    ble::{Address, Connection, central, gatt_client},
    raw,
};
use nrf_usbd::{UsbPeripheral, Usbd, dma_timed_out};
use ukf_core::BridgeProfile;
use ukf_nrf52840_ble_usb::{
    ReportPipeline, UsbReportSink,
    config_hid::CONFIG_REPORT_DESCRIPTOR,
    detected_order_probe::{
        DetectedOrderAction, DetectedOrderEvent, DetectedOrderProbe, DetectedOrderStage,
    },
    diagnostics::{StartupStage as LedStartupStage, led_state},
    hogp::HogpCentral,
    panic_recovery::{PanicRecovery, PanicRecoveryAction},
    s140::{adapter, advertisement_has_hid_service},
    startup::{RecoveryAction, StartupGate, StartupPoll, StartupStage},
    uf2_reset::{UF2_RESET_MAGIC, normalize_reset_report, parse_reset_report},
    usb_enable_boundary::{DriverAction, DriverLifecycle},
    usb_power::{
        UsbPowerAction, UsbPowerEvent, UsbPowerMachine, UsbPowerRecovery, UsbRuntimeFault,
        recovery_action, runtime_fault_recovery,
    },
};
use usb_device::{
    UsbError,
    bus::{PollResult, UsbBus, UsbBusAllocator},
    device::{StringDescriptors, UsbDevice, UsbDeviceBuilder, UsbVidPid},
    endpoint::{EndpointAddress, EndpointType},
};
use usbd_hid::{
    descriptor::{KeyboardReport, SerializedDescriptor},
    hid_class::{
        HIDClass, HidClassSettings, HidCountryCode, HidProtocol, HidSubClass, ProtocolModeConfig,
        ReportType,
    },
};

const REPORT_QUEUE_DEPTH: usize = 16;
const SOC_EVENT_QUEUE_DEPTH: usize = 8;

// `usbd-hid` accepts control SET_REPORT payloads up to the USB control buffer
// size. Matching that size lets the bridge consume and reject oversized input
// instead of leaving it pending indefinitely.
const CONFIG_CONTROL_BUFFER_LEN: usize = 128;

const USBREGSTATUS_VBUSDETECT: u32 = 1 << 0;
const USBREGSTATUS_OUTPUTRDY: u32 = 1 << 1;

// These budgets are deliberately finite scheduler/spin polls rather than
// wall-clock promises: this binary does not own a timer before USB starts.
const STARTUP_TIMEOUT_POLLS: u32 = 100_000;

const GPIO0_BASE: usize = 0x5000_0000;
const GPIO_OUTSET_OFFSET: usize = 0x508;
const GPIO_OUTCLR_OFFSET: usize = 0x50c;
const GPIO_DIRSET_OFFSET: usize = 0x518;
const BLUE_LED_MASK: u32 = 1 << 6;
const RED_LED_MASK: u32 = 1 << 26;
const DIAGNOSTIC_LED_MASK: u32 = BLUE_LED_MASK | RED_LED_MASK;

/// Minimal, independent binding for the XIAO Sense's active-low user LEDs.
///
/// These writes intentionally stay outside the BLE and USB driver ownership
/// boundaries: P0.06 and P0.26 are dedicated board LEDs, not transport pins.
struct DiagnosticLeds;

impl DiagnosticLeds {
    fn init() -> Self {
        // Start with both active-low LEDs off before changing their direction,
        // avoiding an ambiguous flash during GPIO setup.
        write_gpio(GPIO_OUTSET_OFFSET, DIAGNOSTIC_LED_MASK);
        write_gpio(GPIO_DIRSET_OFFSET, DIAGNOSTIC_LED_MASK);
        Self
    }

    fn show(&self, stage: LedStartupStage, blink_phase: bool) {
        let state = led_state(stage, blink_phase);
        let on_mask = if state.blue_on() { BLUE_LED_MASK } else { 0 }
            | if state.red_on() { RED_LED_MASK } else { 0 };
        let off_mask = DIAGNOSTIC_LED_MASK & !on_mask;

        write_gpio(GPIO_OUTCLR_OFFSET, on_mask);
        write_gpio(GPIO_OUTSET_OFFSET, off_mask);
    }
}

fn write_gpio(offset: usize, mask: u32) {
    if mask == 0 {
        return;
    }

    // Safety: GPIO0 is the fixed nRF52840 register block. Each pointer targets
    // a write-only task register, is naturally aligned, and receives only the
    // two XIAO Sense LED bits owned by this diagnostic boundary.
    unsafe {
        core::ptr::write_volatile((GPIO0_BASE + offset) as *mut u32, mask);
    }
}

/// Marker used while the synchronous nrf-usbd enable boundary is active.
fn show_driver_marker(on_mask: u32) {
    write_gpio(GPIO_OUTCLR_OFFSET, on_mask);
    write_gpio(GPIO_OUTSET_OFFSET, DIAGNOSTIC_LED_MASK & !on_mask);
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    let leds = DiagnosticLeds::init();
    let mut recovery = PanicRecovery::new();
    let mut blink_phase = false;

    loop {
        match recovery.next() {
            PanicRecoveryAction::Blink => {
                leds.show(LedStartupStage::Panicked, blink_phase);
                blink_phase = !blink_phase;
                for _ in 0..16_000_000 {
                    core::hint::spin_loop();
                }
            }
            PanicRecoveryAction::ResetIntoUf2Bootloader => {
                reset_into_uf2_bootloader();
            }
        }
    }
}

enum BridgeEvent {
    Connected(HogpCentral, Connection),
    Notification(adapter::Notification),
    Disconnected,
}

static BRIDGE_EVENTS: Channel<CriticalSectionRawMutex, BridgeEvent, REPORT_QUEUE_DEPTH> =
    Channel::new();
static REPORT_QUEUE_OVERFLOWED: AtomicBool = AtomicBool::new(false);
static SOC_EVENTS: Channel<CriticalSectionRawMutex, SocEvent, SOC_EVENT_QUEUE_DEPTH> =
    Channel::new();
static SOC_EVENT_QUEUE_OVERFLOWED: AtomicBool = AtomicBool::new(false);
static USBD_ENABLE_TIMED_OUT: AtomicBool = AtomicBool::new(false);
static USBD_DRIVER_ENABLE_ENTERED: AtomicBool = AtomicBool::new(false);
static USBD_DRIVER_ENABLE_RETURNED: AtomicBool = AtomicBool::new(false);

/// Exclusive application binding for the nRF52840 USBD register block.
///
/// Safety: `0x4002_7000` is the fixed nRF52840 USBD base address. This binary
/// creates exactly one `XiaoUsbd`, never aliases it through a PAC, and S140 does
/// not reserve the USBD peripheral. The wrapper contains no state and is `Send`.
struct XiaoUsbd;

unsafe impl UsbPeripheral for XiaoUsbd {
    const REGISTERS: *const () = 0x4002_7000 as *const ();
}

/// Usb-device bus adapter around the driver's two-phase bounded lifecycle;
/// failures are routed to UF2 recovery by the application.
struct BoundedUsbd {
    inner: Usbd<XiaoUsbd>,
    lifecycle: DriverLifecycle,
}

impl BoundedUsbd {
    fn new() -> Self {
        Self {
            inner: Usbd::new(XiaoUsbd),
            lifecycle: DriverLifecycle::new(),
        }
    }

    fn prepare(&mut self) -> bool {
        USBD_DRIVER_ENABLE_ENTERED.store(true, Ordering::Release);
        // Both LEDs steady: entered the pre-PWRRDY ENABLE/READY boundary.
        show_driver_marker(DIAGNOSTIC_LED_MASK);
        match self.lifecycle.prepare(|| self.inner.prepare_enable()) {
            DriverAction::Prepared => true,
            DriverAction::TimedOut | DriverAction::Rejected | DriverAction::Started => {
                USBD_ENABLE_TIMED_OUT.store(true, Ordering::Release);
                show_driver_marker(RED_LED_MASK);
                false
            }
        }
    }
}

/// The driver owns the two finite ENABLE/READY then pull-up transactions.
///
/// ENABLE/READY follows VBUS/HFCLK; the pull-up is connected only after the
/// startup state machine has observed USBPWRDY.
impl UsbBus for BoundedUsbd {
    fn alloc_ep(
        &mut self,
        ep_dir: usb_device::UsbDirection,
        ep_addr: Option<EndpointAddress>,
        ep_type: EndpointType,
        max_packet_size: u16,
        interval: u8,
    ) -> usb_device::Result<EndpointAddress> {
        self.inner
            .alloc_ep(ep_dir, ep_addr, ep_type, max_packet_size, interval)
    }

    fn enable(&mut self) {
        let outcome = self.lifecycle.start(|| self.inner.start_pullup());
        match outcome {
            DriverAction::TimedOut | DriverAction::Rejected | DriverAction::Prepared => {
                USBD_ENABLE_TIMED_OUT.store(true, Ordering::Release);
                show_driver_marker(RED_LED_MASK);
            }
            DriverAction::Started => {
                USBD_DRIVER_ENABLE_RETURNED.store(true, Ordering::Release);
                show_driver_marker(BLUE_LED_MASK);
            }
        }
    }

    fn reset(&self) {
        self.inner.reset();
    }

    fn set_device_address(&self, addr: u8) {
        self.inner.set_device_address(addr);
    }

    fn write(&self, ep_addr: EndpointAddress, buf: &[u8]) -> usb_device::Result<usize> {
        self.inner.write(ep_addr, buf)
    }

    fn read(&self, ep_addr: EndpointAddress, buf: &mut [u8]) -> usb_device::Result<usize> {
        self.inner.read(ep_addr, buf)
    }

    fn set_stalled(&self, ep_addr: EndpointAddress, stalled: bool) {
        self.inner.set_stalled(ep_addr, stalled);
    }

    fn is_stalled(&self, ep_addr: EndpointAddress) -> bool {
        self.inner.is_stalled(ep_addr)
    }

    fn suspend(&self) {
        self.inner.suspend();
    }

    fn resume(&self) {
        self.inner.resume();
    }

    fn poll(&self) -> PollResult {
        self.inner.poll()
    }
}

struct PendingUsbReport<'a>(&'a mut Option<[u8; 8]>);

impl UsbReportSink for PendingUsbReport<'_> {
    type Error = core::convert::Infallible;

    fn write(&mut self, report: [u8; 8]) -> Result<(), Self::Error> {
        *self.0 = Some(report);
        Ok(())
    }
}

fn usb_power_event(event: SocEvent) -> Option<UsbPowerEvent> {
    match event {
        SocEvent::PowerUsbDetected => Some(UsbPowerEvent::Detected),
        SocEvent::PowerUsbPowerReady => Some(UsbPowerEvent::PowerReady),
        SocEvent::PowerUsbRemoved => Some(UsbPowerEvent::Removed),
        _ => None,
    }
}

fn apply_usb_power_event(power: &mut UsbPowerMachine, event: UsbPowerEvent) -> UsbPowerRecovery {
    recovery_action(power.on_event(event))
}

fn reset_into_uf2_bootloader() -> ! {
    // Safety: S140 owns POWER, so GPREGRET must be changed through its SVC ABI.
    // Both calls use Nordic-defined scalar arguments and complete before the
    // Cortex-M system reset request is issued.
    unsafe {
        RawError::convert(raw::sd_power_gpregret_clr(0, 0xff)).unwrap();
        RawError::convert(raw::sd_power_gpregret_set(0, u32::from(UF2_RESET_MAGIC))).unwrap();
    }
    cortex_m::peripheral::SCB::sys_reset()
}

async fn prepare_softdevice_usb(startup: &mut StartupGate) -> DetectedOrderProbe {
    let mut usbregstatus = 0;

    // Safety: these are the S140 SVC entry points required after the
    // SoftDevice has claimed CLOCK and POWER. Each call uses the exact scalar
    // or valid writable-pointer ABI declared by Nordic's S140 7.3 bindings.
    unsafe {
        RawError::convert(raw::sd_power_usbdetected_enable(1)).unwrap();
        RawError::convert(raw::sd_power_usbpwrrdy_enable(1)).unwrap();
        RawError::convert(raw::sd_power_usbremoved_enable(1)).unwrap();
        RawError::convert(raw::sd_power_usbregstatus_get(&mut usbregstatus)).unwrap();
    }

    let mut order = DetectedOrderProbe::new(
        STARTUP_TIMEOUT_POLLS,
        STARTUP_TIMEOUT_POLLS,
        STARTUP_TIMEOUT_POLLS,
        STARTUP_TIMEOUT_POLLS,
        STARTUP_TIMEOUT_POLLS,
    );
    let mut action = DetectedOrderAction::Wait;
    if usbregstatus & USBREGSTATUS_VBUSDETECT != 0 {
        action = order.on_event(DetectedOrderEvent::Detected);
    }
    if usbregstatus & USBREGSTATUS_OUTPUTRDY != 0 {
        order.on_event(DetectedOrderEvent::PowerReady);
    }

    startup
        .transition(StartupStage::UsbPowerReady, STARTUP_TIMEOUT_POLLS)
        .unwrap();
    if action == DetectedOrderAction::RequestHfclk {
        // Snapshot already proves VBUS DETECTED; request HFCLK before the
        // first loop can enter the WaitingHfclk stage.
        unsafe { RawError::convert(raw::sd_clock_hfclk_request()).unwrap() };
        startup.rearm(STARTUP_TIMEOUT_POLLS);
    }

    loop {
        match order.stage() {
            DetectedOrderStage::WaitingDetected => {
                while let Ok(event) = SOC_EVENTS.try_receive() {
                    match event {
                        SocEvent::PowerUsbDetected => {
                            action = order.on_event(DetectedOrderEvent::Detected)
                        }
                        SocEvent::PowerUsbPowerReady => {
                            order.on_event(DetectedOrderEvent::PowerReady);
                        }
                        _ => {}
                    }
                }
                if action == DetectedOrderAction::RequestHfclk {
                    // VBUS DETECTED is the only prerequisite for this request
                    // in the ordering A/B; PWRRDY remains a later gate.
                    unsafe { RawError::convert(raw::sd_clock_hfclk_request()).unwrap() };
                    startup.rearm(STARTUP_TIMEOUT_POLLS);
                } else if let StartupPoll::TimedOut(RecoveryAction::ResetIntoUf2Bootloader) =
                    startup.poll()
                {
                    reset_into_uf2_bootloader();
                }
            }
            DetectedOrderStage::WaitingHfclk => {
                let mut hfclk_running = 0;
                // Safety: the SVC synchronously writes one `u32` to the
                // provided stack-backed pointer.
                unsafe {
                    RawError::convert(raw::sd_clock_hfclk_is_running(&mut hfclk_running)).unwrap();
                }
                action = order.poll_hfclk(hfclk_running != 0);
                if action == DetectedOrderAction::BeginReady {
                    return order;
                }
                if action == DetectedOrderAction::ResetIntoUf2 {
                    reset_into_uf2_bootloader();
                }
                if let StartupPoll::TimedOut(RecoveryAction::ResetIntoUf2Bootloader) =
                    startup.poll()
                {
                    reset_into_uf2_bootloader();
                }
            }
            _ => return order,
        }
        if SOC_EVENT_QUEUE_OVERFLOWED.load(Ordering::Acquire) {
            reset_into_uf2_bootloader();
        }
        yield_now().await;
    }
}

async fn ble_central_task(sd: &'static Softdevice) -> ! {
    loop {
        let mut hogp = HogpCentral::new();
        let _ = hogp.start_scan();

        let address = match central::scan(sd, &adapter::scan_config(), |report| {
            if report.data.len == 0 {
                return None;
            }
            // Safety: S140 owns this advertisement buffer for the callback's
            // duration and supplies its exact byte length.
            let payload =
                unsafe { slice::from_raw_parts(report.data.p_data, usize::from(report.data.len)) };
            advertisement_has_hid_service(payload).then(|| Address::from_raw(report.peer_addr))
        })
        .await
        {
            Ok(address) => address,
            Err(_) => continue,
        };

        let address_ref = &address;
        let addresses = [address_ref];
        let connection = match central::connect(sd, &adapter::connect_config(&addresses)).await {
            Ok(connection) => connection,
            Err(_) => continue,
        };

        let mut report_map = [0; 512];
        let (client, _) =
            match adapter::discover_and_subscribe(&mut hogp, &connection, &mut report_map).await {
                Ok(discovery) => discovery,
                Err(_) => {
                    let _ = connection.disconnect();
                    continue;
                }
            };

        BRIDGE_EVENTS
            .send(BridgeEvent::Connected(hogp, connection.clone()))
            .await;
        gatt_client::run(&connection, &client, |notification| {
            if BRIDGE_EVENTS
                .try_send(BridgeEvent::Notification(notification))
                .is_err()
            {
                // Losing a release report can leave a host key held. Terminate
                // the link so the consumer emits a deterministic release.
                REPORT_QUEUE_OVERFLOWED.store(true, Ordering::Release);
                let _ = connection.disconnect();
            }
        })
        .await;
        BRIDGE_EVENTS.send(BridgeEvent::Disconnected).await;
    }
}

#[embassy_executor::task]
async fn softdevice_task(sd: &'static Softdevice) -> ! {
    sd.run_with_callback(|event| {
        if usb_power_event(event).is_some() && SOC_EVENTS.try_send(event).is_err() {
            SOC_EVENT_QUEUE_OVERFLOWED.store(true, Ordering::Release);
        }
    })
    .await
}

async fn usb_bridge_task<'a, B: usb_device::bus::UsbBus>(
    usb: &mut UsbDevice<'a, B>,
    keyboard_hid: &mut HIDClass<'a, B>,
    config_hid: &mut HIDClass<'a, B>,
    diagnostic_leds: &DiagnosticLeds,
    mut usb_power: UsbPowerMachine,
) -> ! {
    let mut pipeline = ReportPipeline::new(BridgeProfile::US_JIS_PRESET).unwrap();
    let mut central = None;
    let mut pending = Some([0; 8]);
    let mut usb_poll_returned = false;

    loop {
        if SOC_EVENT_QUEUE_OVERFLOWED.load(Ordering::Acquire) {
            // The queue overflow means at least one USB power transition was
            // lost. The active nrf-usbd instance cannot be rebuilt safely, so
            // use the same bounded UF2 recovery policy as a DMA timeout or
            // active VBUS removal instead of entering the panic handler.
            if matches!(
                runtime_fault_recovery(UsbRuntimeFault::SocEventQueueOverflow),
                UsbPowerRecovery::ResetIntoUf2Bootloader
            ) {
                reset_into_uf2_bootloader();
            }
        }
        while let Ok(event) = SOC_EVENTS.try_receive() {
            if let Some(event) = usb_power_event(event)
                && apply_usb_power_event(&mut usb_power, event)
                    == UsbPowerRecovery::ResetIntoUf2Bootloader
            {
                // Removal and reattachment cannot safely rebuild the current
                // nrf-usbd instance. Return to UF2 instead of entering the
                // red-blink panic handler.
                reset_into_uf2_bootloader();
            }
        }

        if REPORT_QUEUE_OVERFLOWED.swap(false, Ordering::AcqRel) {
            while BRIDGE_EVENTS.try_receive().is_ok() {}
            central = None;
            let mut sink = PendingUsbReport(&mut pending);
            let _ = pipeline.disconnect(&mut sink);
            pipeline = ReportPipeline::new(BridgeProfile::US_JIS_PRESET).unwrap();
        }

        usb.poll(&mut [keyboard_hid, config_hid]);
        if dma_timed_out() {
            // The vendored driver has converted a missing ENDEPIN/ENDEPOUT
            // event into a finite WouldBlock result. Do not leave the host
            // attached to a half-serviced EP0 transfer; recover through UF2.
            diagnostic_leds.show(LedStartupStage::Panicked, false);
            reset_into_uf2_bootloader();
        }
        if !usb_poll_returned {
            // `UsbDeviceBuilder::build` has already enabled the nrf-usbd bus;
            // keep the pre-poll boundary visible until the first poll returns,
            // so a stalled transfer path is distinguishable from a running
            // bridge.
            diagnostic_leds.show(LedStartupStage::UsbReady, false);
            usb_poll_returned = true;
            yield_now().await;
            diagnostic_leds.show(LedStartupStage::BridgeRunning, false);
        }

        let mut config_report = [0_u8; CONFIG_CONTROL_BUFFER_LEN];
        if let Ok(info) = config_hid.pull_raw_report(&mut config_report)
            && info.report_type == ReportType::Feature
            && normalize_reset_report(info.report_id, &config_report[..info.len])
                .and_then(|payload| parse_reset_report(info.report_id, payload))
                .is_ok()
        {
            reset_into_uf2_bootloader();
        }

        if let Some(report) = pending {
            match keyboard_hid.push_raw_input(&report) {
                Ok(_) => pending = None,
                Err(UsbError::WouldBlock) | Err(UsbError::InvalidState) => {}
                Err(_) => pending = Some([0; 8]),
            }
        }

        if pending.is_none()
            && let Ok(event) = BRIDGE_EVENTS.try_receive()
        {
            match event {
                BridgeEvent::Connected(hogp, connection) => {
                    central = Some((hogp, connection));
                }
                BridgeEvent::Notification(notification) => {
                    let rejected = if let Some((hogp, _)) = central.as_ref() {
                        let mut sink = PendingUsbReport(&mut pending);
                        notification
                            .forward(hogp, &mut pipeline, &mut sink)
                            .is_err()
                    } else {
                        false
                    };
                    if rejected {
                        if let Some((_, connection)) = central.take() {
                            let _ = connection.disconnect();
                        }
                        let mut sink = PendingUsbReport(&mut pending);
                        let _ = pipeline.disconnect(&mut sink);
                        pipeline = ReportPipeline::new(BridgeProfile::US_JIS_PRESET).unwrap();
                    }
                }
                BridgeEvent::Disconnected => {
                    central = None;
                    let mut sink = PendingUsbReport(&mut pending);
                    let _ = pipeline.disconnect(&mut sink);
                    pipeline = ReportPipeline::new(BridgeProfile::US_JIS_PRESET).unwrap();
                }
            }
        }

        yield_now().await;
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let diagnostic_leds = DiagnosticLeds::init();
    diagnostic_leds.show(LedStartupStage::Booting, false);
    let mut startup = StartupGate::new(StartupStage::Booting, STARTUP_TIMEOUT_POLLS);

    let sd = Softdevice::enable(&adapter::softdevice_config());
    spawner.spawn(softdevice_task(sd).unwrap());
    startup
        .transition(StartupStage::SoftdeviceReady, STARTUP_TIMEOUT_POLLS)
        .unwrap();
    diagnostic_leds.show(LedStartupStage::SoftdeviceReady, false);
    let mut detected_order = prepare_softdevice_usb(&mut startup).await;

    // Match the legacy board-driver order: VBUS DETECTED, HFCLK, then the
    // driver's single ENABLE/READY prepare phase. USBPWRDY is awaited before
    // the allocator freeze calls start_pullup.
    let mut bounded_usbd = BoundedUsbd::new();
    if !bounded_usbd.prepare() {
        reset_into_uf2_bootloader();
    }
    let mut power_ready_action = detected_order.on_ready();
    if power_ready_action == DetectedOrderAction::AwaitPowerReady {
        startup.rearm(STARTUP_TIMEOUT_POLLS);
        loop {
            while let Ok(event) = SOC_EVENTS.try_receive() {
                match event {
                    SocEvent::PowerUsbPowerReady => {
                        power_ready_action =
                            detected_order.on_event(DetectedOrderEvent::PowerReady);
                    }
                    SocEvent::PowerUsbRemoved => reset_into_uf2_bootloader(),
                    _ => {}
                }
            }
            if power_ready_action == DetectedOrderAction::HoldSuccess {
                break;
            }
            if let StartupPoll::TimedOut(RecoveryAction::ResetIntoUf2Bootloader) = startup.poll() {
                reset_into_uf2_bootloader();
            }
            if SOC_EVENT_QUEUE_OVERFLOWED.load(Ordering::Acquire) {
                reset_into_uf2_bootloader();
            }
            yield_now().await;
        }
    }
    if power_ready_action != DetectedOrderAction::HoldSuccess {
        reset_into_uf2_bootloader();
    }

    let mut usb_power = UsbPowerMachine::new();
    if usb_power.on_event(UsbPowerEvent::Detected) != UsbPowerAction::None
        || usb_power.on_event(UsbPowerEvent::PowerReady) != UsbPowerAction::StartUsbd
    {
        reset_into_uf2_bootloader();
    }
    startup
        .transition(StartupStage::UsbdReady, STARTUP_TIMEOUT_POLLS)
        .unwrap();

    diagnostic_leds.show(LedStartupStage::UsbReady, false);
    // The adapter's UsbBus::enable is now only the PWRRDY-gated start phase;
    // the USBD ENABLE/READY transaction already happened before this wait.
    let usb_bus = UsbBusAllocator::new(bounded_usbd);
    let settings = HidClassSettings {
        subclass: HidSubClass::Boot,
        protocol: HidProtocol::Keyboard,
        config: ProtocolModeConfig::ForceBoot,
        locale: HidCountryCode::NotSupported,
    };
    let mut keyboard_hid =
        HIDClass::new_ep_in_with_settings(&usb_bus, KeyboardReport::desc(), 8, settings);
    let mut config_hid = HIDClass::new_ep_in(&usb_bus, CONFIG_REPORT_DESCRIPTOR, 255);
    let strings = [StringDescriptors::default()
        .manufacturer("Unified Keyboard Firmware")
        .product("XIAO S140 BLE bridge")
        .serial_number("S140-PROTOTYPE")];
    let mut usb = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x1209, 0x0001))
        .strings(&strings)
        .unwrap()
        .max_packet_size_0(64)
        .unwrap()
        .build();
    if USBD_ENABLE_TIMED_OUT.load(Ordering::Acquire) {
        reset_into_uf2_bootloader();
    }
    diagnostic_leds.show(LedStartupStage::UsbReady, false);
    let _ = usb_power.on_event(UsbPowerEvent::UsbdStarted);
    startup
        .transition(StartupStage::Running, STARTUP_TIMEOUT_POLLS)
        .unwrap();

    join(
        ble_central_task(sd),
        usb_bridge_task(
            &mut usb,
            &mut keyboard_hid,
            &mut config_hid,
            &diagnostic_leds,
            usb_power,
        ),
    )
    .await;
}
