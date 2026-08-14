//! Versioned wire format for the first data-driven keymap.

use crate::uf2_reset::crc32_ieee;
use ukf_core::{KEYMAP_RULE_CAPACITY, Keymap, KeymapError, KeymapRule};

/// Version of the fixed single-layer keymap payload.
pub const KEYMAP_PAYLOAD_VERSION: u8 = 1;
/// Bytes occupied by one rule in the payload.
pub const KEYMAP_RULE_WIRE_LEN: usize = 4;
/// Bytes before the first rule.
pub const KEYMAP_PAYLOAD_HEADER_LEN: usize = 2;
/// Largest payload accepted by this codec.
pub const KEYMAP_PAYLOAD_MAX_LEN: usize =
    KEYMAP_PAYLOAD_HEADER_LEN + KEYMAP_RULE_CAPACITY * KEYMAP_RULE_WIRE_LEN;
/// Fixed record size used by the dedicated keymap flash page.
pub const KEYMAP_RECORD_LEN: usize = 160;

const INPUT_SHIFTED: u8 = 1 << 0;
const OUTPUT_SHIFTED: u8 = 1 << 1;
const RULE_FLAGS_MASK: u8 = INPUT_SHIFTED | OUTPUT_SHIFTED;
const KEYMAP_MAGIC: u32 = 0x554b_464b;
const KEYMAP_RECORD_VERSION: u8 = 1;
const KEYMAP_RECORD_PAYLOAD_OFFSET: usize = 8;
const KEYMAP_RECORD_CRC_OFFSET: usize = KEYMAP_RECORD_LEN - size_of::<u32>();

/// An encoded keymap with a fixed backing array and an explicit wire length.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncodedKeymap {
    bytes: [u8; KEYMAP_PAYLOAD_MAX_LEN],
    len: u8,
}

impl EncodedKeymap {
    /// Returns the meaningful bytes to send through the configuration transfer.
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    /// Returns the meaningful wire length.
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Returns whether the encoded payload has no meaningful bytes.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Why a keymap payload was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeymapPayloadError {
    /// The payload length does not match its declared rule count.
    Length(usize),
    /// The payload version is not understood.
    Version(u8),
    /// The rule count exceeds the fixed core capacity.
    RuleCount(u8),
    /// A rule's reserved byte was not zero.
    Reserved(u8),
    /// A rule carried a flag this version does not understand.
    UnknownFlags(u8),
    /// A source usage was listed more than once.
    DuplicateRule {
        /// Duplicated source usage.
        input_usage: u8,
        /// Shift state attached to the duplicated source usage.
        input_shifted: bool,
    },
    /// A rule could not be represented by the core keymap.
    Keymap(KeymapError),
}

/// Why a persisted keymap record was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeymapRecordError {
    /// The record is not exactly one fixed-size record.
    Length(usize),
    /// The page has no keymap record yet.
    Empty,
    /// The record version is not understood.
    Version(u8),
    /// A reserved record byte was not zero.
    Reserved,
    /// The record CRC does not match its contents.
    Crc {
        /// CRC stored in the record.
        expected: u32,
        /// CRC computed over the record body.
        actual: u32,
    },
    /// The embedded payload length is outside the fixed record.
    PayloadLength(u8),
    /// The embedded payload is invalid.
    Payload(KeymapPayloadError),
}

/// Encodes a keymap into the versioned transfer payload.
pub fn encode_keymap_payload(keymap: &Keymap) -> EncodedKeymap {
    let mut encoded = EncodedKeymap {
        bytes: [0; KEYMAP_PAYLOAD_MAX_LEN],
        len: (KEYMAP_PAYLOAD_HEADER_LEN + keymap.len() * KEYMAP_RULE_WIRE_LEN) as u8,
    };
    encoded.bytes[0] = KEYMAP_PAYLOAD_VERSION;
    encoded.bytes[1] = keymap.len() as u8;
    for index in 0..keymap.len() {
        let rule = keymap
            .rule(index)
            .expect("keymap length guarantees every encoded rule exists");
        let offset = KEYMAP_PAYLOAD_HEADER_LEN + index * KEYMAP_RULE_WIRE_LEN;
        encoded.bytes[offset] = rule.input_usage;
        encoded.bytes[offset + 1] =
            u8::from(rule.input_shifted) | (u8::from(rule.output_shifted) << 1);
        encoded.bytes[offset + 2] = rule.output_usage;
        // offset + 3 is reserved and remains zero.
    }
    encoded
}

