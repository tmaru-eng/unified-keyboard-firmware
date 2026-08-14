//! Pure USB power-event ordering for an S140-owned POWER peripheral.
//!
//! The board adapter translates its initial `USBREGSTATUS` snapshot and later
//! SoftDevice SoC events into [`UsbPowerEvent`] values. This module deliberately
//! does not depend on `nrf-softdevice`: host tests can therefore lock down the
//! ordering before the callback and USBD adapter are wired to hardware.

/// USB power and peripheral lifecycle visible to the application.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UsbPowerState {
    /// VBUS is absent, or no detection has been observed yet.
    #[default]
    Absent,
    /// VBUS was detected, but the USB regulator is not ready yet.
    Detected,
    /// USB power is ready and the adapter may start USBD.
    PowerReady,
    /// The adapter confirmed that USBD was started.
    Active,
}

/// Hardware-independent input accepted by [`UsbPowerMachine`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsbPowerEvent {
    /// VBUS detection was observed.
    Detected,
    /// The USB regulator reported that its output is ready.
    PowerReady,
    /// The board adapter successfully started USBD.
    UsbdStarted,
    /// VBUS removal was observed.
    Removed,
}

/// Side effect requested from the board adapter by a state transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsbPowerAction {
    /// No adapter action is required.
    None,
    /// Construct and start the USBD stack exactly once for this attachment.
    StartUsbd,
    /// Power disappeared while USBD was active.
    ///
    /// `nrf-usbd` 0.3.0 and `usb-device` 0.3.2 do not expose the disable and
    /// rebuild lifecycle needed to recover in place. The adapter must treat
    /// this as an explicit unsupported boundary, for example by resetting.
    ActivePowerRemoved,
}

/// Recovery side effect selected by the board adapter for a power action.
///
/// The current polling USBD stack cannot safely disable and rebuild an active
/// instance in place. Startup and reattachment therefore use the same
/// deterministic UF2 recovery path instead of entering a panic loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsbPowerRecovery {
    /// Continue polling the current lifecycle.
    NoAction,
    /// Set the UF2 GPREGRET marker and reset the MCU.
    ResetIntoUf2Bootloader,
}

/// Runtime faults that make the current USB instance unsafe to keep polling.
///
/// These faults are deliberately separate from [`UsbPowerAction`]: they can
/// be raised after the adapter has become active, but they still share the
/// same deterministic UF2 recovery boundary as active VBUS removal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsbRuntimeFault {
    /// The SoftDevice USB-power event channel could not accept an event.
    SocEventQueueOverflow,
    /// An endpoint DMA completion did not arrive within its finite budget.
    EndpointDmaTimeout,
}

/// Selects recovery for a runtime fault without entering the panic handler.
pub const fn runtime_fault_recovery(fault: UsbRuntimeFault) -> UsbPowerRecovery {
    match fault {
        UsbRuntimeFault::SocEventQueueOverflow | UsbRuntimeFault::EndpointDmaTimeout => {
            UsbPowerRecovery::ResetIntoUf2Bootloader
        }
    }
}

/// Converts a pure power-machine action into its adapter recovery policy.
///
/// Keeping this decision outside the board-specific reset implementation lets
/// host tests prove that normal removal and reattachment never become a panic
/// path. The adapter is still responsible for executing the reset action.
pub const fn recovery_action(action: UsbPowerAction) -> UsbPowerRecovery {
    match action {
        UsbPowerAction::None => UsbPowerRecovery::NoAction,
        UsbPowerAction::StartUsbd | UsbPowerAction::ActivePowerRemoved => {
            UsbPowerRecovery::ResetIntoUf2Bootloader
        }
    }
}

/// Deterministic USB power-event state machine.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UsbPowerMachine {
    state: UsbPowerState,
}

impl UsbPowerMachine {
    /// Creates a machine with no observed VBUS.
    pub const fn new() -> Self {
        Self {
            state: UsbPowerState::Absent,
        }
    }

    /// Returns the current lifecycle state.
    pub const fn state(&self) -> UsbPowerState {
        self.state
    }

    /// Applies one ordered power event and returns the required adapter action.
    ///
    /// Duplicate events are idempotent. A power-ready event is accepted only
    /// after detection, so a late event left behind by a removed attachment
    /// cannot start USBD. For an already-powered cold boot, the adapter should
    /// translate its status snapshot into `Detected` followed by `PowerReady`.
    pub fn on_event(&mut self, event: UsbPowerEvent) -> UsbPowerAction {
        use UsbPowerAction::{ActivePowerRemoved, None, StartUsbd};
        use UsbPowerEvent::{Detected, PowerReady, Removed, UsbdStarted};
        use UsbPowerState::{Absent, Active};

        match (self.state, event) {
            (Active, Removed) => {
                self.state = Absent;
                ActivePowerRemoved
            }
            (_, Removed) => {
                self.state = Absent;
                None
            }
            (Absent, Detected) => {
                self.state = UsbPowerState::Detected;
                None
            }
            (UsbPowerState::Detected, PowerReady) => {
                self.state = UsbPowerState::PowerReady;
                StartUsbd
            }
            (UsbPowerState::PowerReady, UsbdStarted) => {
                self.state = Active;
                None
            }
            _ => None,
        }
    }
}
