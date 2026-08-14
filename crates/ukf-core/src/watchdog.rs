//! Bounds how long the host can be left holding a key the user released.
//!
//! A bridge forwards a key press and a key release as two separate reports. If
//! anything swallows the release — the link dies mid-keystroke, the source
//! stops answering, the board resets — the host goes on believing the key is
//! down. That is not a cosmetic loss: a stuck meta key broke Japanese input on
//! the machine's own keyboard, which this bridge is supposed to leave alone.
//!
//! The hard part is that a held key and a dead link look identical from the
//! input side. A HID keyboard reports changes, so holding Shift for a minute
//! produces exactly as much traffic as a keyboard that stopped answering while
//! Shift was down: none. A watchdog driven by report traffic alone would
//! therefore have to choose between releasing keys the user is still holding
//! and never firing at all.
//!
//! So this type takes two different inputs. [`observe_output`] says what the
//! host is now holding, and [`observe_liveness`] says that the source is still
//! answering — which the caller has to establish some other way, by asking the
//! peer something and getting an answer. The deadline only exists while
//! something is held, and only silence from *both* inputs lets it expire.
//!
//! [`observe_output`]: StuckKeyWatchdog::observe_output
//! [`observe_liveness`]: StuckKeyWatchdog::observe_liveness

use crate::BootKeyboardReport;

/// Releases every key when the source stops proving it is still there.
///
/// Time is supplied by the caller in milliseconds from any fixed origin, so
/// this stays free of a clock and testable without one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StuckKeyWatchdog {
    limit_ms: u64,
    /// When the release becomes due, or `None` while the host holds nothing.
    deadline_ms: Option<u64>,
}

impl StuckKeyWatchdog {
    /// Creates a watchdog that releases after `limit_ms` without evidence.
    ///
    /// The limit has to exceed the longest gap the caller's liveness check can
    /// leave between two proofs, or a healthy link will be torn down while a
    /// key is legitimately held.
    pub const fn new(limit_ms: u64) -> Self {
        Self {
            limit_ms,
            deadline_ms: None,
        }
    }

    /// Records the report that was just emitted to the host.
    ///
    /// An emitted report is itself evidence: it only exists because the source
    /// just spoke. An empty one disarms the watchdog, because a host that holds
    /// nothing has nothing to be released from.
    pub fn observe_output(&mut self, report: BootKeyboardReport, now_ms: u64) {
        self.deadline_ms = if report == BootKeyboardReport::EMPTY {
            None
        } else {
            Some(now_ms.saturating_add(self.limit_ms))
        };
    }

    /// Records proof that the source is still answering.
    ///
    /// Ignored while the host holds nothing: liveness is a reason to keep a
    /// deadline open, never a reason to start one.
    pub fn observe_liveness(&mut self, now_ms: u64) {
        if self.deadline_ms.is_some() {
            self.deadline_ms = Some(now_ms.saturating_add(self.limit_ms));
        }
    }

    /// Whether every key must be released now.
    pub fn expired(&self, now_ms: u64) -> bool {
        self.deadline_ms.is_some_and(|deadline| now_ms >= deadline)
    }

    /// The instant at which [`Self::expired`] starts returning true.
    ///
    /// Exposed so an async caller can sleep until exactly that moment instead
    /// of polling, and so it can sleep forever while nothing is held.
    pub const fn deadline(&self) -> Option<u64> {
        self.deadline_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_limit_at_the_end_of_time_does_not_wrap_into_an_immediate_release() {
        // Saturating rather than wrapping. An overflow here would arm a
        // deadline in the past and release the keys on the next poll.
        let mut watchdog = StuckKeyWatchdog::new(5_000);
        let report = BootKeyboardReport {
            modifiers: 0,
            keys: [0x04, 0, 0, 0, 0, 0],
        };

        watchdog.observe_output(report, u64::MAX);

        assert_eq!(watchdog.deadline(), Some(u64::MAX));
        assert!(!watchdog.expired(u64::MAX - 1));
    }
}
