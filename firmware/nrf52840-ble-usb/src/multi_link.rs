//! Host-tested ownership state for concurrent BLE input links.
//!
//! A bond slot is a persistent user-facing identity. A link worker is a
//! temporary runtime resource. Keeping those concepts separate prevents a
//! second connection from replacing the first connection's source, report
//! handle, or lifecycle state.

use ukf_core::{REGISTERED_SOURCE_SLOTS, SourceId, SourceState};

/// Number of BLE links enabled for the first simultaneous-connection phase.
pub const MAX_ACTIVE_BLE_LINKS: usize = 2;

/// Runtime worker identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkWorkerId(pub usize);

/// Why a link worker assignment was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkAssignmentError {
    /// The source slot is outside the persistent BLE registration range.
    InvalidSource(SourceId),
    /// That source slot already owns a worker.
    SourceAlreadyActive(SourceId),
    /// Every runtime worker is already assigned.
    NoWorkerAvailable,
}

/// Fixed-capacity mapping between persistent source slots and runtime workers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MultiLinkSet {
    workers: [Option<SourceId>; MAX_ACTIVE_BLE_LINKS],
    states: [SourceState; REGISTERED_SOURCE_SLOTS],
}

impl MultiLinkSet {
    /// Creates an empty link set.
    pub const fn new() -> Self {
        Self {
            workers: [None; MAX_ACTIVE_BLE_LINKS],
            states: [SourceState::Disconnected; REGISTERED_SOURCE_SLOTS],
        }
    }

    /// Reserves a worker for a source and starts its connection lifecycle.
    pub fn reserve(&mut self, source: SourceId) -> Result<LinkWorkerId, LinkAssignmentError> {
        let source_index = source
            .index()
            .filter(|index| *index < REGISTERED_SOURCE_SLOTS)
            .ok_or(LinkAssignmentError::InvalidSource(source))?;
        if self.workers.contains(&Some(source)) {
            return Err(LinkAssignmentError::SourceAlreadyActive(source));
        }
        let worker = self
            .workers
            .iter()
            .position(Option::is_none)
            .ok_or(LinkAssignmentError::NoWorkerAvailable)?;
        self.workers[worker] = Some(source);
        self.states[source_index] = SourceState::Connecting;
        Ok(LinkWorkerId(worker))
    }

    /// Advances one worker's source-local lifecycle state.
    pub fn set_state(&mut self, worker: LinkWorkerId, state: SourceState) -> Option<SourceId> {
        let source = self.workers.get(worker.0).copied().flatten()?;
        self.states[usize::from(source.0)] = state;
        Some(source)
    }

    /// Returns the source assigned to a worker, if any.
    pub fn source_for(&self, worker: LinkWorkerId) -> Option<SourceId> {
        self.workers.get(worker.0).copied().flatten()
    }

    /// Returns the worker currently assigned to a source, if any.
    pub fn worker_for(&self, source: SourceId) -> Option<LinkWorkerId> {
        self.workers
            .iter()
            .position(|assigned| *assigned == Some(source))
            .map(LinkWorkerId)
    }

    /// Returns a source-local state without exposing worker details.
    pub fn state(&self, source: SourceId) -> Option<SourceState> {
        let index = source
            .index()
            .filter(|index| *index < REGISTERED_SOURCE_SLOTS)?;
        Some(self.states[index])
    }

    /// Releases one worker after its link has disconnected.
    pub fn release(&mut self, worker: LinkWorkerId) -> Option<SourceId> {
        let source = self.workers.get_mut(worker.0)?.take()?;
        self.states[usize::from(source.0)] = SourceState::Disconnected;
        Some(source)
    }

    /// Number of workers currently assigned to BLE links.
    pub fn active_count(&self) -> usize {
        self.workers
            .iter()
            .filter(|source| source.is_some())
            .count()
    }
}

impl Default for MultiLinkSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    #[test]
    fn reserves_two_sources_without_replacing_either() {
        let mut links = MultiLinkSet::new();
        let first = links.reserve(SourceId(0)).unwrap();
        let second = links.reserve(SourceId(1)).unwrap();

        assert_ne!(first, second);
        assert_eq!(links.active_count(), 2);
        assert_eq!(links.worker_for(SourceId(0)), Some(first));
        assert_eq!(links.worker_for(SourceId(1)), Some(second));
        assert_eq!(links.state(SourceId(0)), Some(SourceState::Connecting));
        assert_eq!(links.state(SourceId(1)), Some(SourceState::Connecting));
    }

    #[test]
    fn refuses_duplicate_source_and_third_link() {
        let mut links = MultiLinkSet::new();
        links.reserve(SourceId(0)).unwrap();

        assert_eq!(
            links.reserve(SourceId(0)),
            Err(LinkAssignmentError::SourceAlreadyActive(SourceId(0)))
        );
        links.reserve(SourceId(1)).unwrap();
        assert_eq!(
            links.reserve(SourceId(2)),
            Err(LinkAssignmentError::NoWorkerAvailable)
        );
    }

    #[test]
    fn disconnecting_one_source_keeps_the_other_and_reuses_worker() {
        let mut links = MultiLinkSet::new();
        let first = links.reserve(SourceId(0)).unwrap();
        let second = links.reserve(SourceId(1)).unwrap();
        links.set_state(first, SourceState::Connected);
        links.set_state(second, SourceState::Connected);

        assert_eq!(links.release(first), Some(SourceId(0)));
        assert_eq!(links.active_count(), 1);
        assert_eq!(links.state(SourceId(0)), Some(SourceState::Disconnected));
        assert_eq!(links.state(SourceId(1)), Some(SourceState::Connected));

        let reused = links.reserve(SourceId(2)).unwrap();
        assert_eq!(reused, first);
        assert_eq!(links.worker_for(SourceId(1)), Some(second));
        assert_eq!(links.worker_for(SourceId(2)), Some(first));
    }

    #[test]
    fn state_updates_are_scoped_to_the_assigned_source() {
        let mut links = MultiLinkSet::new();
        let first = links.reserve(SourceId(0)).unwrap();
        let second = links.reserve(SourceId(1)).unwrap();

        links.set_state(first, SourceState::Securing);
        links.set_state(second, SourceState::Discovering);

        assert_eq!(links.state(SourceId(0)), Some(SourceState::Securing));
        assert_eq!(links.state(SourceId(1)), Some(SourceState::Discovering));
    }
}
