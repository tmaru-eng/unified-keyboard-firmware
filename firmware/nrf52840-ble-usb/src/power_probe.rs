//! Pure finite state machine for the S140 USB power/HFCLK-only probe.

#![forbid(unsafe_code)]

/// `POWER.USBREGSTATUS` VBUS-detected bit.
pub const USBREGSTATUS_VBUSDETECT: u32 = 1 << 0;
/// `POWER.USBREGSTATUS` regulator-output-ready bit.
pub const USBREGSTATUS_OUTPUTRDY: u32 = 1 << 1;

/// Power events delivered by the SoftDevice callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerEvent {
    /// VBUS was detected.
    Detected,
    /// USB regulator output is ready.
    PowerReady,
}

/// Finite probe stages observable by the diagnostic LED.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerProbeStage {
    /// Waiting for USB power events or a ready status snapshot.
    WaitingPower,
    /// USB power is ready; waiting for S140 HFCLK to report running.
    WaitingHfclk,
    /// HFCLK is running; hold this state briefly before UF2 reset.
    SuccessHold,
    /// A finite boundary expired and UF2 recovery is required.
    TimedOut,
}

/// Side effect requested by the hardware adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerProbeAction {
    /// Continue yielding/polling the current boundary.
    Wait,
    /// The adapter may begin finite HFCLK polling.
    BeginHfclk,
    /// SVC preparation succeeded; hold the success LED before reset.
    HoldSuccess,
    /// Reset through the existing GPREGRET UF2 path.
    ResetIntoUf2,
}

/// State machine with independent finite budgets for power, HFCLK, and hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerProbe {
    stage: PowerProbeStage,
    detected: bool,
    power_ready: bool,
    remaining: u32,
    hfclk_budget: u32,
    hold_budget: u32,
}

impl PowerProbe {
    /// Creates a probe waiting for USB power status/events.
    pub const fn new(power_budget: u32, hfclk_budget: u32, hold_budget: u32) -> Self {
        Self {
            stage: PowerProbeStage::WaitingPower,
            detected: false,
            power_ready: false,
            remaining: power_budget,
            hfclk_budget,
            hold_budget,
        }
    }

    /// Returns the current finite stage.
    pub const fn stage(self) -> PowerProbeStage {
        self.stage
    }

    /// Applies the initial `USBREGSTATUS` snapshot.
    pub fn observe_status(&mut self, status: u32) -> PowerProbeAction {
        self.detected |= status & USBREGSTATUS_VBUSDETECT != 0;
        self.power_ready |= status & USBREGSTATUS_OUTPUTRDY != 0;
        self.try_begin_hfclk()
    }

    /// Applies a callback event, ignoring events after a terminal stage.
    pub fn on_event(&mut self, event: PowerEvent) -> PowerProbeAction {
        if self.stage != PowerProbeStage::WaitingPower {
            return PowerProbeAction::Wait;
        }
        match event {
            PowerEvent::Detected => self.detected = true,
            PowerEvent::PowerReady => self.power_ready = true,
        }
        self.try_begin_hfclk()
    }

    /// Consumes one finite USB power wait poll.
    pub fn poll_power(&mut self) -> PowerProbeAction {
        if self.stage != PowerProbeStage::WaitingPower {
            return PowerProbeAction::Wait;
        }
        if self.remaining == 0 {
            self.stage = PowerProbeStage::TimedOut;
            return PowerProbeAction::ResetIntoUf2;
        }
        self.remaining -= 1;
        PowerProbeAction::Wait
    }

    /// Consumes one finite HFCLK poll, or enters the success hold.
    pub fn poll_hfclk(&mut self, running: bool) -> PowerProbeAction {
        if self.stage != PowerProbeStage::WaitingHfclk {
            return PowerProbeAction::Wait;
        }
        if running {
            self.stage = PowerProbeStage::SuccessHold;
            self.remaining = self.hold_budget;
            return PowerProbeAction::HoldSuccess;
        }
        if self.remaining == 0 {
            self.stage = PowerProbeStage::TimedOut;
            return PowerProbeAction::ResetIntoUf2;
        }
        self.remaining -= 1;
        PowerProbeAction::Wait
    }

