//! Fixed-byte tests for the 0.5 source diagnostics block.

use ukf_core::{
    BridgeEngine, BridgeProfile, InputTransport, SourceIdentity, SourceName, SourceState,
};
use ukf_nrf52840_ble_usb::diagnostics_report::{
    SOURCE_KIND, SourceReport, decode_source_block, encode_source_block,
};
use ukf_nrf52840_ble_usb::uf2_reset::crc32_ieee;

#[test]
fn virtual_source_block_has_explicit_slot_transport_state_profile_name_and_irk_flag() {
    let mut bridge = BridgeEngine::new();
    bridge
        .register_source(
            ukf_core::VIRTUAL_SOURCE_ID,
            InputTransport::Virtual,
            SourceIdentity::NONE,
            SourceName::try_from_bytes(b"Virtual Input").unwrap(),
            None,
            BridgeProfile {
                us_to_jis: true,
                caps_to_ctrl: false,
                swap_alt_gui: true,
            },
        )
        .unwrap();
    bridge
        .attach(
            ukf_core::VIRTUAL_SOURCE_ID,
            InputTransport::Virtual,
            BridgeProfile {
                us_to_jis: true,
                caps_to_ctrl: false,
                swap_alt_gui: true,
            },
        )
        .unwrap();

    let block = encode_source_block(&SourceReport::from_snapshot(
        &bridge.source_snapshot(ukf_core::VIRTUAL_SOURCE_ID).unwrap(),
    ));

    assert_eq!(
        block,
        [
            0x12, 0x07, 0x01, 0x04, 0x03, 0x05, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x0d, 0x56, 0x69, 0x72, 0x74, 0x75, 0x61, 0x6c, 0x20, 0x49, 0x6e, 0x70, 0x75, 0x74,
            0xcb, 0x7c, 0xa5, 0xd8,
        ]
    );
    assert_eq!(block[1], SOURCE_KIND);
    assert_eq!(decode_source_block(&block).unwrap().slot, 4);
}

#[test]
fn a_registered_ble_block_carries_identity_and_irk_presence_but_not_irk_bytes() {
    let report = SourceReport {
        slot: 0,
        transport: Some(InputTransport::Ble),
        state: SourceState::Connected,
        identity: SourceIdentity::from_address([1, 2, 3, 4, 5, 6], true),
        profile: BridgeProfile::US_JIS,
        name: SourceName::try_from_bytes(b"Keyboard A").unwrap(),
    };
    let block = encode_source_block(&report);

    assert_eq!(&block[8..14], &[1, 2, 3, 4, 5, 6]);
    assert_eq!(block[6], 0b11);
    assert_eq!(block[7], 0b001);
    assert!(block[15..28].contains(&b'K'));
    assert!(!block.windows(16).any(|window| window == [0xa5; 16]));
    assert_eq!(
        u32::from_le_bytes(block[28..32].try_into().unwrap()),
        crc32_ieee(&block[..28])
    );
}

#[test]
fn source_block_rejects_unknown_slot_reserved_bytes_and_bad_crc() {
    let report = SourceReport {
        slot: 4,
        transport: Some(InputTransport::Virtual),
        state: SourceState::Connected,
        identity: SourceIdentity::NONE,
        profile: BridgeProfile::NONE,
        name: SourceName::try_from_bytes(b"Virtual Input").unwrap(),
    };
    let valid = encode_source_block(&report);

    let mut unknown_slot = valid;
    unknown_slot[3] = 5;
    let unknown_slot_crc = crc32_ieee(&unknown_slot[..28]);
    unknown_slot[28..].copy_from_slice(&unknown_slot_crc.to_le_bytes());
    assert!(decode_source_block(&unknown_slot).is_err());

    let mut reserved = valid;
    reserved[6] = 0x80;
    let reserved_crc = crc32_ieee(&reserved[..28]);
    reserved[28..].copy_from_slice(&reserved_crc.to_le_bytes());
    assert!(decode_source_block(&reserved).is_err());

    let mut irk_without_address = valid;
    irk_without_address[6] = 0x02;
    let irk_crc = crc32_ieee(&irk_without_address[..28]);
    irk_without_address[28..].copy_from_slice(&irk_crc.to_le_bytes());
    assert_eq!(
        decode_source_block(&irk_without_address),
        Err(ukf_nrf52840_ble_usb::diagnostics_report::SourceReportError::IdentityKeyWithoutAddress)
    );

    let mut corrupt = valid;
    corrupt[7] ^= 1;
    assert!(decode_source_block(&corrupt).is_err());
}

#[test]
fn source_name_and_diagnostics_reject_invalid_utf8() {
    assert!(SourceName::try_from_bytes(&[0xff]).is_err());

    let report = SourceReport {
        slot: 4,
        transport: Some(InputTransport::Virtual),
        state: SourceState::Connected,
        identity: SourceIdentity::NONE,
        profile: BridgeProfile::NONE,
        name: SourceName::from_raw([0xff; ukf_core::SOURCE_NAME_LEN], 1),
    };
    let block = encode_source_block(&report);

    assert_eq!(
        decode_source_block(&block),
        Err(ukf_nrf52840_ble_usb::diagnostics_report::SourceReportError::NameEncoding)
    );
}
