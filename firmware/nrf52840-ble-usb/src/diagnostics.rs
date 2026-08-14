//! Pure startup-stage to board-LED diagnostic mapping.
//!
//! The board adapter owns electrical details such as active-low GPIO. Keeping
//! this module hardware-independent makes the diagnostic contract testable on
//! the host.

/// Coarse milestones used to identify where board startup stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupStage {
    /// Execution reached the application entry point.
    Booting,
    /// S140 was enabled and its event runner was spawned.
    SoftdeviceReady,
    /// The USB device and HID class were constructed.
    UsbReady,
    /// Both BLE-central and USB bridge loops are about to run.
    BridgeRunning,
    /// A panic stopped normal execution.
    Panicked,
}

/// Logical LED state, independent of the board's GPIO polarity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LedState {
    blue_on: bool,
    red_on: bool,
}

impl LedState {
    /// Creates a logical blue/red LED state.
    pub const fn new(blue_on: bool, red_on: bool) -> Self {
        Self { blue_on, red_on }
    }

    /// Returns whether the blue LED should be illuminated.
    pub const fn blue_on(self) -> bool {
        self.blue_on
    }

    /// Returns whether the red LED should be illuminated.
    pub const fn red_on(self) -> bool {
        self.red_on
    }
}

/// Maps a startup milestone to a logical LED state.
///
/// `blink_phase` only affects [`StartupStage::Panicked`]. Alternating it makes
/// the red LED blink while all normal milestones remain static.
pub const fn led_state(stage: StartupStage, blink_phase: bool) -> LedState {
    match stage {
        StartupStage::Booting => LedState::new(false, true),
        StartupStage::SoftdeviceReady => LedState::new(true, true),
        StartupStage::UsbReady => LedState::new(false, false),
        StartupStage::BridgeRunning => LedState::new(true, false),
        StartupStage::Panicked => LedState::new(false, !blink_phase),
    }
}
