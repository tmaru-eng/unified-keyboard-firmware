#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! S140 USB-only readiness probe.
//!
//! This diagnostic keeps the installed S140 and its CLOCK/POWER ownership,
//! proves USB power/HFCLK through SVC calls, and then probes the USBD READY
//! boundary with a finite raw register helper before constructing one minimal
//! HID device. It does not contain BLE scanning or bridge logic.

use core::{
    panic::PanicInfo,
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_executor::Spawner;
use embassy_futures::yield_now;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use nrf_softdevice::{RawError, SocEvent, Softdevice, raw};
#[cfg(not(feature = "s140-ready-probe-hardware"))]
use nrf_usbd::{UsbPeripheral, Usbd};
#[cfg(feature = "s140-ready-detected-order-hardware")]
use ukf_nrf52840_ble_usb::detected_order_probe::{
    DetectedOrderAction, DetectedOrderEvent, DetectedOrderProbe, DetectedOrderStage,
};
#[cfg(feature = "s140-usb-probe-errata-hardware")]
use ukf_nrf52840_ble_usb::usb_errata_preflight::{ErrataRegisters, preflight};
#[cfg(not(feature = "s140-usb-probe-errata-hardware"))]
use ukf_nrf52840_ble_usb::usb_preflight::preflight;
use ukf_nrf52840_ble_usb::{
    s140::adapter,
    uf2_reset::UF2_RESET_MAGIC,
    usb_preflight::{ProbeStage, ReadyOutcome, ReadyRegisters},
};
#[cfg(not(feature = "s140-ready-probe-hardware"))]
use usb_device::{
    UsbError,
    bus::UsbBusAllocator,
    device::{StringDescriptors, UsbDeviceBuilder, UsbDeviceState, UsbVidPid},
};
#[cfg(not(feature = "s140-ready-probe-hardware"))]
use usbd_hid::{
    descriptor::{KeyboardReport, SerializedDescriptor},
    hid_class::{
        HIDClass, HidClassSettings, HidCountryCode, HidProtocol, HidSubClass, ProtocolModeConfig,
    },
};

const POWER_TIMEOUT_POLLS: u32 = 100_000;
const USBD_TIMEOUT_POLLS: u32 = 100_000;
#[cfg(feature = "s140-ready-probe-hardware")]
const READY_SUCCESS_HOLD_POLLS: u32 = 50_000_000;

const USBREGSTATUS_VBUSDETECT: u32 = 1 << 0;
const USBREGSTATUS_OUTPUTRDY: u32 = 1 << 1;

const USBD_BASE: usize = 0x4002_7000;
const USBD_EVENTCAUSE_OFFSET: usize = 0x400;
const USBD_ENABLE_OFFSET: usize = 0x500;

const GPIO0_BASE: usize = 0x5000_0000;
const GPIO_OUTSET_OFFSET: usize = 0x508;
const GPIO_OUTCLR_OFFSET: usize = 0x50c;
const GPIO_DIRSET_OFFSET: usize = 0x518;
const BLUE_LED_MASK: u32 = 1 << 6;
const RED_LED_MASK: u32 = 1 << 26;
const LED_MASK: u32 = BLUE_LED_MASK | RED_LED_MASK;

#[cfg(not(feature = "s140-ready-probe-hardware"))]
const RELEASE_REPORT: [u8; 8] = [0; 8];

static SOC_EVENTS: Channel<CriticalSectionRawMutex, SocEvent, 4> = Channel::new();
static SOC_EVENT_OVERFLOWED: AtomicBool = AtomicBool::new(false);

/// Independent binding for this diagnostic binary's one USBD instance.
#[cfg(not(feature = "s140-ready-probe-hardware"))]
struct XiaoUsbd;

#[cfg(not(feature = "s140-ready-probe-hardware"))]
unsafe impl UsbPeripheral for XiaoUsbd {
    const REGISTERS: *const () = USBD_BASE as *const ();
}

/// LED electrical output for the active-low XIAO Sense user LEDs.
struct DiagnosticLeds;

impl DiagnosticLeds {
    fn init() -> Self {
        write_gpio(GPIO_OUTSET_OFFSET, LED_MASK);
        write_gpio(GPIO_DIRSET_OFFSET, LED_MASK);
        Self
    }

    fn show(&self, stage: ProbeStage) {
        let (blue_on, red_on) = match stage {
            ProbeStage::PowerReady => (true, false),
            ProbeStage::UsbdEnableIssued => (true, true),
            ProbeStage::ReadySeen => (false, false),
            ProbeStage::ReadyTimeout => (false, true),
            ProbeStage::Running => (true, false),
        };
        let on_mask =
            if blue_on { BLUE_LED_MASK } else { 0 } | if red_on { RED_LED_MASK } else { 0 };
        write_gpio(GPIO_OUTCLR_OFFSET, on_mask);
        write_gpio(GPIO_OUTSET_OFFSET, LED_MASK & !on_mask);
    }

    fn panic_blink(&self, phase: bool) {
        let on_mask = if phase { RED_LED_MASK } else { 0 };
        write_gpio(GPIO_OUTCLR_OFFSET, on_mask);
        write_gpio(GPIO_OUTSET_OFFSET, LED_MASK & !on_mask);
    }
}

fn write_gpio(offset: usize, mask: u32) {
    if mask == 0 {
        return;
    }

    // Safety: GPIO0 is fixed nRF52840 hardware; each address is aligned and
    // only the two dedicated XIAO LED bits are written.
    unsafe {
        core::ptr::write_volatile((GPIO0_BASE + offset) as *mut u32, mask);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    let leds = DiagnosticLeds::init();
    let mut phase = false;
    loop {
        leds.panic_blink(phase);
        phase = !phase;
        for _ in 0..8_000_000 {
            core::hint::spin_loop();
        }
    }
}

/// Raw USBD register view used only before `UsbBusAllocator::freeze`.
struct RawUsbdReady;

impl ReadyRegisters for RawUsbdReady {
    fn enable(&mut self) {
        // Safety: the probe owns this peripheral and no nrf-usbd instance has
        // been created yet. ENABLE is the fixed USBD control register.
        unsafe {
            core::ptr::write_volatile((USBD_BASE + USBD_ENABLE_OFFSET) as *mut u32, 1);
        }
    }

    fn eventcause(&mut self) -> u32 {
        // Safety: the same exclusive pre-builder boundary as `enable`; this
        // read does not clear the W1C READY cause.
        unsafe { core::ptr::read_volatile((USBD_BASE + USBD_EVENTCAUSE_OFFSET) as *const u32) }
    }
}

#[cfg(feature = "s140-usb-probe-errata-hardware")]
impl ErrataRegisters for RawUsbdReady {
    fn read_word(&mut self, address: usize) -> u32 {
        // Safety: the errata helper only supplies its three fixed S140
        // addresses; this read is before nrf-usbd owns the peripheral.
        unsafe { core::ptr::read_volatile(address as *const u32) }
    }

    fn write_word(&mut self, address: usize, value: u32) {
        // Safety: the errata helper only supplies its three fixed S140
        // addresses; no USB EVENTCAUSE or pull-up register is writable here.
        unsafe { core::ptr::write_volatile(address as *mut u32, value) }
    }
}

fn usb_soc_event(event: SocEvent) -> Option<SocEvent> {
    match event {
        SocEvent::PowerUsbDetected | SocEvent::PowerUsbPowerReady => Some(event),
        _ => None,
    }
}

#[embassy_executor::task]
async fn softdevice_task(sd: &'static Softdevice) -> ! {
    sd.run_with_callback(|event| {
        if usb_soc_event(event).is_some() && SOC_EVENTS.try_send(event).is_err() {
            SOC_EVENT_OVERFLOWED.store(true, Ordering::Release);
        }
    })
    .await
}

fn reset_into_uf2_bootloader() -> ! {
    // Safety: S140 owns POWER; GPREGRET is accessed through its SVC ABI.
    unsafe {
        RawError::convert(raw::sd_power_gpregret_clr(0, 0xff)).unwrap();
        RawError::convert(raw::sd_power_gpregret_set(0, u32::from(UF2_RESET_MAGIC))).unwrap();
    }
    cortex_m::peripheral::SCB::sys_reset()
}

#[cfg(not(feature = "s140-ready-detected-order-hardware"))]
async fn prepare_s140_usb(leds: &DiagnosticLeds) {
    let mut usbregstatus = 0;
    // Safety: all calls are S140 7.3 SVCs and use ABI-correct scalar/output
    // arguments while SoftDevice owns CLOCK and POWER.
    unsafe {
        RawError::convert(raw::sd_power_usbdetected_enable(1)).unwrap();
        RawError::convert(raw::sd_power_usbpwrrdy_enable(1)).unwrap();
        RawError::convert(raw::sd_power_usbremoved_enable(1)).unwrap();
        RawError::convert(raw::sd_power_usbregstatus_get(&mut usbregstatus)).unwrap();
        RawError::convert(raw::sd_clock_hfclk_request()).unwrap();
    }

    let mut detected = usbregstatus & USBREGSTATUS_VBUSDETECT != 0;
    let mut power_ready = usbregstatus & USBREGSTATUS_OUTPUTRDY != 0;
    for _ in 0..POWER_TIMEOUT_POLLS {
        while let Ok(event) = SOC_EVENTS.try_receive() {
            match event {
                SocEvent::PowerUsbDetected => detected = true,
                SocEvent::PowerUsbPowerReady => power_ready = true,
                _ => {}
            }
        }
        if detected && power_ready {
            break;
        }
        if SOC_EVENT_OVERFLOWED.load(Ordering::Acquire) {
            reset_into_uf2_bootloader();
        }
        yield_now().await;
    }
    if !(detected && power_ready) {
        reset_into_uf2_bootloader();
    }

    for _ in 0..POWER_TIMEOUT_POLLS {
        let mut running = 0;
        // Safety: SVC writes one u32 into this live stack variable.
        unsafe {
            RawError::convert(raw::sd_clock_hfclk_is_running(&mut running)).unwrap();
        }
        if running != 0 {
            leds.show(ProbeStage::PowerReady);
            return;
        }
        yield_now().await;
    }
    reset_into_uf2_bootloader();
}

#[cfg(feature = "s140-ready-detected-order-hardware")]
async fn prepare_detected_order(leds: &DiagnosticLeds) -> DetectedOrderProbe {
    let mut status = 0;
    // Safety: only S140 POWER/CLOCK SVCs are used before raw USBD probing.
    unsafe {
        RawError::convert(raw::sd_power_usbdetected_enable(1)).unwrap();
        RawError::convert(raw::sd_power_usbpwrrdy_enable(1)).unwrap();
        RawError::convert(raw::sd_power_usbremoved_enable(1)).unwrap();
        RawError::convert(raw::sd_power_usbregstatus_get(&mut status)).unwrap();
    }

    let mut probe = DetectedOrderProbe::new(
        POWER_TIMEOUT_POLLS,
        POWER_TIMEOUT_POLLS,
        USBD_TIMEOUT_POLLS,
        POWER_TIMEOUT_POLLS,
        READY_SUCCESS_HOLD_POLLS,
    );
    let mut action = DetectedOrderAction::Wait;
    if status & USBREGSTATUS_VBUSDETECT != 0 {
        action = probe.on_event(DetectedOrderEvent::Detected);
    }
    if status & USBREGSTATUS_OUTPUTRDY != 0 {
        probe.on_event(DetectedOrderEvent::PowerReady);
    }
    if action == DetectedOrderAction::RequestHfclk {
        // Safety: request HFCLK only after VBUS DETECTED, matching the A/B
        // ordering under test.
        unsafe { RawError::convert(raw::sd_clock_hfclk_request()).unwrap() };
    }

    loop {
        match probe.stage() {
            DetectedOrderStage::WaitingDetected => {
                while let Ok(event) = SOC_EVENTS.try_receive() {
                    let event = match event {
                        SocEvent::PowerUsbDetected => DetectedOrderEvent::Detected,
                        SocEvent::PowerUsbPowerReady => DetectedOrderEvent::PowerReady,
                        _ => continue,
                    };
                    action = probe.on_event(event);
                }
                if action == DetectedOrderAction::RequestHfclk {
                    unsafe { RawError::convert(raw::sd_clock_hfclk_request()).unwrap() };
                } else {
                    action = probe.poll_detected();
                }
                if action == DetectedOrderAction::ResetIntoUf2 {
                    leds.show(ProbeStage::ReadyTimeout);
                    reset_into_uf2_bootloader();
                }
            }
            DetectedOrderStage::WaitingHfclk => {
                let mut running = 0;
                unsafe {
                    RawError::convert(raw::sd_clock_hfclk_is_running(&mut running)).unwrap();
                }
                action = probe.poll_hfclk(running != 0);
                if action == DetectedOrderAction::BeginReady {
                    leds.show(ProbeStage::PowerReady);
                    return probe;
                }
                if action == DetectedOrderAction::ResetIntoUf2 {
                    leds.show(ProbeStage::ReadyTimeout);
                    reset_into_uf2_bootloader();
                }
            }
            _ => return probe,
        }
        if SOC_EVENT_OVERFLOWED.load(Ordering::Acquire) {
            leds.show(ProbeStage::ReadyTimeout);
            reset_into_uf2_bootloader();
        }
        yield_now().await;
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let leds = DiagnosticLeds::init();
    let sd = Softdevice::enable(&adapter::softdevice_config());
    spawner.spawn(softdevice_task(sd).unwrap());
    #[cfg(feature = "s140-ready-detected-order-hardware")]
    let mut detected_order = prepare_detected_order(&leds).await;
    #[cfg(not(feature = "s140-ready-detected-order-hardware"))]
    prepare_s140_usb(&leds).await;

    leds.show(ProbeStage::UsbdEnableIssued);
    let mut registers = RawUsbdReady;
    match preflight(&mut registers, USBD_TIMEOUT_POLLS) {
        ReadyOutcome::Ready { .. } => leds.show(ProbeStage::ReadySeen),
        ReadyOutcome::TimedOut { .. } => {
            leds.show(ProbeStage::ReadyTimeout);
            reset_into_uf2_bootloader();
        }
    }

    #[cfg(feature = "s140-ready-detected-order-hardware")]
    {
        let mut action = detected_order.on_ready();
        if action == DetectedOrderAction::AwaitPowerReady {
            loop {
                while let Ok(event) = SOC_EVENTS.try_receive() {
                    if event == SocEvent::PowerUsbPowerReady {
                        action = detected_order.on_event(DetectedOrderEvent::PowerReady);
                    }
                }
                if action == DetectedOrderAction::HoldSuccess {
                    break;
                }
                action = detected_order.poll_power_ready();
                if action == DetectedOrderAction::ResetIntoUf2 {
                    leds.show(ProbeStage::ReadyTimeout);
                    reset_into_uf2_bootloader();
                }
                yield_now().await;
            }
        }
        leds.show(ProbeStage::Running);
        while detected_order.poll_success_hold() == DetectedOrderAction::Wait {
            yield_now().await;
        }
        reset_into_uf2_bootloader();
    }

    #[cfg(all(
        feature = "s140-ready-probe-hardware",
        not(feature = "s140-ready-detected-order-hardware")
    ))]
    {
        // READY-only A/B marker: no UsbDeviceBuilder, pull-up, or READY clear
        // is executed. Hold the success LED for a much longer finite window so
        // the host can distinguish READY from an immediate timeout reset.
        leds.show(ProbeStage::Running);
        for _ in 0..READY_SUCCESS_HOLD_POLLS {
            yield_now().await;
        }
        reset_into_uf2_bootloader();
    }

    #[cfg(not(feature = "s140-ready-probe-hardware"))]
    {
        // The successful preflight deliberately leaves READY and pull-up
        // alone; nrf-usbd/usb-device now owns builder-time errata, READY W1C,
        // and pull-up.
        let usb_bus = UsbBusAllocator::new(Usbd::new(XiaoUsbd));
        let settings = HidClassSettings {
            subclass: HidSubClass::Boot,
            protocol: HidProtocol::Keyboard,
            config: ProtocolModeConfig::ForceBoot,
            locale: HidCountryCode::NotSupported,
        };
        let mut hid =
            HIDClass::new_ep_in_with_settings(&usb_bus, KeyboardReport::desc(), 8, settings);
        let strings = [StringDescriptors::default()
            .manufacturer("Unified Keyboard Firmware")
            .product("XIAO S140 USB readiness probe")
            .serial_number("S140-USB-PROBE")];
        let mut usb = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x1209, 0x0002))
            .strings(&strings)
            .unwrap()
            .max_packet_size_0(64)
            .unwrap()
            .build();
        leds.show(ProbeStage::Running);

        let mut release_sent = false;
        loop {
            usb.poll(&mut [&mut hid]);
            if usb.state() == UsbDeviceState::Configured {
                if !release_sent {
                    match hid.push_raw_input(&RELEASE_REPORT) {
                        Ok(_) => release_sent = true,
                        Err(UsbError::WouldBlock) | Err(UsbError::InvalidState) => {}
                        Err(_) => release_sent = false,
                    }
                }
            } else {
                release_sent = false;
            }
            yield_now().await;
        }
    }
}
