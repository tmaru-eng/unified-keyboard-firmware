#![cfg_attr(all(target_arch = "arm", target_os = "none"), no_std)]
#![cfg_attr(all(target_arch = "arm", target_os = "none"), no_main)]
#![cfg_attr(not(all(target_arch = "arm", target_os = "none")), allow(dead_code))]
#![deny(unsafe_op_in_unsafe_fn)]

//! USB-only XIAO nRF52840 probe used to isolate USBD from S140 startup.

#[cfg(all(target_arch = "arm", target_os = "none"))]
use core::panic::PanicInfo;
#[cfg(all(target_arch = "arm", target_os = "none"))]
use cortex_m_rt::entry;
#[cfg(all(target_arch = "arm", target_os = "none"))]
use nrf_usbd::{UsbPeripheral, Usbd};
#[cfg(all(target_arch = "arm", target_os = "none"))]
use usb_device::{
    UsbError,
    bus::UsbBusAllocator,
    device::{StringDescriptors, UsbDeviceBuilder, UsbDeviceState, UsbVidPid},
};
#[cfg(all(target_arch = "arm", target_os = "none"))]
use usbd_hid::{
    descriptor::{KeyboardReport, SerializedDescriptor},
    hid_class::{
        HIDClass, HidClassSettings, HidCountryCode, HidProtocol, HidSubClass, ProtocolModeConfig,
    },
};

// Host-side tests use these values as the compatibility contract. The target
// firmware references them below; the host-only stub does not.
#[cfg_attr(not(all(target_arch = "arm", target_os = "none")), allow(dead_code))]
const USB_VID: u16 = 0xCAFE;
#[cfg_attr(not(all(target_arch = "arm", target_os = "none")), allow(dead_code))]
const USB_PID: u16 = 0xBAF2;
#[cfg_attr(not(all(target_arch = "arm", target_os = "none")), allow(dead_code))]
const USB_MANUFACTURER: &str = "nRF52840";
#[cfg_attr(not(all(target_arch = "arm", target_os = "none")), allow(dead_code))]
const USB_PRODUCT: &str = "US-JIS Keyboard Bridge";
#[cfg_attr(not(all(target_arch = "arm", target_os = "none")), allow(dead_code))]
const USB_MAX_POWER_MA: u16 = 100;
#[cfg_attr(not(all(target_arch = "arm", target_os = "none")), allow(dead_code))]
const RELEASE_REPORT: [u8; 8] = [0; 8];

const BLUE_LED_MASK: u32 = 1 << 6;
const RED_LED_MASK: u32 = 1 << 26;
const DIAGNOSTIC_LED_MASK: u32 = BLUE_LED_MASK | RED_LED_MASK;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProbeStage {
    Booting,
    HfclkReady,
    UsbPolled,
    Panicked,
}

/// Electrical levels for the XIAO's active-low blue and red LED pins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LedLevels {
    blue_pin_high: bool,
    red_pin_high: bool,
}

impl LedLevels {
    const fn new(blue_pin_high: bool, red_pin_high: bool) -> Self {
        Self {
            blue_pin_high,
            red_pin_high,
        }
    }
}

const fn led_levels(stage: ProbeStage, blink_phase: bool) -> LedLevels {
    match stage {
        ProbeStage::Booting => LedLevels::new(true, false),
        ProbeStage::HfclkReady => LedLevels::new(false, false),
        ProbeStage::UsbPolled => LedLevels::new(true, true),
        ProbeStage::Panicked => LedLevels::new(true, blink_phase),
    }
}

#[cfg(all(target_arch = "arm", target_os = "none"))]
const CLOCK_BASE: usize = 0x4000_0000;
#[cfg(all(target_arch = "arm", target_os = "none"))]
const TASKS_HFCLKSTART_OFFSET: usize = 0x000;
#[cfg(all(target_arch = "arm", target_os = "none"))]
const HFCLKSTAT_OFFSET: usize = 0x40c;
#[cfg(all(target_arch = "arm", target_os = "none"))]
const HFCLKSTAT_SRC_XTAL: u32 = 1;
#[cfg(all(target_arch = "arm", target_os = "none"))]
const HFCLKSTAT_STATE_RUNNING: u32 = 1 << 16;
#[cfg(all(target_arch = "arm", target_os = "none"))]
const GPIO0_BASE: usize = 0x5000_0000;
#[cfg(all(target_arch = "arm", target_os = "none"))]
const GPIO_OUTSET_OFFSET: usize = 0x508;
#[cfg(all(target_arch = "arm", target_os = "none"))]
const GPIO_OUTCLR_OFFSET: usize = 0x50c;
#[cfg(all(target_arch = "arm", target_os = "none"))]
const GPIO_DIRSET_OFFSET: usize = 0x518;

#[cfg(all(target_arch = "arm", target_os = "none"))]
struct DiagnosticLeds;

#[cfg(all(target_arch = "arm", target_os = "none"))]
impl DiagnosticLeds {
    fn init() -> Self {
        write_gpio(GPIO_OUTSET_OFFSET, DIAGNOSTIC_LED_MASK);
        write_gpio(GPIO_DIRSET_OFFSET, DIAGNOSTIC_LED_MASK);
        Self
    }

