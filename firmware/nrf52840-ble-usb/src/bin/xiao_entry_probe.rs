#![cfg_attr(all(target_arch = "arm", target_os = "none"), no_std)]
#![cfg_attr(all(target_arch = "arm", target_os = "none"), no_main)]
#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(
    not(any(test, all(target_arch = "arm", target_os = "none"))),
    allow(dead_code)
)]

//! XIAO application-entry probe.
//!
//! This binary intentionally owns no CLOCK, POWER, USBD, SoftDevice, or
//! executor state. Its only target-side work is writing the two XIAO Sense
//! user-LED GPIO task registers immediately after the reset handler enters
//! `embedded_main`. That makes a failed LED pattern evidence of an application
//! hand-off/reset-vector problem rather than a transport initialization issue.

#[cfg(all(target_arch = "arm", target_os = "none"))]
use core::panic::PanicInfo;
#[cfg(all(target_arch = "arm", target_os = "none"))]
use cortex_m_rt::entry;

const BLUE_LED_MASK: u32 = 1 << 6;
const RED_LED_MASK: u32 = 1 << 26;
const LED_MASK: u32 = BLUE_LED_MASK | RED_LED_MASK;

const GPIO0_BASE: usize = 0x5000_0000;
const GPIO_OUTSET_OFFSET: usize = 0x508;
const GPIO_OUTCLR_OFFSET: usize = 0x50c;
const GPIO_DIRSET_OFFSET: usize = 0x518;

/// Number of spin iterations used to hold one visible state.
///
/// This is deliberately a coarse busy wait: the probe must not start CLOCK or
/// depend on a timer peripheral. It is long enough at the factory reset clock
/// to make each state observable, while still giving a one-second-ish
/// repeating signature on common nRF52840 clock configurations.
#[cfg(all(target_arch = "arm", target_os = "none"))]
const STARTUP_HOLD_CYCLES: u32 = 8_000_000;
#[cfg(all(target_arch = "arm", target_os = "none"))]
const PANIC_ON_CYCLES: u32 = 8_000_000;
#[cfg(all(target_arch = "arm", target_os = "none"))]
const PANIC_OFF_CYCLES: u32 = 2_000_000;

/// A logical LED state. The board-level output is active-low and is applied in
/// [`DiagnosticLeds::show`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LedPattern {
    Off,
    Blue,
    Red,
    Both,
}

impl LedPattern {
    const fn on_mask(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Blue => BLUE_LED_MASK,
            Self::Red => RED_LED_MASK,
            Self::Both => LED_MASK,
        }
    }
}

/// Long, asymmetric startup signature: blue, both, then red each receive a
/// two-hold pulse separated by an off state. Panic uses a much faster red-only
/// pulse, so the two conditions remain distinguishable by eye.
const STARTUP_PATTERN: [LedPattern; 9] = [
    LedPattern::Blue,
    LedPattern::Blue,
    LedPattern::Off,
    LedPattern::Both,
    LedPattern::Both,
    LedPattern::Off,
    LedPattern::Red,
    LedPattern::Red,
    LedPattern::Off,
];

/// Panic signature: red short-on/short-off, with blue always off.
const PANIC_PATTERN: [LedPattern; 2] = [LedPattern::Red, LedPattern::Off];

#[cfg(all(target_arch = "arm", target_os = "none"))]
struct DiagnosticLeds;

#[cfg(all(target_arch = "arm", target_os = "none"))]
impl DiagnosticLeds {
    fn init() -> Self {
        // Set the output latch high before enabling output direction so both
        // active-low LEDs start off without a setup-time flash.
        write_gpio(GPIO_OUTSET_OFFSET, LED_MASK);
        write_gpio(GPIO_DIRSET_OFFSET, LED_MASK);
        Self
    }

    fn show(&self, pattern: LedPattern) {
        let on_mask = pattern.on_mask();
        let off_mask = LED_MASK & !on_mask;

        // Safety: GPIO0 is the fixed nRF52840 register block. These pointers
        // target write-only task registers, are naturally aligned, and only
        // receive the two dedicated XIAO Sense LED bits.
        write_gpio(GPIO_OUTCLR_OFFSET, on_mask);
        write_gpio(GPIO_OUTSET_OFFSET, off_mask);
    }
}

#[cfg(all(target_arch = "arm", target_os = "none"))]
fn write_gpio(offset: usize, mask: u32) {
    if mask == 0 {
        return;
    }

    // Safety: see `DiagnosticLeds::show`; this function is only called with
    // the fixed GPIO0 task-register offsets and the board LED mask.
    unsafe {
        core::ptr::write_volatile((GPIO0_BASE + offset) as *mut u32, mask);
    }
}

#[cfg(all(target_arch = "arm", target_os = "none"))]
fn spin_delay(cycles: u32) {
    for _ in 0..cycles {
        core::hint::spin_loop();
    }
}

#[cfg(all(target_arch = "arm", target_os = "none"))]
#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    let leds = DiagnosticLeds::init();
    let mut phase = 0;

    loop {
        let pattern = PANIC_PATTERN[phase];
        leds.show(pattern);
        spin_delay(if pattern == LedPattern::Red {
            PANIC_ON_CYCLES
        } else {
            PANIC_OFF_CYCLES
        });
        phase = (phase + 1) % PANIC_PATTERN.len();
    }
}

#[cfg(all(target_arch = "arm", target_os = "none"))]
#[entry]
fn embedded_main() -> ! {
    // This is intentionally the first application operation. If this pattern
    // is absent after a UF2 hand-off, no peripheral initialization is involved
    // in the failure.
    let leds = DiagnosticLeds::init();
    let mut phase = 0;

    loop {
        leds.show(STARTUP_PATTERN[phase]);
        spin_delay(STARTUP_HOLD_CYCLES);
        phase = (phase + 1) % STARTUP_PATTERN.len();
    }
}

#[cfg(not(all(target_arch = "arm", target_os = "none")))]
fn main() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_signature_is_asymmetric_and_contains_each_led_state() {
        assert_eq!(STARTUP_PATTERN[0], LedPattern::Blue);
        assert_eq!(STARTUP_PATTERN[2], LedPattern::Off);
        assert_eq!(STARTUP_PATTERN[3], LedPattern::Both);
        assert_eq!(STARTUP_PATTERN[6], LedPattern::Red);
        assert!(STARTUP_PATTERN.contains(&LedPattern::Blue));
        assert!(STARTUP_PATTERN.contains(&LedPattern::Red));
        assert!(STARTUP_PATTERN.contains(&LedPattern::Both));
    }

    #[test]
    fn panic_signature_is_red_only() {
        assert_eq!(PANIC_PATTERN, [LedPattern::Red, LedPattern::Off]);
        assert_eq!(LedPattern::Red.on_mask(), RED_LED_MASK);
        assert_eq!(LedPattern::Off.on_mask(), 0);
    }

    #[test]
    fn xiao_led_masks_and_task_registers_are_fixed() {
        assert_eq!(BLUE_LED_MASK, 1 << 6);
        assert_eq!(RED_LED_MASK, 1 << 26);
        assert_eq!(GPIO0_BASE, 0x5000_0000);
        assert_eq!(GPIO_OUTSET_OFFSET, 0x508);
        assert_eq!(GPIO_OUTCLR_OFFSET, 0x50c);
        assert_eq!(GPIO_DIRSET_OFFSET, 0x518);
    }
}
