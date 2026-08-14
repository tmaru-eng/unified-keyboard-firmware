#![no_std]
#![no_main]
#![deny(unsafe_op_in_unsafe_fn)]

//! S140 USB POWER/HFCLK-only probe.
//!
//! This binary intentionally never touches the USBD register block and never
//! constructs a USB bus. It proves only that the SoftDevice accepts USB power
//! event enables, reports a usable USBREGSTATUS snapshot, and can start HFCLK.

use core::{
    panic::PanicInfo,
    sync::atomic::{AtomicBool, Ordering},
};

use embassy_executor::Spawner;
use embassy_futures::yield_now;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use nrf_softdevice::{RawError, SocEvent, Softdevice, raw};
use ukf_nrf52840_ble_usb::{
    power_probe::{PowerEvent, PowerProbe, PowerProbeAction, PowerProbeStage},
    s140::adapter,
    uf2_reset::UF2_RESET_MAGIC,
};

const POWER_TIMEOUT_POLLS: u32 = 100_000;
const HFCLK_TIMEOUT_POLLS: u32 = 100_000;
// Long enough for the host monitor to observe application detach before the
// deliberate UF2 reset. This remains a finite budget, not an infinite wait.
const SUCCESS_HOLD_POLLS: u32 = 50_000_000;

const GPIO0_BASE: usize = 0x5000_0000;
const GPIO_OUTSET_OFFSET: usize = 0x508;
const GPIO_OUTCLR_OFFSET: usize = 0x50c;
const GPIO_DIRSET_OFFSET: usize = 0x518;
const BLUE_LED_MASK: u32 = 1 << 6;
const RED_LED_MASK: u32 = 1 << 26;
const LED_MASK: u32 = BLUE_LED_MASK | RED_LED_MASK;

static SOC_EVENTS: Channel<CriticalSectionRawMutex, SocEvent, 4> = Channel::new();
static SOC_EVENT_OVERFLOWED: AtomicBool = AtomicBool::new(false);

struct DiagnosticLeds;

#[derive(Clone, Copy)]
enum LedStage {
    PowerReady,
    HfclkReady,
    Timeout,
}

impl DiagnosticLeds {
    fn init() -> Self {
        write_gpio(GPIO_OUTSET_OFFSET, LED_MASK);
        write_gpio(GPIO_DIRSET_OFFSET, LED_MASK);
        Self
    }

    fn show(&self, stage: LedStage) {
        let on_mask = match stage {
            LedStage::PowerReady => BLUE_LED_MASK,
            LedStage::HfclkReady => LED_MASK,
            LedStage::Timeout => RED_LED_MASK,
        };
        write_gpio(GPIO_OUTCLR_OFFSET, on_mask);
        write_gpio(GPIO_OUTSET_OFFSET, LED_MASK & !on_mask);
    }
}

fn write_gpio(offset: usize, mask: u32) {
    if mask == 0 {
        return;
    }
    // Safety: GPIO0 is fixed nRF52840 hardware; only the two dedicated XIAO
    // LED bits are written and all register addresses are aligned.
    unsafe {
        core::ptr::write_volatile((GPIO0_BASE + offset) as *mut u32, mask);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    let leds = DiagnosticLeds::init();
    loop {
        leds.show(LedStage::Timeout);
        for _ in 0..8_000_000 {
            core::hint::spin_loop();
        }
        leds.show(LedStage::HfclkReady);
        for _ in 0..8_000_000 {
            core::hint::spin_loop();
        }
    }
}

fn usb_soc_event(event: SocEvent) -> Option<PowerEvent> {
    match event {
        SocEvent::PowerUsbDetected => Some(PowerEvent::Detected),
        SocEvent::PowerUsbPowerReady => Some(PowerEvent::PowerReady),
        _ => None,
    }
}

#[embassy_executor::task]
async fn softdevice_task(sd: &'static Softdevice) -> ! {
    sd.run_with_callback(|event| {
        if let Some(event) = usb_soc_event(event) {
            let event = match event {
                PowerEvent::Detected => SocEvent::PowerUsbDetected,
                PowerEvent::PowerReady => SocEvent::PowerUsbPowerReady,
            };
            if SOC_EVENTS.try_send(event).is_err() {
                SOC_EVENT_OVERFLOWED.store(true, Ordering::Release);
            }
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

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let leds = DiagnosticLeds::init();
    let sd = Softdevice::enable(&adapter::softdevice_config());
    spawner.spawn(softdevice_task(sd).unwrap());

    let mut status = 0;
    // Safety: all calls are S140 7.3 POWER/CLOCK SVCs; no USBD register is
    // touched by this probe.
    unsafe {
        RawError::convert(raw::sd_power_usbdetected_enable(1)).unwrap();
        RawError::convert(raw::sd_power_usbpwrrdy_enable(1)).unwrap();
        RawError::convert(raw::sd_power_usbremoved_enable(1)).unwrap();
        RawError::convert(raw::sd_power_usbregstatus_get(&mut status)).unwrap();
    }

    let mut probe = PowerProbe::new(POWER_TIMEOUT_POLLS, HFCLK_TIMEOUT_POLLS, SUCCESS_HOLD_POLLS);
    let mut action = probe.observe_status(status);
    if action == PowerProbeAction::BeginHfclk {
        leds.show(LedStage::PowerReady);
        // Safety: this is the S140-owned HFCLK request SVC.
        unsafe { RawError::convert(raw::sd_clock_hfclk_request()).unwrap() };
    }
    loop {
        match probe.stage() {
            PowerProbeStage::WaitingPower => {
                while let Ok(event) = SOC_EVENTS.try_receive() {
                    let event = match event {
                        SocEvent::PowerUsbDetected => PowerEvent::Detected,
                        SocEvent::PowerUsbPowerReady => PowerEvent::PowerReady,
                        _ => continue,
                    };
                    action = probe.on_event(event);
                }
                if SOC_EVENT_OVERFLOWED.load(Ordering::Acquire) {
                    leds.show(LedStage::Timeout);
                    reset_into_uf2_bootloader();
                }
                if action == PowerProbeAction::BeginHfclk {
                    leds.show(LedStage::PowerReady);
                    // Safety: this is the S140-owned HFCLK request SVC.
                    unsafe { RawError::convert(raw::sd_clock_hfclk_request()).unwrap() };
                } else {
                    action = probe.poll_power();
                }
                if action == PowerProbeAction::ResetIntoUf2 {
                    leds.show(LedStage::Timeout);
                    reset_into_uf2_bootloader();
                }
            }
            PowerProbeStage::WaitingHfclk => {
                let mut running = 0;
                // Safety: SVC writes one u32 into this live stack variable.
                unsafe {
                    RawError::convert(raw::sd_clock_hfclk_is_running(&mut running)).unwrap();
                }
                action = probe.poll_hfclk(running != 0);
                if action == PowerProbeAction::HoldSuccess {
                    leds.show(LedStage::HfclkReady);
                } else if action == PowerProbeAction::ResetIntoUf2 {
                    leds.show(LedStage::Timeout);
                    reset_into_uf2_bootloader();
                }
            }
            PowerProbeStage::SuccessHold => {
                action = probe.poll_success_hold();
                if action == PowerProbeAction::ResetIntoUf2 {
                    reset_into_uf2_bootloader();
                }
            }
            PowerProbeStage::TimedOut => reset_into_uf2_bootloader(),
        }
        yield_now().await;
    }
}
