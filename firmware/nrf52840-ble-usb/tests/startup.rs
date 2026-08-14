//! Host-side tests for the finite application-startup state machine.

use ukf_nrf52840_ble_usb::startup::{
    InvalidTransition, RecoveryAction, StartupGate, StartupPoll, StartupStage,
};

#[test]
fn complete_startup_path_is_monotonic() {
    let mut gate = StartupGate::new(StartupStage::Booting, 8);
    for stage in [
        StartupStage::SoftdeviceReady,
        StartupStage::UsbPowerReady,
        StartupStage::UsbdReady,
        StartupStage::Running,
    ] {
        gate.transition(stage, 8).unwrap();
    }
    assert_eq!(gate.stage(), StartupStage::Running);
}

#[test]
fn skipped_stage_is_rejected_without_mutating_the_gate() {
    let mut gate = StartupGate::new(StartupStage::Booting, 5);
    assert_eq!(
        gate.transition(StartupStage::UsbdReady, 5),
        Err(InvalidTransition {
            current: StartupStage::Booting,
            requested: StartupStage::UsbdReady,
        })
    );
    assert_eq!(gate.stage(), StartupStage::Booting);
    assert_eq!(gate.remaining_polls(), 5);
}

#[test]
fn timeout_action_is_explicit_at_each_recovery_boundary() {
    for stage in [
        StartupStage::Booting,
        StartupStage::SoftdeviceReady,
        StartupStage::UsbPowerReady,
        StartupStage::UsbdReady,
    ] {
        let mut gate = StartupGate::new(stage, 0);
        assert_eq!(
            gate.poll(),
            StartupPoll::TimedOut(RecoveryAction::ResetIntoUf2Bootloader)
        );
    }
}
