//! Pure ordering gate for the detected-before-READY S140 A/B probe.

#![forbid(unsafe_code)]

/// Stages intentionally split VBUS detection, HFCLK, USBD READY, and PWRRDY.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetectedOrderStage {
    /// Waiting for VBUS detection.
    WaitingDetected,
    /// Waiting for HFCLK after VBUS detection.
    WaitingHfclk,
    /// Waiting for raw USBD READY.
    WaitingReady,
    /// Waiting for the later PWRRDY observation.
    WaitingPowerReady,
    /// Holding a successful marker before UF2 reset.
    SuccessHold,
    /// A finite boundary expired.
    TimedOut,
}

/// Events observed through the S140 callback or status snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetectedOrderEvent {
    /// VBUS detection event.
    Detected,
    /// USB regulator power-ready event.
    PowerReady,
}

/// Finite action at one ordering boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetectedOrderAction {
    /// Continue finite polling.
    Wait,
    /// Request HFCLK after VBUS detection.
    RequestHfclk,
    /// Begin raw READY polling.
    BeginReady,
    /// Continue until PWRRDY is observed after READY.
    AwaitPowerReady,
    /// Hold the success marker.
    HoldSuccess,
    /// Reset through the UF2 path.
    ResetIntoUf2,
}

/// Pure finite gate used by the hardware probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DetectedOrderProbe {
    stage: DetectedOrderStage,
    remaining: u32,
    hfclk_budget: u32,
    ready_budget: u32,
    power_ready_budget: u32,
    hold_budget: u32,
    power_ready_seen: bool,
}

impl DetectedOrderProbe {
    /// Creates a gate that waits for VBUS detected before requesting HFCLK.
    pub const fn new(
        detected_budget: u32,
        hfclk_budget: u32,
        ready_budget: u32,
        power_ready_budget: u32,
        hold_budget: u32,
    ) -> Self {
        Self {
            stage: DetectedOrderStage::WaitingDetected,
            remaining: detected_budget,
            hfclk_budget,
            ready_budget,
            power_ready_budget,
            hold_budget,
            power_ready_seen: false,
        }
    }

    /// Returns the current stage.
    pub const fn stage(self) -> DetectedOrderStage {
        self.stage
    }

    /// Records a callback event, retaining early PWRRDY for post-READY audit.
    pub fn on_event(&mut self, event: DetectedOrderEvent) -> DetectedOrderAction {
        match event {
            DetectedOrderEvent::Detected if self.stage == DetectedOrderStage::WaitingDetected => {
                self.stage = DetectedOrderStage::WaitingHfclk;
                self.remaining = self.hfclk_budget;
                DetectedOrderAction::RequestHfclk
            }
            DetectedOrderEvent::PowerReady => {
                self.power_ready_seen = true;
                if self.stage == DetectedOrderStage::WaitingPowerReady {
                    self.stage = DetectedOrderStage::SuccessHold;
                    self.remaining = self.hold_budget;
                    DetectedOrderAction::HoldSuccess
                } else {
                    DetectedOrderAction::Wait
                }
            }
            _ => DetectedOrderAction::Wait,
        }
    }

    /// Consumes the detected wait budget.
    pub fn poll_detected(&mut self) -> DetectedOrderAction {
        if self.stage != DetectedOrderStage::WaitingDetected {
            return DetectedOrderAction::Wait;
        }
        if self.remaining == 0 {
            self.stage = DetectedOrderStage::TimedOut;
            DetectedOrderAction::ResetIntoUf2
        } else {
            self.remaining -= 1;
            DetectedOrderAction::Wait
        }
    }