/// Decodes and validates a complete keymap transfer payload.
pub fn decode_keymap_payload(bytes: &[u8]) -> Result<Keymap, KeymapPayloadError> {
    if bytes.len() < KEYMAP_PAYLOAD_HEADER_LEN {
        return Err(KeymapPayloadError::Length(bytes.len()));
    }
    if bytes[0] != KEYMAP_PAYLOAD_VERSION {
        return Err(KeymapPayloadError::Version(bytes[0]));
    }
    let rule_count = usize::from(bytes[1]);
    if rule_count > KEYMAP_RULE_CAPACITY {
        return Err(KeymapPayloadError::RuleCount(bytes[1]));
    }
    let expected_len = KEYMAP_PAYLOAD_HEADER_LEN + rule_count * KEYMAP_RULE_WIRE_LEN;
    if bytes.len() != expected_len {
        return Err(KeymapPayloadError::Length(bytes.len()));
    }

    let mut keymap = Keymap::new();
    for index in 0..rule_count {
        let offset = KEYMAP_PAYLOAD_HEADER_LEN + index * KEYMAP_RULE_WIRE_LEN;
        let input_usage = bytes[offset];
        let flags = bytes[offset + 1];
        if flags & !RULE_FLAGS_MASK != 0 {
            return Err(KeymapPayloadError::UnknownFlags(flags));
        }
        if bytes[offset + 3] != 0 {
            return Err(KeymapPayloadError::Reserved(bytes[offset + 3]));
        }
        let input_shifted = flags & INPUT_SHIFTED != 0;
        if keymap.contains_rule(input_usage, input_shifted) {
            return Err(KeymapPayloadError::DuplicateRule {
                input_usage,
                input_shifted,
            });
        }
        keymap
            .set_rule(KeymapRule::new(
                input_usage,
                input_shifted,
                bytes[offset + 2],
                flags & OUTPUT_SHIFTED != 0,
            ))
            .map_err(KeymapPayloadError::Keymap)?;
    }
    Ok(keymap)
}

/// Encodes a keymap into the fixed-size record written to the dedicated page.
pub fn encode_keymap_record(keymap: &Keymap) -> [u8; KEYMAP_RECORD_LEN] {
    let payload = encode_keymap_payload(keymap);
    let mut bytes = [0; KEYMAP_RECORD_LEN];
    bytes[0..4].copy_from_slice(&KEYMAP_MAGIC.to_le_bytes());
    bytes[4] = KEYMAP_RECORD_VERSION;
    bytes[5] = payload.len;
    bytes[KEYMAP_RECORD_PAYLOAD_OFFSET..][..payload.len()].copy_from_slice(payload.as_slice());
    let crc = crc32_ieee(&bytes[..KEYMAP_RECORD_CRC_OFFSET]);
    bytes[KEYMAP_RECORD_CRC_OFFSET..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

/// Decodes a fixed-size keymap record and its embedded versioned payload.
pub fn decode_keymap_record(bytes: &[u8]) -> Result<Keymap, KeymapRecordError> {
    if bytes.len() != KEYMAP_RECORD_LEN {
        return Err(KeymapRecordError::Length(bytes.len()));
    }
    let magic = u32::from_le_bytes(
        bytes[0..4]
            .try_into()
            .expect("a four-byte magic was just sliced from a longer record"),
    );
    if magic != KEYMAP_MAGIC {
        return Err(KeymapRecordError::Empty);
    }
    if bytes[4] != KEYMAP_RECORD_VERSION {
        return Err(KeymapRecordError::Version(bytes[4]));
    }
    let expected = u32::from_le_bytes(
        bytes[KEYMAP_RECORD_CRC_OFFSET..]
            .try_into()
            .expect("a four-byte CRC was just sliced from a longer record"),
    );
    let actual = crc32_ieee(&bytes[..KEYMAP_RECORD_CRC_OFFSET]);
    if expected != actual {
        return Err(KeymapRecordError::Crc { expected, actual });
    }

    let payload_len = usize::from(bytes[5]);
    if payload_len > KEYMAP_PAYLOAD_MAX_LEN
        || KEYMAP_RECORD_PAYLOAD_OFFSET + payload_len > KEYMAP_RECORD_CRC_OFFSET
    {
        return Err(KeymapRecordError::PayloadLength(bytes[5]));
    }
    if bytes[6..KEYMAP_RECORD_PAYLOAD_OFFSET]
        .iter()
        .any(|byte| *byte != 0)
        || bytes[KEYMAP_RECORD_PAYLOAD_OFFSET + payload_len..KEYMAP_RECORD_CRC_OFFSET]
            .iter()
            .any(|byte| *byte != 0)
    {
        return Err(KeymapRecordError::Reserved);
    }
    decode_keymap_payload(
        &bytes[KEYMAP_RECORD_PAYLOAD_OFFSET..KEYMAP_RECORD_PAYLOAD_OFFSET + payload_len],
    )
    .map_err(KeymapRecordError::Payload)
}