    fn show(&self, stage: ProbeStage, blink_phase: bool) {
        let levels = led_levels(stage, blink_phase);
        let high_mask = if levels.blue_pin_high {
            BLUE_LED_MASK
        } else {
            0
        } | if levels.red_pin_high { RED_LED_MASK } else { 0 };

        write_gpio(GPIO_OUTCLR_OFFSET, DIAGNOSTIC_LED_MASK & !high_mask);
        write_gpio(GPIO_OUTSET_OFFSET, high_mask);
    }
}

#[cfg(all(target_arch = "arm", target_os = "none"))]
fn write_gpio(offset: usize, mask: u32) {
    if mask == 0 {
        return;
    }

    // Safety: GPIO0 is the fixed nRF52840 register block. Each pointer is
    // naturally aligned and only the two dedicated XIAO LED bits are written.
    unsafe {
        core::ptr::write_volatile((GPIO0_BASE + offset) as *mut u32, mask);
    }
}

#[cfg(all(target_arch = "arm", target_os = "none"))]
#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    let leds = DiagnosticLeds::init();
    let mut blink_phase = false;

    loop {
        leds.show(ProbeStage::Panicked, blink_phase);
        blink_phase = !blink_phase;
        for _ in 0..16_000_000 {
            core::hint::spin_loop();
        }
    }
}

/// Exclusive application binding for the nRF52840 USBD register block.
#[cfg(all(target_arch = "arm", target_os = "none"))]
struct XiaoUsbd;

#[cfg(all(target_arch = "arm", target_os = "none"))]
unsafe impl UsbPeripheral for XiaoUsbd {
    const REGISTERS: *const () = 0x4002_7000 as *const ();
}

#[cfg(all(target_arch = "arm", target_os = "none"))]
fn start_external_hfclk() {
    // Safety: the SoftDevice is absent in this probe, so the application owns
    // CLOCK. Both addresses are fixed nRF52840 task/status registers and are
    // naturally aligned for volatile 32-bit access.
    unsafe {
        core::ptr::write_volatile((CLOCK_BASE + TASKS_HFCLKSTART_OFFSET) as *mut u32, 1);
        while core::ptr::read_volatile((CLOCK_BASE + HFCLKSTAT_OFFSET) as *const u32)
            & (HFCLKSTAT_SRC_XTAL | HFCLKSTAT_STATE_RUNNING)
            != (HFCLKSTAT_SRC_XTAL | HFCLKSTAT_STATE_RUNNING)
        {
            core::hint::spin_loop();
        }
    }
}

#[cfg(all(target_arch = "arm", target_os = "none"))]
#[entry]
fn embedded_main() -> ! {
    let diagnostic_leds = DiagnosticLeds::init();
    diagnostic_leds.show(ProbeStage::Booting, false);

    start_external_hfclk();
    diagnostic_leds.show(ProbeStage::HfclkReady, false);

    let usb_bus = UsbBusAllocator::new(Usbd::new(XiaoUsbd));
    let settings = HidClassSettings {
        subclass: HidSubClass::Boot,
        protocol: HidProtocol::Keyboard,
        config: ProtocolModeConfig::ForceBoot,
        locale: HidCountryCode::NotSupported,
    };
    let mut hid = HIDClass::new_ep_in_with_settings(&usb_bus, KeyboardReport::desc(), 8, settings);
    let strings = [StringDescriptors::default()
        .manufacturer(USB_MANUFACTURER)
        .product(USB_PRODUCT)];
    let mut usb = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(USB_VID, USB_PID))
        .strings(&strings)
        .unwrap()
        .self_powered(false)
        .max_power(usize::from(USB_MAX_POWER_MA))
        .unwrap()
        .max_packet_size_0(64)
        .unwrap()
        .build();
    let mut release_sent = false;
    let mut first_poll_pending = true;

    loop {
        usb.poll(&mut [&mut hid]);
        if first_poll_pending {
            diagnostic_leds.show(ProbeStage::UsbPolled, false);
            first_poll_pending = false;
        }

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
    }
}

#[cfg(not(all(target_arch = "arm", target_os = "none")))]
fn main() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_matches_the_observed_legacy_usb_identity() {
        assert_eq!((USB_VID, USB_PID), (0xCAFE, 0xBAF2));
        assert_eq!(USB_MANUFACTURER, "nRF52840");
        assert_eq!(USB_PRODUCT, "US-JIS Keyboard Bridge");
        assert_eq!(USB_MAX_POWER_MA, 100);
    }

    #[test]
    fn first_input_report_releases_every_boot_keyboard_slot() {
        assert_eq!(RELEASE_REPORT.len(), 8);
        assert!(RELEASE_REPORT.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn startup_stages_drive_the_xiao_led_pins_active_low() {
        assert_eq!(
            led_levels(ProbeStage::Booting, false),
            LedLevels::new(true, false)
        );
        assert_eq!(
            led_levels(ProbeStage::HfclkReady, false),
            LedLevels::new(false, false)
        );
        assert_eq!(
            led_levels(ProbeStage::UsbPolled, false),
            LedLevels::new(true, true)
        );
    }

    #[test]
    fn panic_blinks_only_the_red_led() {
        assert_eq!(
            led_levels(ProbeStage::Panicked, false),
            LedLevels::new(true, false)
        );
        assert_eq!(
            led_levels(ProbeStage::Panicked, true),
            LedLevels::new(true, true)
        );
    }

    #[test]
    fn led_masks_bind_red_to_p0_26_and_blue_to_p0_06() {
        assert_eq!(RED_LED_MASK, 1 << 26);
        assert_eq!(BLUE_LED_MASK, 1 << 6);
    }
}
