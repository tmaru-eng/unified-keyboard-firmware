//! Latest-value storage for configuration changes crossing task boundaries.
//!
//! Configuration changes are state, not radio events. Keeping the latest
//! value here lets the report task apply it even while the ordinary radio
//! event queue contains disconnect or notification records.

use crate::profile_store::StoredProfile;
use ukf_core::{BridgeProfile, Keymap, SOURCE_SLOT_COUNT};

/// Pending configuration values waiting for the report task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigurationUpdates {
    profile: Option<StoredProfile>,
    keymap: Option<Keymap>,
    source_profiles: [Option<BridgeProfile>; SOURCE_SLOT_COUNT],
}

impl ConfigurationUpdates {
    /// Creates an empty update store.
    pub const fn new() -> Self {
        Self {
            profile: None,
            keymap: None,
            source_profiles: [None; SOURCE_SLOT_COUNT],
        }
    }

    /// Replaces the pending global profile with the latest committed value.
    pub fn stage_profile(&mut self, profile: StoredProfile) {
        self.profile = Some(profile);
    }

    /// Takes the latest pending global profile, if any.
    pub fn take_profile(&mut self) -> Option<StoredProfile> {
        self.profile.take()
    }

    /// Replaces the pending global keymap with the latest committed value.
    pub fn stage_keymap(&mut self, keymap: Keymap) {
        self.keymap = Some(keymap);
    }

    /// Takes the latest pending global keymap, if any.
    pub fn take_keymap(&mut self) -> Option<Keymap> {
        self.keymap.take()
    }

    /// Replaces one source slot's pending profile.
    ///
    /// Returns `false` for a slot outside the fixed source table.
    pub fn stage_source_profile(&mut self, slot: u8, profile: BridgeProfile) -> bool {
        let Some(pending) = self.source_profiles.get_mut(usize::from(slot)) else {
            return false;
        };
        *pending = Some(profile);
        true
    }

    /// Takes one source slot's latest pending profile.
    pub fn take_source_profile(&mut self, slot: u8) -> Option<BridgeProfile> {
        self.source_profiles
            .get_mut(usize::from(slot))
            .and_then(Option::take)
    }

    /// Whether a global profile or keymap is waiting.
    pub const fn has_global_updates(&self) -> bool {
        self.profile.is_some() || self.keymap.is_some()
    }

    /// Whether any source-specific profile is waiting.
    pub fn has_source_updates(&self) -> bool {
        self.source_profiles.iter().any(Option::is_some)
    }
}

impl Default for ConfigurationUpdates {
    fn default() -> Self {
        Self::new()
    }
}
