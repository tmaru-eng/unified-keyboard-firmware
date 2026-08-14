//! Persistent record of one pairing, so a bonded keyboard reconnects by itself.
//!
//! Pairing keys live in RAM inside the host stack and are gone at the next
//! reset. Every reset therefore made the bridge a stranger again, and the
//! keyboard would only speak to a stranger while its pairing button was held —
//! which turned every bring-up cycle into a trip to the keyboard.
//!
//! The record here is deliberately free of `trouble-host` types. Conversion to
//! and from `BondInformation` sits behind the hardware feature; the encoding
//! itself is plain data and is tested on the host, because a record that
//! decodes wrong is indistinguishable from a keyboard that refuses to connect.

use crate::uf2_reset::crc32_ieee;

/// Number of user-selectable BLE registration slots.
pub const BOND_SLOT_COUNT: usize = 4;
/// Maximum UTF-8/name bytes retained for one registered keyboard.
pub const BOND_NAME_LEN: usize = 13;

/// Wire size of one stored bond, chosen to be a multiple of the flash write
/// size on this part and to leave room for fields a later pairing mode needs.
pub const BOND_RECORD_LEN: usize = 64;

/// Wire size of one slot in the multi-bond page.
///
/// The legacy record remains 64 bytes so an existing 0.5 installation can be
/// decoded and migrated. The new record has a separate version and enough
/// reserved bytes to add non-secret metadata without moving the key fields.
pub const BOND_SLOT_RECORD_LEN: usize = 128;
/// Number of append-only records in the reserved 4 KiB bond page.
pub const BOND_PAGE_RECORD_COUNT: usize = 32;
/// Bytes read from and written to the reserved bond page.
///
/// A page is intentionally larger than the four logical slots. Each bond
/// mutation appends one 128-byte record, so a torn write leaves the previous
/// valid record available instead of erasing the only copy first.
pub const BOND_PAGE_DATA_LEN: usize = BOND_PAGE_RECORD_COUNT * BOND_SLOT_RECORD_LEN;

/// Marks a slot as written. Erased flash reads as `0xff`, which this is not.
const BOND_MAGIC: u32 = 0x554b_4642;
/// Incremented when the layout below changes, so an old record is ignored
/// rather than decoded into the wrong fields.
const BOND_VERSION: u8 = 1;
const BOND_SLOT_VERSION: u8 = 2;

const CRC_OFFSET: usize = BOND_RECORD_LEN - size_of::<u32>();
const SLOT_CRC_OFFSET: usize = BOND_SLOT_RECORD_LEN - size_of::<u32>();
const BOND_TOMBSTONE_NAME_LEN: u8 = u8::MAX;

/// One pairing, in the form it is written to flash.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct StoredBond {
    /// Long term key agreed during pairing.
    pub ltk: u128,
    /// HCI address kind of the peer's identity address.
    pub addr_kind: u8,
    /// Peer identity address, least significant byte first.
    pub addr: [u8; 6],
    /// Identity resolving key, present only when the peer sent one.
    pub irk: Option<u128>,
    /// Whether the pairing asked for the keys to be kept.
    pub is_bonded: bool,
    /// Security level the pairing reached.
    pub security_level: u8,
    /// Encrypted diversifier. Zero for secure connections, set for legacy.
    pub ediv: u16,
    /// Random number that goes with `ediv`. Zero for secure connections.
    pub rand: [u8; 8],
    /// Negotiated key length in bytes.
    pub encryption_key_len: u8,
}

/// One registered bond and its user-visible slot metadata.
///
/// The slot number is duplicated in the record so a misplaced or stale flash
/// write cannot silently make one keyboard appear under another slot.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct StoredBondSlot {
    /// Stable registration slot number, from zero through three.
    pub slot: u8,
    /// Pairing material for this keyboard.
    pub bond: StoredBond,
    /// UTF-8 bytes of the user-visible name.
    pub name: [u8; BOND_NAME_LEN],
    /// Number of meaningful bytes in [`Self::name`].
    pub name_len: u8,
}

impl core::fmt::Debug for StoredBondSlot {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("StoredBondSlot")
            .field("slot", &self.slot)
            .field("bond", &self.bond)
            .field("name", &self.name_bytes())
            .finish()
    }
}

