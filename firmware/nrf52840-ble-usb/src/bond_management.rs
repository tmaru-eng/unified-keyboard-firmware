//! Host-to-firmware operations for registered BLE bond slots.
//!
//! Listing deliberately reuses the source diagnostics blocks: a registered
//! bond is source slot 0 through 3, while slot 4 remains the virtual source.
//! This payload therefore only needs to carry the two mutating operations that
//! cannot be expressed by a read: rename and delete.

use ukf_core::{SourceName, SourceNameError};

/// Transfer target for bond-slot management operations.
pub const TARGET_BOND_MANAGEMENT: u8 = 4;
/// Current management payload version.
pub const BOND_MANAGEMENT_VERSION: u8 = 1;
/// Rename operation.
pub const OP_RENAME: u8 = 1;
/// Delete operation.
pub const OP_DELETE: u8 = 2;
/// Fixed payload size. A fixed shape keeps delete and rename equally easy to
/// validate and makes trailing-byte mistakes visible to the receiver.
pub const BOND_MANAGEMENT_PAYLOAD_LEN: usize = 18;
/// Maximum name bytes carried by a rename request.
pub const BOND_MANAGEMENT_NAME_LEN: usize = 13;
const BOND_SLOT_COUNT: u8 = 4;

/// One validated bond-management request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BondManagementRequest {
    /// Changes only the display name of a registered slot.
    Rename {
        /// Registration slot to update.
        slot: u8,
        /// New fixed-capacity name.
        name: SourceName,
    },
    /// Removes the pairing and its source profile from a registered slot.
    Delete {
        /// Registration slot to remove.
        slot: u8,
    },
}

/// Why a bond-management payload was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BondManagementError {
    /// Payload is not the fixed 18-byte shape.
    Length(usize),
    /// Payload version is unsupported.
    Version(u8),
    /// Operation byte is unknown.
    Operation(u8),
    /// Slot is outside the four registration slots.
    Slot(u8),
    /// Name length does not fit the fixed name field.
    NameLength(u8),
    /// Name bytes are not valid UTF-8.
    NameEncoding,
    /// Delete requests must not carry a name.
    DeleteName,
    /// Bytes after the meaningful name are not zero-filled.
    Padding,
}

impl From<SourceNameError> for BondManagementError {
    fn from(error: SourceNameError) -> Self {
        match error {
            SourceNameError::TooLong(_) => Self::NameLength(u8::MAX),
            SourceNameError::InvalidUtf8 => Self::NameEncoding,
        }
    }
}

/// Encodes a validated rename request.
pub fn encode_rename(
    slot: u8,
    name: &[u8],
) -> Result<[u8; BOND_MANAGEMENT_PAYLOAD_LEN], BondManagementError> {
    validate_slot(slot)?;
    let name = SourceName::try_from_bytes(name).map_err(BondManagementError::from)?;
    let mut payload = [0; BOND_MANAGEMENT_PAYLOAD_LEN];
    payload[0] = BOND_MANAGEMENT_VERSION;
    payload[1] = OP_RENAME;
    payload[2] = slot;
    payload[3] = name.len();
    payload[4..4 + BOND_MANAGEMENT_NAME_LEN].copy_from_slice(&name.to_bytes());
    Ok(payload)
}

/// Encodes a validated delete request.
pub fn encode_delete(slot: u8) -> Result<[u8; BOND_MANAGEMENT_PAYLOAD_LEN], BondManagementError> {
    validate_slot(slot)?;
    let mut payload = [0; BOND_MANAGEMENT_PAYLOAD_LEN];
    payload[0] = BOND_MANAGEMENT_VERSION;
    payload[1] = OP_DELETE;
    payload[2] = slot;
    Ok(payload)
}

/// Decodes and validates one bond-management payload.
pub fn decode(payload: &[u8]) -> Result<BondManagementRequest, BondManagementError> {
    if payload.len() != BOND_MANAGEMENT_PAYLOAD_LEN {
        return Err(BondManagementError::Length(payload.len()));
    }
    if payload[0] != BOND_MANAGEMENT_VERSION {
        return Err(BondManagementError::Version(payload[0]));
    }
    validate_slot(payload[2])?;
    match payload[1] {
        OP_RENAME => {
            let name_len = usize::from(payload[3]);
            if name_len > BOND_MANAGEMENT_NAME_LEN {
                return Err(BondManagementError::NameLength(payload[3]));
            }
            if payload[4 + name_len..].iter().any(|byte| *byte != 0) {
                return Err(BondManagementError::Padding);
            }
            let name = SourceName::try_from_bytes(&payload[4..4 + name_len])
                .map_err(BondManagementError::from)?;
            Ok(BondManagementRequest::Rename {
                slot: payload[2],
                name,
            })
        }
        OP_DELETE => {
            if payload[3..].iter().any(|byte| *byte != 0) {
                return Err(BondManagementError::DeleteName);
            }
            Ok(BondManagementRequest::Delete { slot: payload[2] })
        }
        operation => Err(BondManagementError::Operation(operation)),
    }
}

fn validate_slot(slot: u8) -> Result<(), BondManagementError> {
    if slot >= BOND_SLOT_COUNT {
        Err(BondManagementError::Slot(slot))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    #[test]
    fn a_rename_round_trip_keeps_the_slot_and_utf8_name() {
        let payload = encode_rename(2, "Corne".as_bytes()).unwrap();

        assert_eq!(
            decode(&payload),
            Ok(BondManagementRequest::Rename {
                slot: 2,
                name: SourceName::try_from_bytes(b"Corne").unwrap(),
            })
        );
    }

    #[test]
    fn rename_wire_fixture_matches_the_ui_codec() {
        assert_eq!(
            encode_rename(2, b"Corne").unwrap(),
            [
                1, 1, 2, 5, b'C', b'o', b'r', b'n', b'e', 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ]
        );
    }

    #[test]
    fn a_delete_round_trip_has_no_hidden_name_bytes() {
        let payload = encode_delete(1).unwrap();

        assert_eq!(
            decode(&payload),
            Ok(BondManagementRequest::Delete { slot: 1 })
        );
        assert_eq!(&payload[3..], &[0; BOND_MANAGEMENT_PAYLOAD_LEN - 3]);
    }

    #[test]
    fn unknown_and_virtual_slots_are_rejected() {
        assert_eq!(encode_delete(4), Err(BondManagementError::Slot(4)));
        let mut payload = encode_delete(0).unwrap();
        payload[2] = 9;
        assert_eq!(decode(&payload), Err(BondManagementError::Slot(9)));
    }

    #[test]
    fn malformed_name_and_noncanonical_padding_are_rejected() {
        let mut too_long = encode_delete(0).unwrap();
        too_long[1] = OP_RENAME;
        too_long[3] = 14;
        assert_eq!(decode(&too_long), Err(BondManagementError::NameLength(14)));

        let mut padded = encode_rename(0, b"KB-A").unwrap();
        padded[10] = 1;
        assert_eq!(decode(&padded), Err(BondManagementError::Padding));
    }

    #[test]
    fn delete_cannot_smuggle_name_bytes() {
        let mut payload = encode_delete(0).unwrap();
        payload[3] = 1;

        assert_eq!(decode(&payload), Err(BondManagementError::DeleteName));
    }

    #[test]
    fn invalid_utf8_is_rejected_before_it_reaches_the_wire() {
        assert_eq!(
            encode_rename(0, &[0xff]),
            Err(BondManagementError::NameEncoding)
        );
    }
}