    /// Polls HFCLK after detection and before raw USBD ENABLE.
    pub fn poll_hfclk(&mut self, running: bool) -> DetectedOrderAction {
        if self.stage != DetectedOrderStage::WaitingHfclk {
            return DetectedOrderAction::Wait;
        }
        if running {
            self.stage = DetectedOrderStage::WaitingReady;
            self.remaining = self.ready_budget;
            DetectedOrderAction::BeginReady
        } else if self.remaining == 0 {
            self.stage = DetectedOrderStage::TimedOut;
            DetectedOrderAction::ResetIntoUf2
        } else {
            self.remaining -= 1;
            DetectedOrderAction::Wait
        }
    }

    /// Marks raw READY success and defers PWRRDY acceptance until now.
    pub fn on_ready(&mut self) -> DetectedOrderAction {
        if self.stage != DetectedOrderStage::WaitingReady {
            return DetectedOrderAction::Wait;
        }
        self.stage = DetectedOrderStage::WaitingPowerReady;
        self.remaining = self.power_ready_budget;
        if self.power_ready_seen {
            self.stage = DetectedOrderStage::SuccessHold;
            self.remaining = self.hold_budget;
            DetectedOrderAction::HoldSuccess
        } else {
            DetectedOrderAction::AwaitPowerReady
        }
    }

    /// Consumes the post-READY PWRRDY budget.
    pub fn poll_power_ready(&mut self) -> DetectedOrderAction {
        if self.stage != DetectedOrderStage::WaitingPowerReady {
            return DetectedOrderAction::Wait;
        }
        if self.remaining == 0 {
            self.stage = DetectedOrderStage::TimedOut;
            DetectedOrderAction::ResetIntoUf2
        } else {
            self.remaining -= 1;
            DetectedOrderAction::Wait
        }
    }

    /// Consumes the finite success marker hold.
    pub fn poll_success_hold(&mut self) -> DetectedOrderAction {
        if self.stage != DetectedOrderStage::SuccessHold {
            return DetectedOrderAction::Wait;
        }
        if self.remaining == 0 {
            self.stage = DetectedOrderStage::TimedOut;
            DetectedOrderAction::ResetIntoUf2
        } else {
            self.remaining -= 1;
            DetectedOrderAction::Wait
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET: u32 = 2;

    fn probe() -> DetectedOrderProbe {
        DetectedOrderProbe::new(BUDGET, BUDGET, BUDGET, BUDGET, BUDGET)
    }

    #[test]
    fn detected_requests_hfclk_before_ready() {
        let mut probe = probe();
        assert_eq!(
            probe.on_event(DetectedOrderEvent::Detected),
            DetectedOrderAction::RequestHfclk
        );
        assert_eq!(probe.poll_hfclk(true), DetectedOrderAction::BeginReady);
        assert_eq!(probe.stage(), DetectedOrderStage::WaitingReady);
    }

    #[test]
    fn ready_defers_power_ready_until_after_raw_ready() {
        let mut probe = probe();
        probe.on_event(DetectedOrderEvent::Detected);
        probe.poll_hfclk(true);
        assert_eq!(probe.on_ready(), DetectedOrderAction::AwaitPowerReady);
        assert_eq!(
            probe.on_event(DetectedOrderEvent::PowerReady),
            DetectedOrderAction::HoldSuccess
        );
    }

    #[test]
    fn early_power_ready_is_recorded_but_does_not_skip_ready() {
        let mut probe = probe();
        probe.on_event(DetectedOrderEvent::PowerReady);
        probe.on_event(DetectedOrderEvent::Detected);
        probe.poll_hfclk(true);
        assert_eq!(probe.on_ready(), DetectedOrderAction::HoldSuccess);
    }

    #[test]
    fn each_boundary_is_finite() {
        let mut probe = probe();
        assert_eq!(probe.poll_detected(), DetectedOrderAction::Wait);
        assert_eq!(probe.poll_detected(), DetectedOrderAction::Wait);
        assert_eq!(probe.poll_detected(), DetectedOrderAction::ResetIntoUf2);
        assert_eq!(probe.stage(), DetectedOrderStage::TimedOut);
    }
}
