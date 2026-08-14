//! Host-side contract tests for the first keymap transfer payload.

use ukf_core::{Keymap, KeymapRule};
use ukf_nrf52840_ble_usb::keymap_store::{
    KEYMAP_PAYLOAD_VERSION, KEYMAP_RECORD_LEN, KEYMAP_RULE_WIRE_LEN, KeymapPayloadError,
    KeymapRecordError, decode_keymap_payload, decode_keymap_record, encode_keymap_payload,
    encode_keymap_record,
};

#[test]
fn the_default_keymap_has_a_stable_round_trip() {
    let encoded = encode_keymap_payload(&Keymap::US_JIS);

    assert_eq!(encoded.as_slice()[0], KEYMAP_PAYLOAD_VERSION);
    assert_eq!(encoded.as_slice()[1], Keymap::US_JIS.len() as u8);
    assert_eq!(
        encoded.len(),
        2 + Keymap::US_JIS.len() * KEYMAP_RULE_WIRE_LEN
    );
    assert_eq!(
        decode_keymap_payload(encoded.as_slice()),
        Ok(Keymap::US_JIS)
    );
}

#[test]
fn a_custom_keymap_survives_wire_encoding_and_decoding() {
    let mut keymap = Keymap::new();
    keymap
        .set_rule(KeymapRule::new(0x04, false, 0x05, false))
        .unwrap();
    keymap
        .set_rule(KeymapRule::new(0x1f, true, 0x2f, false))
        .unwrap();

    let encoded = encode_keymap_payload(&keymap);

    assert_eq!(decode_keymap_payload(encoded.as_slice()), Ok(keymap));
}

#[test]
fn malformed_keymap_payloads_are_rejected_before_core_application() {
    assert_eq!(
        decode_keymap_payload(&[KEYMAP_PAYLOAD_VERSION]),
        Err(KeymapPayloadError::Length(1))
    );
    assert_eq!(
        decode_keymap_payload(&[KEYMAP_PAYLOAD_VERSION + 1, 0]),
        Err(KeymapPayloadError::Version(KEYMAP_PAYLOAD_VERSION + 1))
    );
    assert_eq!(
        decode_keymap_payload(&[KEYMAP_PAYLOAD_VERSION, 1]),
        Err(KeymapPayloadError::Length(2))
    );
    assert_eq!(
        decode_keymap_payload(&[KEYMAP_PAYLOAD_VERSION, 1, 0x04, 0x04, 0x05, 0]),
        Err(KeymapPayloadError::UnknownFlags(0x04))
    );
    assert_eq!(
        decode_keymap_payload(&[KEYMAP_PAYLOAD_VERSION, 1, 0x04, 0, 0x05, 1]),
        Err(KeymapPayloadError::Reserved(1))
    );
    assert_eq!(
        decode_keymap_payload(&[KEYMAP_PAYLOAD_VERSION, 1, 0, 0, 0x05, 0]),
        Err(KeymapPayloadError::Keymap(
            ukf_core::KeymapError::InvalidInputUsage
        ))
    );
}

#[test]
fn duplicate_input_rules_are_rejected() {
    let payload = [
        KEYMAP_PAYLOAD_VERSION,
        2,
        0x04,
        0,
        0x05,
        0,
        0x04,
        0,
        0x06,
        0,
    ];

    assert_eq!(
        decode_keymap_payload(&payload),
        Err(KeymapPayloadError::DuplicateRule {
            input_usage: 0x04,
            input_shifted: false,
        })
    );
}

#[test]
fn a_persisted_keymap_record_round_trips_and_rejects_corruption() {
    let mut keymap = Keymap::new();
    keymap
        .set_rule(KeymapRule::new(0x04, false, 0x05, false))
        .unwrap();
    let record = encode_keymap_record(&keymap);

    assert_eq!(record.len(), KEYMAP_RECORD_LEN);
    assert_eq!(decode_keymap_record(&record), Ok(keymap));

    let mut corrupt = record;
    corrupt[12] ^= 1;
    assert!(matches!(
        decode_keymap_record(&corrupt),
        Err(KeymapRecordError::Crc { .. })
    ));
}

#[test]
fn an_erased_keymap_page_is_not_a_valid_record() {
    assert_eq!(
        decode_keymap_record(&[0xff; KEYMAP_RECORD_LEN]),
        Err(KeymapRecordError::Empty)
    );
}
