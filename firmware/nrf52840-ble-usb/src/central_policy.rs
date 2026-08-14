//! Host-tested policy for the single-peer BLE central.

use crate::diagnostics_report::BridgeState;

/// Events observed by the radio adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CentralEvent {
    /// The controller accepted a scan start.
    ScanStarted,
    /// A connectable HID advertisement was selected.
    HidAdvertisement {
        /// Address copied from the advertising report.
        address: [u8; 6],
        /// RSSI copied from the advertising report.
        rssi: i8,
    },
    /// A connection completed.
    Connected,
    /// Link security completed.
    Secured,
    /// HID discovery and notification subscription completed.
    Subscribed,
    /// A transient operation failed and should be retried.
    RetryableFailure(u8),
    /// The peer disconnected.
    Disconnected,
}

/// Bounded policy state shared by the radio task and diagnostics snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CentralPolicy {
    state: BridgeState,
    last_error: u8,
    retry_count: u8,
}

impl CentralPolicy {
    /// Maximum consecutive transient failures before the state is reported as failed.
    pub const MAX_RETRIES: u8 = 3;

    /// Creates a policy that starts in the scanning phase.
    pub const fn new() -> Self {
        Self {
            state: BridgeState::Starting,
            last_error: 0,
            retry_count: 0,
        }
    }

    /// Current state for the diagnostics report.
    pub const fn state(self) -> BridgeState {
        self.state
    }

    /// Last error code, or zero when no failure is recorded.
    pub const fn last_error(self) -> u8 {
        self.last_error
    }

    /// Applies one adapter event and returns whether the adapter should retry.
    pub fn apply(&mut self, event: CentralEvent) -> bool {
        match event {
            CentralEvent::ScanStarted => {
                self.state = BridgeState::Scanning;
                self.last_error = 0;
                self.retry_count = 0;
                false
            }
            CentralEvent::HidAdvertisement { .. } => {
                self.state = BridgeState::Connecting;
                false
            }
            CentralEvent::Connected => {
                self.state = BridgeState::Securing;
                false
            }
            CentralEvent::Secured => {
                self.state = BridgeState::Discovering;
                false
            }
            CentralEvent::Subscribed => {
                self.state = BridgeState::Subscribed;
                self.last_error = 0;
                self.retry_count = 0;
                false
            }
            CentralEvent::RetryableFailure(error) => {
                self.last_error = error;
                self.retry_count = self.retry_count.saturating_add(1);
                if self.retry_count >= Self::MAX_RETRIES {
                    self.state = BridgeState::Failed;
                    false
                } else {
                    self.state = BridgeState::Scanning;
                    true
                }
            }
            CentralEvent::Disconnected => {
                self.state = BridgeState::Scanning;
                true
            }
        }
    }
}

impl Default for CentralPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    #[test]
    fn transient_failures_return_to_scanning_then_fail_boundlessly() {
        let mut policy = CentralPolicy::new();
        assert!(!policy.apply(CentralEvent::ScanStarted));
        assert!(policy.apply(CentralEvent::RetryableFailure(7)));
        assert_eq!(policy.state(), BridgeState::Scanning);
        assert!(policy.apply(CentralEvent::RetryableFailure(8)));
        assert!(!policy.apply(CentralEvent::RetryableFailure(9)));
        assert_eq!(policy.state(), BridgeState::Failed);
        assert_eq!(policy.last_error(), 9);
    }

    #[test]
    fn disconnect_requests_release_and_scan() {
        let mut policy = CentralPolicy::new();
        policy.apply(CentralEvent::ScanStarted);
        policy.apply(CentralEvent::HidAdvertisement {
            address: [0; 6],
            rssi: -40,
        });
        policy.apply(CentralEvent::Connected);
        policy.apply(CentralEvent::Secured);
        policy.apply(CentralEvent::Subscribed);
        assert!(policy.apply(CentralEvent::Disconnected));
        assert_eq!(policy.state(), BridgeState::Scanning);
    }
}
