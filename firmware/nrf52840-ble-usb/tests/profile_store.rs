//! Host tests for the persistent compatibility profile record and payload.

use ukf_core::BridgeProfile;
use ukf_nrf52840_ble_usb::profile_store::{
    PROFILE_RECORD_LEN, ProfileRecordError, StoredProfile, decode_profile, decode_profile_payload,
    decode_source_profile_payload, encode_profile, encode_profile_payload,
    encode_source_profile_payload,
};
use ukf_nrf52840_ble_usb::uf2_reset::crc32_ieee;

fn profile_from_flags(flags: u8) -> StoredProfile {
    StoredProfile {
        us_to_jis: flags & 0x01 != 0,
        caps_to_ctrl: flags & 0x02 != 0,
        swap_alt_gui: flags & 0x04 != 0,
    }
}

#[test]
fn every_profile_flag_combination_survives_a_record_round_trip() {
    for flags in 0..=0x07 {
        let profile = profile_from_flags(flags);

        assert_eq!(decode_profile(&encode_profile(&profile)), Ok(profile));
    }
}

#[test]
fn erased_profile_slot_is_empty() {
    let erased = [0xff; PROFILE_RECORD_LEN];

    assert_eq!(decode_profile(&erased), Err(ProfileRecordError::Empty));
}

#[test]
fn profile_record_version_is_rejected() {
    let profile = StoredProfile::from(BridgeProfile::NONE);
    let mut record = encode_profile(&profile);
    record[4] = 2;

    assert_eq!(decode_profile(&record), Err(ProfileRecordError::Version(2)));
}

#[test]
fn one_bit_profile_record_corruption_is_rejected_by_crc() {
    let profile = StoredProfile::from(BridgeProfile::US_JIS_PRESET);
    let mut record = encode_profile(&profile);
    record[5] ^= 0x01;

    let expected = u32::from_le_bytes(record[12..16].try_into().expect("four CRC bytes"));
    let actual = ukf_nrf52840_ble_usb::uf2_reset::crc32_ieee(&record[..12]);

    assert_eq!(
        decode_profile(&record),
        Err(ProfileRecordError::Crc { expected, actual })
    );
}

#[test]
fn every_profile_flag_combination_survives_a_payload_round_trip() {
    for flags in 0..=0x07 {
        let profile = profile_from_flags(flags);
        let payload = encode_profile_payload(&profile);

        assert_eq!(payload, [1, flags]);
        assert_eq!(decode_profile_payload(&payload), Ok(profile));
    }
}

#[test]
fn profile_payload_length_version_and_undefined_bits_are_rejected() {
    assert_eq!(
        decode_profile_payload(&[1]),
        Err(ProfileRecordError::Length(1))
    );
    assert_eq!(
        decode_profile_payload(&[1, 0, 0]),
        Err(ProfileRecordError::Length(3))
    );
    assert_eq!(
        decode_profile_payload(&[2, 0]),
        Err(ProfileRecordError::Version(2))
    );
    // Named apart from `Empty` on purpose. "Nobody has written a profile yet"
    // is answered by falling back to the default; "the sender set a bit this
    // build cannot name" must not be, because the fallback would quietly change
    // what every keystroke becomes.
    assert_eq!(
        decode_profile_payload(&[1, 0x08]),
        Err(ProfileRecordError::UnknownFlags(0x08))
    );
}

#[test]
fn a_record_with_a_dirty_reserved_byte_is_not_reported_as_never_written() {
    // Reserved bytes are zero in everything this build writes, so a non-zero
    // one means the record came from somewhere else. Answering that with the
    // default would adopt a profile nobody chose.
    let mut record = encode_profile(&StoredProfile {
        us_to_jis: true,
        caps_to_ctrl: false,
        swap_alt_gui: false,
    });
    record[6] = 1;
    let crc = crc32_ieee(&record[..12]);
    record[12..].copy_from_slice(&crc.to_le_bytes());

    assert_eq!(decode_profile(&record), Err(ProfileRecordError::Reserved));
}

#[test]
fn stored_profile_and_bridge_profile_convert_in_both_directions() {
    let bridge = BridgeProfile {
        us_to_jis: true,
        caps_to_ctrl: false,
        swap_alt_gui: true,
    };
    let stored = StoredProfile::from(bridge);

    assert_eq!(
        stored,
        StoredProfile {
            us_to_jis: true,
            caps_to_ctrl: false,
            swap_alt_gui: true,
        }
    );
    assert_eq!(BridgeProfile::from(stored), bridge);
}

#[test]
fn source_profile_payload_is_versioned_length_delimited_and_slot_specific() {
    let profile = StoredProfile::from(BridgeProfile::US_JIS_PRESET);
    let payload = encode_source_profile_payload(4, &profile);

    assert_eq!(payload, [2, 4, 4, 7]);
    assert_eq!(decode_source_profile_payload(&payload), Ok((4, profile)));
}

#[test]
fn source_profile_payload_rejects_ambiguous_version_length_and_unknown_slot() {
    assert_eq!(
        decode_source_profile_payload(&[1, 2, 4, 7]),
        Err(ProfileRecordError::Version(1))
    );
    assert_eq!(
        decode_source_profile_payload(&[2, 3, 4, 7]),
        Err(ProfileRecordError::PayloadLength(3))
    );
    assert_eq!(
        decode_source_profile_payload(&[2, 4, 5, 7]),
        Err(ProfileRecordError::SourceSlot(5))
    );
}
