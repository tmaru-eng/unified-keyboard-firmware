/// Result of one bounded DMA completion observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmaPoll {
    /// The DMA completion event has not arrived and budget remains.
    Waiting,
    /// The DMA completion event was observed.
    Complete,
    /// The finite budget was exhausted without completion.
    TimedOut,
}

/// Finite state-free budget for a single USBD DMA transaction.
///
/// The hardware driver owns the event register; this helper only defines the
/// exact polling contract so it can be tested without MMIO. A completion event
/// wins even when the budget is zero, while a missing event can never spin
/// indefinitely.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmaGate {
    remaining: u32,
}

impl DmaGate {
    /// Creates a gate allowing at most `budget` event-register observations.
    pub const fn new(budget: u32) -> Self {
        Self { remaining: budget }
    }

    /// Observes one completion event and returns the next bounded state.
    pub fn poll(&mut self, completed: bool) -> DmaPoll {
        if completed {
            return DmaPoll::Complete;
        }
        if self.remaining == 0 {
            return DmaPoll::TimedOut;
        }

        self.remaining -= 1;
        if self.remaining == 0 {
            DmaPoll::TimedOut
        } else {
            DmaPoll::Waiting
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DmaGate, DmaPoll};

    #[test]
    fn completion_wins_when_end_event_arrives_before_budget() {
        let mut gate = DmaGate::new(3);

        assert_eq!(gate.poll(false), DmaPoll::Waiting);
        assert_eq!(gate.poll(false), DmaPoll::Waiting);
        assert_eq!(gate.poll(true), DmaPoll::Complete);
    }

    #[test]
    fn missing_end_event_times_out_at_the_exact_finite_budget() {
        let mut gate = DmaGate::new(3);

        assert_eq!(gate.poll(false), DmaPoll::Waiting);
        assert_eq!(gate.poll(false), DmaPoll::Waiting);
        assert_eq!(gate.poll(false), DmaPoll::TimedOut);
        assert_eq!(gate.poll(false), DmaPoll::TimedOut);
    }

    #[test]
    fn zero_budget_does_not_spin_and_an_observed_event_still_completes() {
        let mut gate = DmaGate::new(0);

        assert_eq!(gate.poll(false), DmaPoll::TimedOut);
        assert_eq!(gate.poll(true), DmaPoll::Complete);
    }
}
