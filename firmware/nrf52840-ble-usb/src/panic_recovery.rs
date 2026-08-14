//! Finite recovery policy for unexpected panics in the board binary.
//!
//! Known runtime faults use their own explicit recovery boundaries. This
//! policy is only for faults that still reach the panic handler: preserve a
//! short, observable diagnostic marker, then return to UF2 instead of leaving
//! the USB pull-up attached forever.

#![forbid(unsafe_code)]

/// Number of LED phases emitted before requesting UF2 recovery.
pub const PANIC_BLINK_PHASES: u8 = 4;

/// Action selected by one panic-recovery phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanicRecoveryAction {
    /// Show one red diagnostic phase and continue the finite sequence.
    Blink,
    /// Request the existing UF2 reset path.
    ResetIntoUf2Bootloader,
}

/// Finite state machine used by the panic handler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PanicRecovery {
    remaining_phases: u8,
}

impl Default for PanicRecovery {
    fn default() -> Self {
        Self::new()
    }
}

impl PanicRecovery {
    /// Starts a fresh finite panic diagnostic sequence.
    pub const fn new() -> Self {
        Self {
            remaining_phases: PANIC_BLINK_PHASES,
        }
    }

    /// Consumes one phase and never waits indefinitely.
    pub const fn next(&mut self) -> PanicRecoveryAction {
        if self.remaining_phases == 0 {
            PanicRecoveryAction::ResetIntoUf2Bootloader
        } else {
            self.remaining_phases -= 1;
            PanicRecoveryAction::Blink
        }
    }
}
