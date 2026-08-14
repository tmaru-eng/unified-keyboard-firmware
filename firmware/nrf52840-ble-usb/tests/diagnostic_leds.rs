//! Host-side contract tests for the XIAO startup diagnostic display.

use ukf_nrf52840_ble_usb::diagnostics::{LedState, StartupStage, led_state};

#[test]
fn startup_stages_have_distinct_static_led_states() {
    assert_eq!(
        led_state(StartupStage::Booting, false),
        LedState::new(false, true)
    );
    assert_eq!(
        led_state(StartupStage::SoftdeviceReady, false),
        LedState::new(true, true)
    );
    assert_eq!(
        led_state(StartupStage::UsbReady, false),
        LedState::new(false, false)
    );
    assert_eq!(
        led_state(StartupStage::BridgeRunning, false),
        LedState::new(true, false)
    );
}

#[test]
fn panic_stage_blinks_only_the_red_led() {
    assert_eq!(
        led_state(StartupStage::Panicked, false),
        LedState::new(false, true)
    );
    assert_eq!(
        led_state(StartupStage::Panicked, true),
        LedState::new(false, false)
    );
}
