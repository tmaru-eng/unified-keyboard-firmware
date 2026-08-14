//! Host-side contract tests for configuration updates crossing task boundaries.

use ukf_core::{BridgeProfile, Keymap, KeymapRule};
use ukf_nrf52840_ble_usb::configuration_updates::ConfigurationUpdates;
use ukf_nrf52840_ble_usb::profile_store::StoredProfile;

#[test]
fn latest_global_updates_are_retained_until_the_report_task_drains_them() {
    let mut updates = ConfigurationUpdates::new();
    let first = StoredProfile {
        us_to_jis: false,
        caps_to_ctrl: false,
        swap_alt_gui: false,
    };
    let latest = StoredProfile {
        us_to_jis: true,
        caps_to_ctrl: true,
        swap_alt_gui: false,
    };
    let mut keymap = Keymap::new();
    keymap
        .set_rule(KeymapRule::new(0x04, false, 0x05, false))
        .unwrap();

    updates.stage_profile(first);
    updates.stage_profile(latest);
    updates.stage_keymap(keymap);

    assert_eq!(updates.take_profile(), Some(latest));
    assert_eq!(updates.take_keymap(), Some(keymap));
    assert!(!updates.has_global_updates());
}

#[test]
fn source_updates_are_independent_and_coalesce_per_slot() {
    let mut updates = ConfigurationUpdates::new();

    assert!(updates.stage_source_profile(0, BridgeProfile::NONE));
    assert!(updates.stage_source_profile(1, BridgeProfile::US_JIS));
    assert!(updates.stage_source_profile(1, BridgeProfile::US_JIS_PRESET));
    assert!(!updates.stage_source_profile(5, BridgeProfile::NONE));

    assert_eq!(updates.take_source_profile(0), Some(BridgeProfile::NONE));
    assert_eq!(
        updates.take_source_profile(1),
        Some(BridgeProfile::US_JIS_PRESET)
    );
    assert_eq!(updates.take_source_profile(2), None);
    assert!(!updates.has_source_updates());
}
