//! TDD acceptance tests for the bridge's platform-independent behavior.

use ukf_core::{
    BootKeyboardReport, BridgeEngine, BridgeProfile, InputTransport, KEYMAP_RULE_CAPACITY, Keymap,
    KeymapError, KeymapRule, SourceId,
};

fn report(modifiers: u8, keys: [u8; 6]) -> BootKeyboardReport {
    BootKeyboardReport { modifiers, keys }
}

#[test]
fn us_jis_preset_maps_at_to_unshifted_left_bracket() {
    let mut bridge = BridgeEngine::new();
    bridge
        .attach(
            SourceId(0),
            InputTransport::Ble,
            BridgeProfile {
                us_to_jis: true,
                ..BridgeProfile::NONE
            },
        )
        .unwrap();
    let output = bridge
        .submit_boot_report(SourceId(0), report(0b10, [0x1f, 0, 0, 0, 0, 0]))
        .unwrap();
    assert_eq!(output.report, report(0, [0x2f, 0, 0, 0, 0, 0]));
}

#[test]
fn compatibility_preset_turns_caps_into_control_and_swaps_alt_gui() {
    let mut bridge = BridgeEngine::new();
    bridge
        .attach(
            SourceId(0),
            InputTransport::Ble,
            BridgeProfile::US_JIS_PRESET,
        )
        .unwrap();
    let output = bridge
        .submit_boot_report(SourceId(0), report(0b0000_0100, [0x39, 0, 0, 0, 0, 0]))
        .unwrap();
    assert_eq!(output.report.modifiers, 0b0000_1001);
    assert_eq!(output.report.keys, [0; 6]);
}

#[test]
fn bridge_merges_usb_and_multiple_ble_keyboards() {
    let mut bridge = BridgeEngine::new();
    bridge
        .attach(SourceId(0), InputTransport::Usb, BridgeProfile::NONE)
        .unwrap();
    bridge
        .attach(SourceId(1), InputTransport::Ble, BridgeProfile::NONE)
        .unwrap();
    bridge
        .attach(SourceId(2), InputTransport::Ble, BridgeProfile::NONE)
        .unwrap();
    bridge
        .submit_boot_report(SourceId(0), report(0, [0x04, 0, 0, 0, 0, 0]))
        .unwrap();
    bridge
        .submit_boot_report(SourceId(1), report(0, [0x05, 0, 0, 0, 0, 0]))
        .unwrap();
    let output = bridge
        .submit_boot_report(SourceId(2), report(0b10, [0x06, 0, 0, 0, 0, 0]))
        .unwrap();
    assert_eq!(output.report, report(0b10, [0x04, 0x05, 0x06, 0, 0, 0]));
}

#[test]
fn detaching_a_keyboard_releases_only_its_keys() {
    let mut bridge = BridgeEngine::new();
    bridge
        .attach(SourceId(0), InputTransport::Usb, BridgeProfile::NONE)
        .unwrap();
    bridge
        .attach(SourceId(1), InputTransport::Ble, BridgeProfile::NONE)
        .unwrap();
    bridge
        .submit_boot_report(SourceId(0), report(0, [0x04, 0, 0, 0, 0, 0]))
        .unwrap();
    bridge
        .submit_boot_report(SourceId(1), report(0, [0x05, 0, 0, 0, 0, 0]))
        .unwrap();
    let output = bridge.detach(SourceId(0)).unwrap();
    assert_eq!(output.report, report(0, [0x05, 0, 0, 0, 0, 0]));
}

#[test]
fn more_than_six_distinct_keys_reports_rollover_without_panicking() {
    let mut bridge = BridgeEngine::new();
    for source in 0..4 {
        bridge
            .attach(SourceId(source), InputTransport::Ble, BridgeProfile::NONE)
            .unwrap();
    }
    bridge
        .submit_boot_report(SourceId(0), report(0, [1, 2, 3, 4, 5, 6]))
        .unwrap();
    let output = bridge
        .submit_boot_report(SourceId(1), report(0, [7, 0, 0, 0, 0, 0]))
        .unwrap();
    assert!(output.rollover);
    assert_eq!(output.report.keys, [1, 2, 3, 4, 5, 6]);
}

#[test]
fn incompatible_shift_targets_in_one_report_preserve_the_original_chord() {
    let mut bridge = BridgeEngine::new();
    bridge
        .attach(
            SourceId(0),
            InputTransport::Ble,
            BridgeProfile {
                us_to_jis: true,
                ..BridgeProfile::NONE
            },
        )
        .unwrap();
    let source = report(0b10, [0x35, 0x1f, 0, 0, 0, 0]);
    let output = bridge.submit_boot_report(SourceId(0), source).unwrap();
    assert_eq!(output.report, source);
}

#[test]
fn shortcut_modifiers_preserve_original_us_usage() {
    let mut bridge = BridgeEngine::new();
    bridge
        .attach(
            SourceId(0),
            InputTransport::Ble,
            BridgeProfile {
                us_to_jis: true,
                ..BridgeProfile::NONE
            },
        )
        .unwrap();
    let source = report(0b1, [0x35, 0, 0, 0, 0, 0]);
    let output = bridge.submit_boot_report(SourceId(0), source).unwrap();
    assert_eq!(output.report, source);
}

