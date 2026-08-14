//! Bounded application-startup state machine.
//!
//! Hardware adapters use this module to put a finite budget around each
//! startup boundary. The module has no timer, executor, or reset dependency;
//! callers provide one [`StartupGate::poll`] call per scheduler or spin poll
//! and perform the requested recovery action when the gate expires.

#![forbid(unsafe_code)]

/// Ordered milestones required before the bridge can run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupStage {
    /// The application reset vector was entered.
    Booting,
    /// S140 was enabled and its event runner was spawned.
    SoftdeviceReady,
    /// VBUS/USB regulator power and the requested HFCLK are ready.
    UsbPowerReady,
    /// The USBD peripheral reported that it is ready for the USB stack.
    UsbdReady,
    /// BLE-central and USB bridge tasks may run.
    Running,
}

impl StartupStage {
    /// Returns the next legal startup stage, if any.
    pub const fn next(self) -> Option<Self> {
        match self {
            Self::Booting => Some(Self::SoftdeviceReady),
            Self::SoftdeviceReady => Some(Self::UsbPowerReady),
            Self::UsbPowerReady => Some(Self::UsbdReady),
            Self::UsbdReady => Some(Self::Running),
            Self::Running => None,
        }
    }

    /// Returns the recovery action for a timeout at this stage.
    pub const fn timeout_recovery(self) -> RecoveryAction {
        match self {
            Self::Running => RecoveryAction::None,
            Self::Booting | Self::SoftdeviceReady | Self::UsbPowerReady | Self::UsbdReady => {
                RecoveryAction::ResetIntoUf2Bootloader
            }
        }
    }
}

/// Recovery side effect selected after a stage timeout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryAction {
    /// Continue waiting or remain in the running state.
    None,
    /// Leave the application through the board's UF2 reset path.
    ResetIntoUf2Bootloader,
}

/// Result of one finite startup poll.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupPoll {
    /// The stage still has budget remaining.
    Waiting,
    /// The stage budget is exhausted and the caller must apply the action.
    TimedOut(RecoveryAction),
}

/// Invalid stage transition requested by a hardware adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidTransition {
    /// Stage currently owned by the gate.
    pub current: StartupStage,
    /// Stage requested by the caller.
    pub requested: StartupStage,
}

/// Finite poll budget for one startup stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StartupGate {
    stage: StartupStage,
    remaining_polls: u32,
}

impl StartupGate {
    /// Creates a gate at `stage` with a finite poll budget.
    pub const fn new(stage: StartupStage, timeout_polls: u32) -> Self {
        Self {
            stage,
            remaining_polls: timeout_polls,
        }
    }

    /// Returns the stage currently being waited on.
    pub const fn stage(self) -> StartupStage {
        self.stage
    }

    /// Returns the number of polls remaining before timeout.
    pub const fn remaining_polls(self) -> u32 {
        self.remaining_polls
    }

    /// Advances to the next ordered stage and reloads its finite budget.
    pub fn transition(
        &mut self,
        requested: StartupStage,
        timeout_polls: u32,
    ) -> Result<(), InvalidTransition> {
        if self.stage.next() != Some(requested) {
            return Err(InvalidTransition {
                current: self.stage,
                requested,
            });
        }
        self.stage = requested;
        self.remaining_polls = timeout_polls;
        Ok(())
    }

    /// Reloads the budget while remaining in the current stage.
    ///
    /// This is useful when a stage has two finite sub-boundaries, such as USB
    /// power detection followed by the HFCLK-ready check.
    pub const fn rearm(&mut self, timeout_polls: u32) {
        self.remaining_polls = timeout_polls;
    }

    /// Consumes one budget unit without ever waiting indefinitely.
    pub const fn poll(&mut self) -> StartupPoll {
        if self.remaining_polls == 0 {
            StartupPoll::TimedOut(self.stage.timeout_recovery())
        } else {
            self.remaining_polls -= 1;
            StartupPoll::Waiting
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_are_strictly_ordered() {
        let mut gate = StartupGate::new(StartupStage::Booting, 3);
        assert_eq!(gate.transition(StartupStage::SoftdeviceReady, 4), Ok(()));
        assert_eq!(
            gate.transition(StartupStage::UsbdReady, 4),
            Err(InvalidTransition {
                current: StartupStage::SoftdeviceReady,
                requested: StartupStage::UsbdReady,
            })
        );
        assert_eq!(gate.stage(), StartupStage::SoftdeviceReady);
        assert_eq!(gate.remaining_polls(), 4);
    }

    #[test]
    fn finite_budget_times_out_with_uf2_recovery() {
        let mut gate = StartupGate::new(StartupStage::UsbPowerReady, 2);
        assert_eq!(gate.poll(), StartupPoll::Waiting);
        assert_eq!(gate.poll(), StartupPoll::Waiting);
        assert_eq!(
            gate.poll(),
            StartupPoll::TimedOut(RecoveryAction::ResetIntoUf2Bootloader)
        );
        assert_eq!(
            gate.poll(),
            StartupPoll::TimedOut(RecoveryAction::ResetIntoUf2Bootloader)
        );
    }

    #[test]
    fn each_pre_running_stage_requests_uf2_recovery() {
        for stage in [
            StartupStage::Booting,
            StartupStage::SoftdeviceReady,
            StartupStage::UsbPowerReady,
            StartupStage::UsbdReady,
        ] {
            assert_eq!(
                stage.timeout_recovery(),
                RecoveryAction::ResetIntoUf2Bootloader
            );
        }
        assert_eq!(
            StartupStage::Running.timeout_recovery(),
            RecoveryAction::None
        );
    }

    #[test]
    fn rearm_is_finite_and_does_not_change_stage() {
        let mut gate = StartupGate::new(StartupStage::UsbPowerReady, 0);
        assert_eq!(
            gate.poll(),
            StartupPoll::TimedOut(RecoveryAction::ResetIntoUf2Bootloader)
        );
        gate.rearm(1);
        assert_eq!(gate.stage(), StartupStage::UsbPowerReady);
        assert_eq!(gate.poll(), StartupPoll::Waiting);
        assert_eq!(
            gate.poll(),
            StartupPoll::TimedOut(RecoveryAction::ResetIntoUf2Bootloader)
        );
    }
}
