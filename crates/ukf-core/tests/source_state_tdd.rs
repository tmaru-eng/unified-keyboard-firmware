//! TDD acceptance tests for source registration, lifecycle, and ownership.

use ukf_core::{
    BootKeyboardReport, BridgeEngine, BridgeProfile, InputTransport, SOURCE_SLOT_COUNT, SourceId,
    SourceIdentity, SourceName, SourceState, VIRTUAL_SOURCE_ID,
};

fn report(modifiers: u8, keys: [u8; 6]) -> BootKeyboardReport {
    BootKeyboardReport { modifiers, keys }
}

#[test]
fn the_registered_slots_are_four_physical_slots_plus_virtual_slot_four() {
    assert_eq!(SOURCE_SLOT_COUNT, 5);
    assert_eq!(VIRTUAL_SOURCE_ID, SourceId(4));
    assert_eq!(SourceId(4).index(), Some(4));
    assert_eq!(SourceId(5).index(), None);
}

#[test]
fn snapshots_expose_identity_state_name_profile_and_only_irk_presence() {
    let mut bridge = BridgeEngine::new();
    let name = SourceName::try_from_bytes(b"Keyboard A").expect("short fixed name fits");
    bridge
        .register_source(
            SourceId(0),
            InputTransport::Ble,
            SourceIdentity::from_address([1, 2, 3, 4, 5, 6], true),
            name,
            Some([0xa5; 16]),
            BridgeProfile::US_JIS,
        )
        .expect("slot zero is registered");

    let snapshot = bridge.source_snapshot(SourceId(0)).expect("slot exists");
    assert_eq!(snapshot.slot, SourceId(0));
    assert_eq!(snapshot.transport, Some(InputTransport::Ble));
    assert_eq!(snapshot.state, SourceState::Disconnected);
    assert_eq!(snapshot.identity.address, Some([1, 2, 3, 4, 5, 6]));
    assert!(snapshot.identity.irk_present);
    assert_eq!(snapshot.name.as_bytes(), b"Keyboard A");
    assert_eq!(snapshot.profile, BridgeProfile::US_JIS);
}

#[test]
fn debug_output_redacts_private_irk_bytes() {
    let mut bridge = BridgeEngine::new();
    bridge
        .register_source(
            SourceId(0),
            InputTransport::Ble,
            SourceIdentity::from_address([1, 2, 3, 4, 5, 6], true),
            SourceName::try_from_bytes(b"Keyboard A").unwrap(),
            Some([0xa5; 16]),
            BridgeProfile::NONE,
        )
        .unwrap();

    let rendered = format!("{bridge:?}");

    assert!(rendered.contains("irk_present: true"));
    assert!(!rendered.contains("irk:"));
    assert!(!rendered.contains("165, 165, 165"));
}

#[test]
fn detaching_one_source_keeps_the_other_source_report_and_profile() {
    let mut bridge = BridgeEngine::new();
    bridge
        .attach(SourceId(0), InputTransport::Ble, BridgeProfile::NONE)
        .unwrap();
    bridge
        .attach(
            VIRTUAL_SOURCE_ID,
            InputTransport::Virtual,
            BridgeProfile::US_JIS,
        )
        .unwrap();

    bridge
        .submit_boot_report(SourceId(0), report(0, [0x04, 0, 0, 0, 0, 0]))
        .unwrap();
    bridge
        .submit_boot_report(VIRTUAL_SOURCE_ID, report(0, [0x05, 0, 0, 0, 0, 0]))
        .unwrap();

    let output = bridge.detach(SourceId(0)).unwrap();

    assert_eq!(output.report.keys, [0x05, 0, 0, 0, 0, 0]);
    assert_eq!(bridge.profile(VIRTUAL_SOURCE_ID), Ok(BridgeProfile::US_JIS));
    assert_eq!(
        bridge.source_snapshot(VIRTUAL_SOURCE_ID).unwrap().state,
        SourceState::Connected
    );
}

#[test]
fn a_profile_can_change_for_one_registered_source_without_touching_the_other() {
    let mut bridge = BridgeEngine::new();
    bridge
        .attach(SourceId(0), InputTransport::Ble, BridgeProfile::NONE)
        .unwrap();
    bridge
        .attach(
            VIRTUAL_SOURCE_ID,
            InputTransport::Virtual,
            BridgeProfile::NONE,
        )
        .unwrap();

    bridge
        .set_profile(SourceId(0), BridgeProfile::US_JIS)
        .expect("source zero profile is editable");

    assert_eq!(bridge.profile(SourceId(0)), Ok(BridgeProfile::US_JIS));
    assert_eq!(bridge.profile(VIRTUAL_SOURCE_ID), Ok(BridgeProfile::NONE));
}