#[test]
fn simultaneous_sources_do_not_apply_a_source_local_shift_conversion() {
    let mut bridge = BridgeEngine::new();
    bridge
        .attach(
            SourceId(0),
            InputTransport::Usb,
            BridgeProfile {
                us_to_jis: true,
                ..BridgeProfile::NONE
            },
        )
        .unwrap();
    bridge
        .attach(SourceId(1), InputTransport::Ble, BridgeProfile::NONE)
        .unwrap();
    bridge
        .submit_boot_report(SourceId(0), report(0b10, [0x1f, 0, 0, 0, 0, 0]))
        .unwrap();
    let output = bridge
        .submit_boot_report(SourceId(1), report(0, [0x04, 0, 0, 0, 0, 0]))
        .unwrap();
    assert_eq!(output.report, report(0b10, [0x1f, 0x04, 0, 0, 0, 0]));
}

#[test]
fn custom_keymap_rule_replaces_the_default_layout_mapping() {
    let mut bridge = BridgeEngine::new();
    let mut keymap = Keymap::US_JIS;
    keymap
        .set_rule(KeymapRule::new(0x1f, true, 0x04, false))
        .unwrap();
    bridge.set_keymap(keymap);
    bridge
        .attach(
            SourceId(0),
            InputTransport::Ble,
            BridgeProfile {
                us_to_jis: true,
                ..BridgeProfile::NONE
            },
        )
        .unwrap();

    let output = bridge
        .submit_boot_report(SourceId(0), report(0b10, [0x1f, 0, 0, 0, 0, 0]))
        .unwrap();

    assert_eq!(output.report, report(0, [0x04, 0, 0, 0, 0, 0]));
}

#[test]
fn keymap_rule_can_request_a_synthesized_shift() {
    let mut bridge = BridgeEngine::new();
    let mut keymap = Keymap::new();
    keymap
        .set_rule(KeymapRule::new(0x04, false, 0x1e, true))
        .unwrap();
    bridge.set_keymap(keymap);
    bridge
        .attach(
            SourceId(0),
            InputTransport::Ble,
            BridgeProfile {
                us_to_jis: true,
                ..BridgeProfile::NONE
            },
        )
        .unwrap();

    let output = bridge
        .submit_boot_report(SourceId(0), report(0, [0x04, 0, 0, 0, 0, 0]))
        .unwrap();

    assert_eq!(output.report, report(0b10, [0x1e, 0, 0, 0, 0, 0]));
}

#[test]
fn keymap_rejects_an_empty_input_and_overflow_without_partial_rules() {
    let mut keymap = Keymap::new();
    assert_eq!(
        keymap.set_rule(KeymapRule::new(0, false, 0x04, false)),
        Err(KeymapError::InvalidInputUsage)
    );

    for usage in 1..=KEYMAP_RULE_CAPACITY as u8 {
        keymap
            .set_rule(KeymapRule::new(usage, false, usage, false))
            .unwrap();
    }
    assert_eq!(keymap.len(), KEYMAP_RULE_CAPACITY);
    assert_eq!(
        keymap.set_rule(KeymapRule::new(0x7f, false, 0x04, false)),
        Err(KeymapError::Full)
    );
    assert_eq!(keymap.len(), KEYMAP_RULE_CAPACITY);
}

#[test]
fn each_source_uses_its_own_keymap() {
    let mut bridge = BridgeEngine::new();
    for source in [SourceId(0), SourceId(1)] {
        bridge
            .attach(
                source,
                InputTransport::Ble,
                BridgeProfile {
                    us_to_jis: true,
                    ..BridgeProfile::NONE
                },
            )
            .unwrap();
    }

    let mut source_zero_map = Keymap::new();
    source_zero_map
        .set_rule(KeymapRule::new(0x04, false, 0x1e, false))
        .unwrap();
    let mut source_one_map = Keymap::new();
    source_one_map
        .set_rule(KeymapRule::new(0x04, false, 0x1f, false))
        .unwrap();
    bridge
        .set_source_keymap(SourceId(0), source_zero_map)
        .unwrap();
    bridge
        .set_source_keymap(SourceId(1), source_one_map)
        .unwrap();

    let source_zero = bridge
        .submit_boot_report(SourceId(0), report(0, [0x04, 0, 0, 0, 0, 0]))
        .unwrap();
    bridge.detach(SourceId(0)).unwrap();
    let source_one = bridge
        .submit_boot_report(SourceId(1), report(0, [0x04, 0, 0, 0, 0, 0]))
        .unwrap();

    assert_eq!(source_zero.report, report(0, [0x1e, 0, 0, 0, 0, 0]));
    assert_eq!(source_one.report, report(0, [0x1f, 0, 0, 0, 0, 0]));
    assert_eq!(bridge.source_keymap(SourceId(0)).unwrap(), source_zero_map);
    assert_eq!(bridge.source_keymap(SourceId(1)).unwrap(), source_one_map);
}