impl StoredBondSlot {
    /// Builds a slot record after validating its fixed-capacity UTF-8 name.
    pub fn new(slot: u8, bond: StoredBond, name: &[u8]) -> Result<Self, BondSlotRecordError> {
        if usize::from(slot) >= BOND_SLOT_COUNT {
            return Err(BondSlotRecordError::Slot(slot));
        }
        if name.len() > BOND_NAME_LEN {
            return Err(BondSlotRecordError::NameLength(name.len()));
        }
        if core::str::from_utf8(name).is_err() {
            return Err(BondSlotRecordError::NameEncoding);
        }
        let mut bytes = [0; BOND_NAME_LEN];
        bytes[..name.len()].copy_from_slice(name);
        Ok(Self {
            slot,
            bond,
            name: bytes,
            name_len: name.len() as u8,
        })
    }

    /// Returns the meaningful UTF-8/name bytes.
    pub fn name_bytes(&self) -> &[u8] {
        &self.name[..usize::from(self.name_len)]
    }
}

/// Why a multi-bond record could not be decoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BondSlotRecordError {
    /// The slice was not exactly one slot record.
    Length(usize),
    /// The slot has never been written.
    Empty,
    /// The record was written by another layout.
    Version(u8),
    /// The record names a slot outside the four registered slots.
    Slot(u8),
    /// The name does not fit its fixed field.
    NameLength(usize),
    /// The name bytes are not valid UTF-8.
    NameEncoding,
    /// The record claims an IRK but contains the invalid all-zero value.
    InvalidIrk,
    /// Reserved bytes are not in their canonical zero state.
    Reserved,
    /// The CRC suffix does not match the record body.
    Crc {
        /// CRC stored in the record.
        expected: u32,
        /// CRC computed over the record.
        actual: u32,
    },
}

impl core::fmt::Debug for StoredBond {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("StoredBond")
            .field("addr_kind", &self.addr_kind)
            .field("addr", &self.addr)
            .field("irk_present", &self.irk.is_some())
            .field("is_bonded", &self.is_bonded)
            .field("security_level", &self.security_level)
            .field("encryption_key_len", &self.encryption_key_len)
            .finish()
    }
}

/// Why a stored bond could not be read back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BondRecordError {
    /// The slice was not exactly one record.
    Length(usize),
    /// The slot has never been written, or holds something else entirely.
    Empty,
    /// The record was written by a different layout.
    Version(u8),
    /// The record is corrupt.
    Crc {
        /// CRC stored in the record.
        expected: u32,
        /// CRC computed over the record.
        actual: u32,
    },
    /// The record claims an IRK but contains the invalid all-zero value.
    InvalidIrk,
}

