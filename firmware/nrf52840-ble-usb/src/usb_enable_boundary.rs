//! Finite gate around a synchronous USB driver `enable` callback.

#![forbid(unsafe_code)]

/// Result of the finite pre-enable gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnableBoundary {
    /// READY was observed and the driver enable callback was invoked.
    DriverEnabled,
    /// READY timed out; the driver callback was not invoked.
    TimedOut,
}

/// Lifecycle of the two-phase nRF USBD startup.
///
/// nRF52840 turns on the USB regulator when USBD is enabled. The regulator's
/// PWRRDY event therefore follows the ENABLE/READY transaction; pull-up must
/// remain disconnected until that later event has been observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverStage {
    /// No USBD regulator enable has been attempted.
    Idle,
    /// ENABLE/READY completed; USBPWRDY is still required before start.
    Prepared,
    /// Pull-up/start phase completed after USBPWRDY.
    Started,
    /// A finite prepare or start boundary failed.
    TimedOut,
}

/// Result of a two-phase driver transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverAction {
    /// The ENABLE/READY prepare phase completed.
    Prepared,
    /// The post-PWRRDY pull-up/start phase completed.
    Started,
    /// The requested phase reached its finite timeout.
    TimedOut,
    /// The phase was requested in an invalid lifecycle state.
    Rejected,
}

/// Pure lifecycle guard for the legacy-compatible ENABLE/PWRRDY/start order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DriverLifecycle {
    stage: DriverStage,
}

impl DriverLifecycle {
    /// Create a lifecycle that has not touched USBD.
    pub const fn new() -> Self {
        Self {
            stage: DriverStage::Idle,
        }
    }

    /// Return the current lifecycle stage.
    pub const fn stage(self) -> DriverStage {
        self.stage
    }

    /// Run the single pre-PWRRDY ENABLE/READY phase.
    pub fn prepare<F>(&mut self, driver_prepare: F) -> DriverAction
    where
        F: FnOnce() -> bool,
    {
        if self.stage != DriverStage::Idle {
            return DriverAction::Rejected;
        }
        if driver_prepare() {
            self.stage = DriverStage::Prepared;
            DriverAction::Prepared
        } else {
            self.stage = DriverStage::TimedOut;
            DriverAction::TimedOut
        }
    }

    /// Run the post-PWRRDY start/pull-up phase exactly once.
    pub fn start<F>(&mut self, driver_start: F) -> DriverAction
    where
        F: FnOnce() -> bool,
    {
        if self.stage != DriverStage::Prepared {
            return DriverAction::Rejected;
        }
        if driver_start() {
            self.stage = DriverStage::Started;
            DriverAction::Started
        } else {
            self.stage = DriverStage::TimedOut;
            DriverAction::TimedOut
        }
    }
}

impl Default for DriverLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

/// Minimal readiness probe used before calling an unfallible driver enable.
pub trait ReadyProbe {
    /// Perform a finite readiness check without connecting the USB pull-up.
    fn ready(&mut self) -> bool;
}

/// Invoke the synchronous driver only after the finite readiness check passes.
pub fn gate<P, F>(probe: &mut P, driver_enable: F) -> EnableBoundary
where
    P: ReadyProbe,
    F: FnOnce(),
{
    if probe.ready() {
        driver_enable();
        EnableBoundary::DriverEnabled
    } else {
        EnableBoundary::TimedOut
    }
}

/// Let the driver own the single finite ENABLE/READY transaction.
///
/// This is the production path once the driver itself has a bounded
/// readiness wait. A second raw MMIO preflight would enable the peripheral
/// twice and diverges from the legacy nrfx lifecycle.
pub fn driver_owned_enable<F>(driver_enable: F) -> EnableBoundary
where
    F: FnOnce() -> bool,
{
    if driver_enable() {
        EnableBoundary::DriverEnabled
    } else {
        EnableBoundary::TimedOut
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    #[derive(Default)]
    struct FakeProbe {
        ready: bool,
        polls: u32,
    }

    impl ReadyProbe for FakeProbe {
        fn ready(&mut self) -> bool {
            self.polls += 1;
            self.ready
        }
    }

    #[test]
    fn ready_invokes_driver_exactly_once() {
        let mut probe = FakeProbe {
            ready: true,
            ..Default::default()
        };
        let driver_calls = Cell::new(0);
        let outcome = gate(&mut probe, || driver_calls.set(driver_calls.get() + 1));
        assert_eq!(outcome, EnableBoundary::DriverEnabled);
        assert_eq!(probe.polls, 1);
        assert_eq!(driver_calls.get(), 1);
    }

    #[test]
    fn timeout_skips_unbounded_driver_callback() {
        let mut probe = FakeProbe::default();
        let driver_calls = Cell::new(0);
        let outcome = gate(&mut probe, || driver_calls.set(driver_calls.get() + 1));
        assert_eq!(outcome, EnableBoundary::TimedOut);
        assert_eq!(probe.polls, 1);
        assert_eq!(driver_calls.get(), 0);
    }

    #[test]
    fn gate_does_not_call_driver_before_readiness() {
        let mut probe = FakeProbe::default();
        let mut pullup_writes = 0;
        let outcome = gate(&mut probe, || pullup_writes += 1);
        assert_eq!(outcome, EnableBoundary::TimedOut);
        assert_eq!(pullup_writes, 0);
    }

    #[test]
    fn driver_owned_enable_reports_one_finite_driver_attempt() {
        let driver_calls = Cell::new(0);
        let outcome = driver_owned_enable(|| {
            driver_calls.set(driver_calls.get() + 1);
            false
        });

        assert_eq!(outcome, EnableBoundary::TimedOut);
        assert_eq!(driver_calls.get(), 1);
    }

    #[test]
    fn prepare_precedes_start_and_each_driver_phase_runs_once() {
        let mut lifecycle = DriverLifecycle::new();
        let prepare_calls = Cell::new(0);
        let start_calls = Cell::new(0);

        assert_eq!(
            lifecycle.prepare(|| {
                prepare_calls.set(prepare_calls.get() + 1);
                true
            }),
            DriverAction::Prepared
        );
        assert_eq!(
            lifecycle.start(|| {
                start_calls.set(start_calls.get() + 1);
                true
            }),
            DriverAction::Started
        );
        assert_eq!(prepare_calls.get(), 1);
        assert_eq!(start_calls.get(), 1);
        assert_eq!(lifecycle.stage(), DriverStage::Started);
    }

    #[test]
    fn start_before_prepare_is_rejected_without_pullup() {
        let mut lifecycle = DriverLifecycle::new();
        let pullup_writes = Cell::new(0);
        assert_eq!(
            lifecycle.start(|| {
                pullup_writes.set(pullup_writes.get() + 1);
                true
            }),
            DriverAction::Rejected
        );
        assert_eq!(pullup_writes.get(), 0);
        assert_eq!(lifecycle.stage(), DriverStage::Idle);
    }

    #[test]
    fn prepare_timeout_blocks_start_and_pullup() {
        let mut lifecycle = DriverLifecycle::new();
        let pullup_writes = Cell::new(0);
        assert_eq!(lifecycle.prepare(|| false), DriverAction::TimedOut);
        assert_eq!(
            lifecycle.start(|| {
                pullup_writes.set(pullup_writes.get() + 1);
                true
            }),
            DriverAction::Rejected
        );
        assert_eq!(pullup_writes.get(), 0);
        assert_eq!(lifecycle.stage(), DriverStage::TimedOut);
    }
}
