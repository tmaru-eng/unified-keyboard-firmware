//! Host acceptance tests for the pure USB power-event state machine.

#[path = "../src/usb_power.rs"]
mod usb_power;

use usb_power::{
    UsbPowerAction, UsbPowerEvent, UsbPowerMachine, UsbPowerRecovery, UsbPowerState,
    UsbRuntimeFault, recovery_action, runtime_fault_recovery,
};

#[test]
fn runtime_faults_use_the_same_uf2_recovery_boundary_as_power_removal() {
    assert_eq!(
        runtime_fault_recovery(UsbRuntimeFault::SocEventQueueOverflow),
        UsbPowerRecovery::ResetIntoUf2Bootloader
    );
    assert_eq!(
        runtime_fault_recovery(UsbRuntimeFault::EndpointDmaTimeout),
        UsbPowerRecovery::ResetIntoUf2Bootloader
    );
}

#[test]
fn normal_power_lifecycle_never_panics_and_uses_uf2_recovery_when_rebuild_is_unsafe() {
    assert_eq!(
        recovery_action(UsbPowerAction::None),
        UsbPowerRecovery::NoAction
    );
    assert_eq!(
        recovery_action(UsbPowerAction::StartUsbd),
        UsbPowerRecovery::ResetIntoUf2Bootloader
    );
    assert_eq!(
        recovery_action(UsbPowerAction::ActivePowerRemoved),
        UsbPowerRecovery::ResetIntoUf2Bootloader
    );
}

#[test]
fn detected_then_power_ready_starts_usbd_exactly_once() {
    let mut power = UsbPowerMachine::new();

    assert_eq!(power.state(), UsbPowerState::Absent);
    assert_eq!(
        power.on_event(UsbPowerEvent::Detected),
        UsbPowerAction::None
    );
    assert_eq!(power.state(), UsbPowerState::Detected);
    assert_eq!(
        power.on_event(UsbPowerEvent::PowerReady),
        UsbPowerAction::StartUsbd
    );
    assert_eq!(power.state(), UsbPowerState::PowerReady);
    assert_eq!(
        power.on_event(UsbPowerEvent::PowerReady),
        UsbPowerAction::None
    );
}

#[test]
fn active_is_entered_only_after_the_adapter_confirms_usbd_started() {
    let mut power = UsbPowerMachine::new();

    assert_eq!(
        power.on_event(UsbPowerEvent::UsbdStarted),
        UsbPowerAction::None
    );
    power.on_event(UsbPowerEvent::Detected);
    assert_eq!(
        power.on_event(UsbPowerEvent::UsbdStarted),
        UsbPowerAction::None
    );
    power.on_event(UsbPowerEvent::PowerReady);
    assert_eq!(
        power.on_event(UsbPowerEvent::UsbdStarted),
        UsbPowerAction::None
    );
    assert_eq!(power.state(), UsbPowerState::Active);
}

#[test]
fn duplicate_and_out_of_order_events_do_not_restart_usbd() {
    let mut power = UsbPowerMachine::new();

    assert_eq!(
        power.on_event(UsbPowerEvent::PowerReady),
        UsbPowerAction::None
    );
    assert_eq!(power.state(), UsbPowerState::Absent);
    power.on_event(UsbPowerEvent::Detected);
    assert_eq!(
        power.on_event(UsbPowerEvent::Detected),
        UsbPowerAction::None
    );
    assert_eq!(
        power.on_event(UsbPowerEvent::PowerReady),
        UsbPowerAction::StartUsbd
    );
    power.on_event(UsbPowerEvent::UsbdStarted);
    assert_eq!(
        power.on_event(UsbPowerEvent::Detected),
        UsbPowerAction::None
    );
    assert_eq!(
        power.on_event(UsbPowerEvent::PowerReady),
        UsbPowerAction::None
    );
    assert_eq!(power.state(), UsbPowerState::Active);
}

#[test]
fn removed_before_ready_cancels_startup_and_requires_a_fresh_detection() {
    let mut power = UsbPowerMachine::new();

    power.on_event(UsbPowerEvent::Detected);
    assert_eq!(power.on_event(UsbPowerEvent::Removed), UsbPowerAction::None);
    assert_eq!(power.state(), UsbPowerState::Absent);
    assert_eq!(
        power.on_event(UsbPowerEvent::PowerReady),
        UsbPowerAction::None
    );
    assert_eq!(power.state(), UsbPowerState::Absent);

    power.on_event(UsbPowerEvent::Detected);
    assert_eq!(
        power.on_event(UsbPowerEvent::PowerReady),
        UsbPowerAction::StartUsbd
    );
}

#[test]
fn removed_while_active_reports_the_unsupported_stop_boundary() {
    let mut power = UsbPowerMachine::new();

    power.on_event(UsbPowerEvent::Detected);
    power.on_event(UsbPowerEvent::PowerReady);
    power.on_event(UsbPowerEvent::UsbdStarted);

    assert_eq!(
        power.on_event(UsbPowerEvent::Removed),
        UsbPowerAction::ActivePowerRemoved
    );
    assert_eq!(power.state(), UsbPowerState::Absent);
    assert_eq!(power.on_event(UsbPowerEvent::Removed), UsbPowerAction::None);
}

#[test]
fn active_removal_and_reattachment_choose_uf2_recovery_instead_of_panic() {
    let mut power = UsbPowerMachine::new();
    power.on_event(UsbPowerEvent::Detected);
    power.on_event(UsbPowerEvent::PowerReady);
    power.on_event(UsbPowerEvent::UsbdStarted);

    assert_eq!(
        recovery_action(power.on_event(UsbPowerEvent::Removed)),
        UsbPowerRecovery::ResetIntoUf2Bootloader
    );
    assert_eq!(power.state(), UsbPowerState::Absent);

    power.on_event(UsbPowerEvent::Detected);
    assert_eq!(
        recovery_action(power.on_event(UsbPowerEvent::PowerReady)),
        UsbPowerRecovery::ResetIntoUf2Bootloader
    );
}
