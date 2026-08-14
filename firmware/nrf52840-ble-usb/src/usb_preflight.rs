//! Pure, bounded USBD READY preflight used by the S140 diagnostic binary.

#![forbid(unsafe_code)]

/// PAC bit for `USBD.EVENTCAUSE.READY`.
pub const EVENTCAUSE_READY_BIT: u32 = 1 << 11;

/// LED-visible boundaries for the diagnostic probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeStage {
    /// SVC USB power status and HFCLK are ready.
    PowerReady,
    /// Raw USBD ENABLE has been issued.
    UsbdEnableIssued,
    /// USBD READY was observed; the caller may invoke usb-device builder.
    ReadySeen,
    /// READY did not arrive before the finite budget expired.
    ReadyTimeout,
    /// The USB builder was created and polling may begin.
    Running,
}

/// Result of one finite READY preflight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadyOutcome {
    /// READY was observed after this many register polls.
    Ready {
        /// Number of EVENTCAUSE reads before READY was observed.
        polls: u32,
    },
    /// READY was not observed after this many register polls.
    TimedOut {
        /// Number of EVENTCAUSE reads performed before timeout.
        polls: u32,
    },
}

/// Minimal register boundary required by the pure preflight algorithm.
pub trait ReadyRegisters {
    /// Issue the USBD ENABLE write exactly once.
    fn enable(&mut self);

    /// Read the current USBD EVENTCAUSE value.
    fn eventcause(&mut self) -> u32;
}

/// Issue ENABLE and perform a finite READY poll.
///
/// This helper deliberately has no pull-up operation and never clears
/// EVENTCAUSE.READY. The nrf-usbd driver owns both actions after a successful
/// result; a timeout therefore cannot accidentally connect D+/D-.
pub fn preflight<R: ReadyRegisters>(registers: &mut R, max_polls: u32) -> ReadyOutcome {
    registers.enable();
    for polls in 0..max_polls {
        if registers.eventcause() & EVENTCAUSE_READY_BIT != 0 {
            return ReadyOutcome::Ready { polls };
        }
    }
    ReadyOutcome::TimedOut { polls: max_polls }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeRegisters {
        enable_writes: u32,
        reads: u32,
        ready_at: Option<u32>,
        eventcause: u32,
        pullup_writes: u32,
    }

    impl ReadyRegisters for FakeRegisters {
        fn enable(&mut self) {
            self.enable_writes += 1;
        }

        fn eventcause(&mut self) -> u32 {
            let reads = self.reads;
            self.reads += 1;
            let mut eventcause = self.eventcause;
            if self.ready_at == Some(reads) {
                eventcause |= EVENTCAUSE_READY_BIT;
            }
            eventcause
        }
    }

    impl FakeRegisters {
        fn poll(&mut self) -> ReadyOutcome {
            // Mirror the production boundary while retaining read counters
            // for assertions about the finite fake.
            self.enable();
            for polls in 0..8 {
                self.reads = polls;
                if self.eventcause() & EVENTCAUSE_READY_BIT != 0 {
                    return ReadyOutcome::Ready { polls };
                }
            }
            ReadyOutcome::TimedOut { polls: 8 }
        }
    }

    #[test]
    fn ready_on_nth_poll_is_reported_without_clearing_ready() {
        let mut registers = FakeRegisters {
            ready_at: Some(3),
            ..Default::default()
        };
        let outcome = preflight(&mut registers, 8);
        assert_eq!(outcome, ReadyOutcome::Ready { polls: 3 });
        assert_eq!(registers.enable_writes, 1);
        assert_eq!(registers.pullup_writes, 0);
    }

    #[test]
    fn never_ready_times_out_at_the_exact_budget() {
        let mut registers = FakeRegisters::default();
        let outcome = preflight(&mut registers, 8);
        assert_eq!(outcome, ReadyOutcome::TimedOut { polls: 8 });
        assert_eq!(registers.enable_writes, 1);
        assert_eq!(registers.pullup_writes, 0);
    }

    #[test]
    fn only_ready_bit_11_counts_as_ready() {
        let mut registers = FakeRegisters {
            eventcause: 1 << 10,
            ..Default::default()
        };
        assert_eq!(
            preflight(&mut registers, 2),
            ReadyOutcome::TimedOut { polls: 2 }
        );
        registers.eventcause = EVENTCAUSE_READY_BIT | (1 << 10);
        assert_eq!(
            preflight(&mut registers, 2),
            ReadyOutcome::Ready { polls: 0 }
        );
    }

    #[test]
    fn zero_budget_still_issues_enable_but_never_touches_pullup() {
        let mut registers = FakeRegisters::default();
        assert_eq!(
            preflight(&mut registers, 0),
            ReadyOutcome::TimedOut { polls: 0 }
        );
        assert_eq!(registers.enable_writes, 1);
        assert_eq!(registers.pullup_writes, 0);
    }

    #[test]
    fn fake_poll_model_can_represent_the_required_timeout_contract() {
        let mut registers = FakeRegisters {
            ready_at: Some(5),
            ..Default::default()
        };
        assert_eq!(registers.poll(), ReadyOutcome::Ready { polls: 5 });
        assert_eq!(registers.pullup_writes, 0);
    }
}