    /// Consumes one finite success hold poll before UF2 reset.
    pub fn poll_success_hold(&mut self) -> PowerProbeAction {
        if self.stage != PowerProbeStage::SuccessHold {
            return PowerProbeAction::Wait;
        }
        if self.remaining == 0 {
            self.stage = PowerProbeStage::TimedOut;
            return PowerProbeAction::ResetIntoUf2;
        }
        self.remaining -= 1;
        PowerProbeAction::Wait
    }

    fn try_begin_hfclk(&mut self) -> PowerProbeAction {
        if self.stage == PowerProbeStage::WaitingPower && self.detected && self.power_ready {
            self.stage = PowerProbeStage::WaitingHfclk;
            self.remaining = self.hfclk_budget;
            PowerProbeAction::BeginHfclk
        } else {
            PowerProbeAction::Wait
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_with_both_bits_enters_hfclk_once() {
        let mut probe = PowerProbe::new(3, 4, 2);
        assert_eq!(
            probe.observe_status(USBREGSTATUS_VBUSDETECT | USBREGSTATUS_OUTPUTRDY),
            PowerProbeAction::BeginHfclk
        );
        assert_eq!(probe.stage(), PowerProbeStage::WaitingHfclk);
        assert_eq!(probe.poll_hfclk(true), PowerProbeAction::HoldSuccess);
    }

    #[test]
    fn events_can_complete_power_in_order() {
        let mut probe = PowerProbe::new(3, 4, 2);
        assert_eq!(probe.on_event(PowerEvent::Detected), PowerProbeAction::Wait);
        assert_eq!(
            probe.on_event(PowerEvent::PowerReady),
            PowerProbeAction::BeginHfclk
        );
    }

    #[test]
    fn power_timeout_requests_uf2_reset() {
        let mut probe = PowerProbe::new(1, 1, 1);
        assert_eq!(probe.poll_power(), PowerProbeAction::Wait);
        assert_eq!(probe.poll_power(), PowerProbeAction::ResetIntoUf2);
        assert_eq!(probe.stage(), PowerProbeStage::TimedOut);
    }

    #[test]
    fn hfclk_timeout_is_finite_and_distinct_from_success() {
        let mut probe = PowerProbe::new(1, 2, 1);
        assert_eq!(
            probe.observe_status(USBREGSTATUS_VBUSDETECT | USBREGSTATUS_OUTPUTRDY),
            PowerProbeAction::BeginHfclk
        );
        assert_eq!(probe.poll_hfclk(false), PowerProbeAction::Wait);
        assert_eq!(probe.poll_hfclk(false), PowerProbeAction::Wait);
        assert_eq!(probe.poll_hfclk(false), PowerProbeAction::ResetIntoUf2);
    }

    #[test]
    fn successful_hold_resets_only_after_finite_hold() {
        let mut probe = PowerProbe::new(1, 1, 2);
        assert_eq!(
            probe.observe_status(USBREGSTATUS_VBUSDETECT | USBREGSTATUS_OUTPUTRDY),
            PowerProbeAction::BeginHfclk
        );
        assert_eq!(probe.poll_hfclk(true), PowerProbeAction::HoldSuccess);
        assert_eq!(probe.poll_success_hold(), PowerProbeAction::Wait);
        assert_eq!(probe.poll_success_hold(), PowerProbeAction::Wait);
        assert_eq!(probe.poll_success_hold(), PowerProbeAction::ResetIntoUf2);
    }

    #[test]
    fn late_events_do_not_restart_terminal_stage() {
        let mut probe = PowerProbe::new(0, 1, 1);
        assert_eq!(probe.poll_power(), PowerProbeAction::ResetIntoUf2);
        assert_eq!(probe.on_event(PowerEvent::Detected), PowerProbeAction::Wait);
        assert_eq!(probe.stage(), PowerProbeStage::TimedOut);
    }
}
