//! Host acceptance tests for the finite unexpected-panic recovery policy.

#[path = "../src/panic_recovery.rs"]
mod panic_recovery;

use panic_recovery::{PANIC_BLINK_PHASES, PanicRecovery, PanicRecoveryAction};

#[test]
fn panic_diagnostic_has_exactly_four_phases_then_uf2_reset() {
    let mut recovery = PanicRecovery::new();

    for _ in 0..PANIC_BLINK_PHASES {
        assert_eq!(recovery.next(), PanicRecoveryAction::Blink);
    }
    assert_eq!(recovery.next(), PanicRecoveryAction::ResetIntoUf2Bootloader);
}

#[test]
fn panic_recovery_stays_terminal_after_reset_is_requested() {
    let mut recovery = PanicRecovery::new();

    for _ in 0..PANIC_BLINK_PHASES {
        let _ = recovery.next();
    }
    assert_eq!(recovery.next(), PanicRecoveryAction::ResetIntoUf2Bootloader);
    assert_eq!(recovery.next(), PanicRecoveryAction::ResetIntoUf2Bootloader);
}