/// Encodes one bond into its flash record.
pub fn encode_bond(bond: &StoredBond) -> [u8; BOND_RECORD_LEN] {
    let mut bytes = [0; BOND_RECORD_LEN];
    bytes[0..4].copy_from_slice(&BOND_MAGIC.to_le_bytes());
    bytes[4] = BOND_VERSION;
    bytes[5] = bond.addr_kind;
    bytes[6] = u8::from(bond.is_bonded);
    bytes[7] = bond.security_level;
    bytes[8..24].copy_from_slice(&bond.ltk.to_le_bytes());
    bytes[24..30].copy_from_slice(&bond.addr);
    bytes[30..32].copy_from_slice(&bond.ediv.to_le_bytes());
    bytes[32..40].copy_from_slice(&bond.rand);
    bytes[40] = bond.encryption_key_len;
    // The key is optional, and a key of all zeros is a legal value, so its
    // presence needs a flag of its own rather than being inferred from bytes.
    bytes[41] = u8::from(bond.irk.is_some());
    bytes[42..58].copy_from_slice(&bond.irk.unwrap_or(0).to_le_bytes());
    let crc = crc32_ieee(&bytes[..CRC_OFFSET]);
    bytes[CRC_OFFSET..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

/// Decodes one flash record, rejecting anything that is not a current one.
pub fn decode_bond(bytes: &[u8]) -> Result<StoredBond, BondRecordError> {
    if bytes.len() != BOND_RECORD_LEN {
        return Err(BondRecordError::Length(bytes.len()));
    }
    let magic = u32::from_le_bytes(
        bytes[0..4]
            .try_into()
            .expect("a four-byte magic was just sliced from a longer record"),
    );
    if magic != BOND_MAGIC {
        return Err(BondRecordError::Empty);
    }
    if bytes[4] != BOND_VERSION {
        return Err(BondRecordError::Version(bytes[4]));
    }

    let expected = u32::from_le_bytes(
        bytes[CRC_OFFSET..]
            .try_into()
            .expect("a four-byte CRC was just sliced from a longer record"),
    );
    let actual = crc32_ieee(&bytes[..CRC_OFFSET]);
    if expected != actual {
        return Err(BondRecordError::Crc { expected, actual });
    }

    let irk = if bytes[41] == 0 {
        None
    } else {
        let value = u128::from_le_bytes(
            bytes[42..58]
                .try_into()
                .expect("sixteen bytes of resolving key"),
        );
        if value == 0 {
            return Err(BondRecordError::InvalidIrk);
        }
        Some(value)
    };

    Ok(StoredBond {
        ltk: u128::from_le_bytes(bytes[8..24].try_into().expect("sixteen bytes of key")),
        addr_kind: bytes[5],
        addr: bytes[24..30].try_into().expect("six bytes of address"),
        irk,
        is_bonded: bytes[6] != 0,
        security_level: bytes[7],
        ediv: u16::from_le_bytes(bytes[30..32].try_into().expect("two bytes of diversifier")),
        rand: bytes[32..40].try_into().expect("eight bytes of random"),
        encryption_key_len: bytes[40],
    })
}

/// Encodes one slot of the current multi-bond layout.
pub fn encode_bond_slot(slot: &StoredBondSlot) -> [u8; BOND_SLOT_RECORD_LEN] {
    let mut bytes = [0; BOND_SLOT_RECORD_LEN];
    bytes[0..4].copy_from_slice(&BOND_MAGIC.to_le_bytes());
    bytes[4] = BOND_SLOT_VERSION;
    bytes[5] = slot.slot;
    bytes[6] = slot.name_len;
    bytes[7..7 + BOND_NAME_LEN].copy_from_slice(&slot.name);
    bytes[20] = slot.bond.addr_kind;
    bytes[21] = u8::from(slot.bond.is_bonded);
    bytes[22] = slot.bond.security_level;
    bytes[23..39].copy_from_slice(&slot.bond.ltk.to_le_bytes());
    bytes[39..45].copy_from_slice(&slot.bond.addr);
    bytes[45..47].copy_from_slice(&slot.bond.ediv.to_le_bytes());
    bytes[47..55].copy_from_slice(&slot.bond.rand);
    bytes[55] = slot.bond.encryption_key_len;
    bytes[56] = u8::from(slot.bond.irk.is_some());
    bytes[57..73].copy_from_slice(&slot.bond.irk.unwrap_or(0).to_le_bytes());
    let crc = crc32_ieee(&bytes[..SLOT_CRC_OFFSET]);
    bytes[SLOT_CRC_OFFSET..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

/// Decodes one slot of the current multi-bond layout.
pub fn decode_bond_slot(bytes: &[u8]) -> Result<StoredBondSlot, BondSlotRecordError> {
    if bytes.len() != BOND_SLOT_RECORD_LEN {
        return Err(BondSlotRecordError::Length(bytes.len()));
    }
    let magic = u32::from_le_bytes(
        bytes[0..4]
            .try_into()
            .expect("a four-byte magic was just sliced from a longer record"),
    );
    if magic != BOND_MAGIC {
        return Err(BondSlotRecordError::Empty);
    }
    if bytes[4] != BOND_SLOT_VERSION {
        return Err(BondSlotRecordError::Version(bytes[4]));
    }

    let expected = u32::from_le_bytes(
        bytes[SLOT_CRC_OFFSET..]
            .try_into()
            .expect("a four-byte CRC was just sliced from a longer record"),
    );
    let actual = crc32_ieee(&bytes[..SLOT_CRC_OFFSET]);
    if expected != actual {
        return Err(BondSlotRecordError::Crc { expected, actual });
    }

    let slot = bytes[5];
    if usize::from(slot) >= BOND_SLOT_COUNT {
        return Err(BondSlotRecordError::Slot(slot));
    }
    let name_len = usize::from(bytes[6]);
    if name_len > BOND_NAME_LEN {
        return Err(BondSlotRecordError::NameLength(name_len));
    }
    if core::str::from_utf8(&bytes[7..7 + name_len]).is_err() {
        return Err(BondSlotRecordError::NameEncoding);
    }
    if bytes[7 + name_len..7 + BOND_NAME_LEN]
        .iter()
        .any(|byte| *byte != 0)
        || bytes[73..SLOT_CRC_OFFSET].iter().any(|byte| *byte != 0)
    {
        return Err(BondSlotRecordError::Reserved);
    }

    let irk = if bytes[56] == 0 {
        None
    } else {
        let value = u128::from_le_bytes(
            bytes[57..73]
                .try_into()
                .expect("sixteen bytes of resolving key"),
        );
        if value == 0 {
            return Err(BondSlotRecordError::InvalidIrk);
        }
        Some(value)
    };

    let mut name = [0; BOND_NAME_LEN];
    name.copy_from_slice(&bytes[7..7 + BOND_NAME_LEN]);
    Ok(StoredBondSlot {
        slot,
        bond: StoredBond {
            ltk: u128::from_le_bytes(bytes[23..39].try_into().expect("sixteen bytes of key")),
            addr_kind: bytes[20],
            addr: bytes[39..45].try_into().expect("six bytes of address"),
            irk,
            is_bonded: bytes[21] != 0,
            security_level: bytes[22],
            ediv: u16::from_le_bytes(bytes[45..47].try_into().expect("two bytes of diversifier")),
            rand: bytes[47..55].try_into().expect("eight bytes of random"),
            encryption_key_len: bytes[55],
        },
        name,
        name_len: bytes[6],
    })
}

/// Encodes a deletion marker for one logical slot.
///
/// A missing record means "no update yet". Deletion therefore needs an
/// explicit marker so replaying an older record cannot resurrect a removed
/// keyboard.
pub fn encode_bond_tombstone(slot: u8) -> [u8; BOND_SLOT_RECORD_LEN] {
    let mut bytes = [0; BOND_SLOT_RECORD_LEN];
    bytes[0..4].copy_from_slice(&BOND_MAGIC.to_le_bytes());
    bytes[4] = BOND_SLOT_VERSION;
    bytes[5] = slot;
    bytes[6] = BOND_TOMBSTONE_NAME_LEN;
    let crc = crc32_ieee(&bytes[..SLOT_CRC_OFFSET]);
    bytes[SLOT_CRC_OFFSET..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

fn decode_bond_tombstone(bytes: &[u8]) -> Result<u8, BondSlotRecordError> {
    if bytes.len() != BOND_SLOT_RECORD_LEN {
        return Err(BondSlotRecordError::Length(bytes.len()));
    }
    let magic = u32::from_le_bytes(
        bytes[0..4]
            .try_into()
            .expect("a four-byte magic was just sliced from a longer record"),
    );
    if magic != BOND_MAGIC {
        return Err(BondSlotRecordError::Empty);
    }
    if bytes[4] != BOND_SLOT_VERSION {
        return Err(BondSlotRecordError::Version(bytes[4]));
    }
    let expected = u32::from_le_bytes(
        bytes[SLOT_CRC_OFFSET..]
            .try_into()
            .expect("a four-byte CRC was just sliced from a longer record"),
    );
    let actual = crc32_ieee(&bytes[..SLOT_CRC_OFFSET]);
    if expected != actual {
        return Err(BondSlotRecordError::Crc { expected, actual });
    }
    let slot = bytes[5];
    if usize::from(slot) >= BOND_SLOT_COUNT {
        return Err(BondSlotRecordError::Slot(slot));
    }
    if bytes[6] != BOND_TOMBSTONE_NAME_LEN {
        return Err(BondSlotRecordError::NameLength(usize::from(bytes[6])));
    }
    if bytes[7..SLOT_CRC_OFFSET].iter().any(|byte| *byte != 0) {
        return Err(BondSlotRecordError::Reserved);
    }
    Ok(slot)
}

/// Encodes the initial logical snapshot into the append-only page format.
///
/// Runtime persistence appends records instead of calling this function. It
/// remains public for deterministic host fixtures and migration tests.
pub fn encode_bond_page(
    slots: &[Option<StoredBondSlot>; BOND_SLOT_COUNT],
) -> [u8; BOND_PAGE_DATA_LEN] {
    let mut page = [0xff; BOND_PAGE_DATA_LEN];
    for (index, slot) in slots.iter().enumerate() {
        let Some(slot) = slot else { continue };
        if usize::from(slot.slot) != index {
            continue;
        }
        let start = index * BOND_SLOT_RECORD_LEN;
        page[start..start + BOND_SLOT_RECORD_LEN].copy_from_slice(&encode_bond_slot(slot));
    }
    page
}

/// Decodes the current page and, when needed, the 0.5 single-record layout.
///
/// The boolean is true only when the legacy record was found and needs an
/// append-only migration write. Records are replayed in physical order, so a
/// later valid record or tombstone wins for its logical slot. A CRC or version
/// problem remains visible to the caller through the last valid value rather
/// than allowing pairing material to be guessed.
pub fn decode_bond_page(
    page: &[u8; BOND_PAGE_DATA_LEN],
) -> ([Option<StoredBondSlot>; BOND_SLOT_COUNT], bool) {
    let mut slots = [None; BOND_SLOT_COUNT];
    let mut current_record_found = false;
    for record in page.chunks_exact(BOND_SLOT_RECORD_LEN) {
        if let Ok(slot) = decode_bond_slot(record) {
            slots[usize::from(slot.slot)] = Some(slot);
            current_record_found = true;
        } else if let Ok(slot) = decode_bond_tombstone(record) {
            slots[usize::from(slot)] = None;
            current_record_found = true;
        }
    }
    if !current_record_found && let Ok(bond) = decode_bond(&page[..BOND_RECORD_LEN]) {
        slots[0] = Some(
            StoredBondSlot::new(0, bond, b"BLE Keyboard")
                .expect("the built-in legacy migration name fits its fixed field"),
        );
        return (slots, true);
    }
    (slots, false)
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    fn sample() -> StoredBond {
        StoredBond {
            ltk: 0x0f0e_0d0c_0b0a_0908_0706_0504_0302_0100,
            addr_kind: 1,
            addr: [0x25, 0x55, 0x07, 0x25, 0xe9, 0xdb],
            irk: Some(0xdead_beef_cafe_f00d_1234_5678_9abc_def0),
            is_bonded: true,
            security_level: 2,
            ediv: 0xbeef,
            rand: [1, 2, 3, 4, 5, 6, 7, 8],
            encryption_key_len: 16,
        }
    }

    #[test]
    fn a_bond_survives_the_round_trip() {
        let bond = sample();

        let decoded = decode_bond(&encode_bond(&bond)).expect("a freshly encoded record decodes");

        assert_eq!(decoded, bond);
    }

    #[test]
    fn an_absent_resolving_key_stays_absent() {
        // A peer that sends no IRK must not come back with a key of zero, which
        // would be a legal key and would be offered to the controller as one.
        let bond = StoredBond {
            irk: None,
            ..sample()
        };

        let decoded = decode_bond(&encode_bond(&bond)).expect("a record without an IRK decodes");

        assert_eq!(decoded.irk, None);
    }

    #[test]
    fn an_all_zero_resolving_key_is_not_accepted_as_present() {
        let bond = StoredBond {
            irk: Some(0),
            ..sample()
        };

        assert_eq!(
            decode_bond(&encode_bond(&bond)),
            Err(BondRecordError::InvalidIrk)
        );
    }

    #[test]
    fn erased_flash_is_not_mistaken_for_a_bond() {
        let erased = [0xff; BOND_RECORD_LEN];

        assert_eq!(decode_bond(&erased), Err(BondRecordError::Empty));
    }

    #[test]
    fn a_never_written_slot_is_not_mistaken_for_a_bond() {
        let zeroed = [0x00; BOND_RECORD_LEN];

        assert_eq!(decode_bond(&zeroed), Err(BondRecordError::Empty));
    }

    #[test]
    fn a_corrupt_record_is_refused_rather_than_half_read() {
        let mut record = encode_bond(&sample());
        record[9] ^= 0x01;

        assert!(matches!(
            decode_bond(&record),
            Err(BondRecordError::Crc { .. })
        ));
    }

    #[test]
    fn a_record_from_another_layout_is_refused() {
        let mut record = encode_bond(&sample());
        record[4] = BOND_VERSION + 1;
        let crc = crc32_ieee(&record[..CRC_OFFSET]);
        record[CRC_OFFSET..].copy_from_slice(&crc.to_le_bytes());

        assert_eq!(
            decode_bond(&record),
            Err(BondRecordError::Version(BOND_VERSION + 1))
        );
    }

    #[test]
    fn debug_output_redacts_pairing_material() {
        let rendered = std::format!("{:?}", sample());

        assert!(rendered.contains("irk_present: true"));
        assert!(!rendered.contains("ltk:"));
        assert!(!rendered.contains("irk:"));
        assert!(!rendered.contains("rand:"));
        assert!(!rendered.contains("dead_beef"));
    }

    #[test]
    fn a_named_slot_survives_the_current_layout_round_trip() {
        let slot = StoredBondSlot::new(2, sample(), "Corne".as_bytes()).unwrap();

        let decoded = decode_bond_slot(&encode_bond_slot(&slot)).unwrap();

        assert_eq!(decoded, slot);
        assert_eq!(decoded.name_bytes(), b"Corne");
    }

    #[test]
    fn a_page_keeps_all_slots_and_erased_slots_empty() {
        let first = StoredBondSlot::new(0, sample(), b"Keyboard A").unwrap();
        let third = StoredBondSlot::new(3, sample(), b"Corne").unwrap();
        let page = encode_bond_page(&[Some(first), None, None, Some(third)]);

        let (decoded, migrated) = decode_bond_page(&page);

        assert!(!migrated);
        assert_eq!(decoded[0], Some(first));
        assert_eq!(decoded[1], None);
        assert_eq!(decoded[2], None);
        assert_eq!(decoded[3], Some(third));
    }

    #[test]
    fn a_later_slot_record_replaces_an_older_record_during_replay() {
        let first = StoredBondSlot::new(0, sample(), b"Keyboard A").unwrap();
        let renamed = StoredBondSlot::new(0, sample(), b"Keyboard B").unwrap();
        let mut page = [0xff; BOND_PAGE_DATA_LEN];
        page[..BOND_SLOT_RECORD_LEN].copy_from_slice(&encode_bond_slot(&first));
        page[BOND_SLOT_RECORD_LEN..2 * BOND_SLOT_RECORD_LEN]
            .copy_from_slice(&encode_bond_slot(&renamed));

        let (decoded, migrated) = decode_bond_page(&page);

        assert!(!migrated);
        assert_eq!(decoded[0], Some(renamed));
    }

    #[test]
    fn a_tombstone_prevents_an_older_slot_from_being_resurrected() {
        let slot = StoredBondSlot::new(0, sample(), b"Keyboard A").unwrap();
        let mut page = [0xff; BOND_PAGE_DATA_LEN];
        page[..BOND_SLOT_RECORD_LEN].copy_from_slice(&encode_bond_slot(&slot));
        page[BOND_SLOT_RECORD_LEN..2 * BOND_SLOT_RECORD_LEN]
            .copy_from_slice(&encode_bond_tombstone(0));

        let (decoded, migrated) = decode_bond_page(&page);

        assert!(!migrated);
        assert_eq!(decoded[0], None);
    }

    #[test]
    fn a_legacy_record_is_mapped_to_slot_zero_for_quiet_migration() {
        let mut page = [0xff; BOND_PAGE_DATA_LEN];
        page[..BOND_RECORD_LEN].copy_from_slice(&encode_bond(&sample()));

        let (decoded, migrated) = decode_bond_page(&page);

        assert!(migrated);
        assert_eq!(decoded[0].unwrap().bond, sample());
        assert_eq!(decoded[0].unwrap().name_bytes(), b"BLE Keyboard");
        assert_eq!(decoded[1], None);
    }

    #[test]
    fn a_slot_with_invalid_utf8_name_is_refused() {
        let error = StoredBondSlot::new(0, sample(), &[0xff]);

        assert_eq!(error, Err(BondSlotRecordError::NameEncoding));
    }

    #[test]
    fn a_slot_with_an_all_zero_resolving_key_is_refused() {
        let slot = StoredBondSlot::new(
            0,
            StoredBond {
                irk: Some(0),
                ..sample()
            },
            b"Keyboard A",
        )
        .unwrap();

        assert_eq!(
            decode_bond_slot(&encode_bond_slot(&slot)),
            Err(BondSlotRecordError::InvalidIrk)
        );
    }

    #[test]
    fn a_slot_with_changed_reserved_bytes_is_refused_even_with_a_new_crc() {
        let slot = StoredBondSlot::new(0, sample(), b"Keyboard A").unwrap();
        let mut record = encode_bond_slot(&slot);
        record[80] = 1;
        let crc = crc32_ieee(&record[..SLOT_CRC_OFFSET]);
        record[SLOT_CRC_OFFSET..].copy_from_slice(&crc.to_le_bytes());

        assert_eq!(
            decode_bond_slot(&record),
            Err(BondSlotRecordError::Reserved)
        );
    }

    #[test]
    fn named_slot_debug_does_not_include_pairing_material() {
        let slot = StoredBondSlot::new(1, sample(), b"Keyboard A").unwrap();
        let rendered = std::format!("{:?}", slot);

        assert!(rendered.contains("name"));
        assert!(!rendered.contains("ltk:"));
        assert!(!rendered.contains("dead_beef"));
    }
}
