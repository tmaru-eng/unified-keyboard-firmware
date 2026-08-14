//! Persistent storage format for one keyboard compatibility profile.

use crate::uf2_reset::crc32_ieee;
use ukf_core::{BridgeProfile, SOURCE_SLOT_COUNT};

/// Wire size of one stored profile record.
pub const PROFILE_RECORD_LEN: usize = 16;

const PROFILE_MAGIC: u32 = 0x554b_4650;
const PROFILE_VERSION: u8 = 1;
const PROFILE_PAYLOAD_LEN: usize = 2;
const PROFILE_FLAGS_MASK: u8 = 0x07;
const CRC_OFFSET: usize = PROFILE_RECORD_LEN - size_of::<u32>();
/// Length of the versioned source-profile transfer payload.
pub const SOURCE_PROFILE_PAYLOAD_LEN: usize = 4;
/// Version of the source-profile transfer payload.
pub const SOURCE_PROFILE_PAYLOAD_VERSION: u8 = 2;

/// One keyboard compatibility profile in the form used by the store.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoredProfile {
    /// Apply the ANSI US to JIS compatibility conversion.
    pub us_to_jis: bool,
    /// Convert Caps Lock presses into Left Control presses.
    pub caps_to_ctrl: bool,
    /// Swap Alt and GUI modifiers.
    pub swap_alt_gui: bool,
}

/// One committed profile waiting for quiet-time flash persistence.
pub type PendingProfile = Option<StoredProfile>;

/// Why a stored profile could not be read back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileRecordError {
    /// The slice was not exactly one record or payload.
    Length(usize),
    /// The slot has never been written: it carries no profile magic.
    ///
    /// This is the one failure a caller is expected to answer by falling back
    /// to the built-in default. The others below all mean a profile *was*
    /// written and cannot be trusted, which is a different action.
    Empty,
    /// A reserved byte was not zero, so the record is not one this build wrote.
    Reserved,
    /// A flag bit is set that this build has no meaning for.
    ///
    /// Kept apart from [`Self::Empty`] deliberately. A newer sender setting a
    /// flag this build cannot name, answered by silently using the default,
    /// would change what every keystroke becomes without either side seeing a
    /// disagreement.
    UnknownFlags(u8),
    /// The record or payload was written by a different layout version.
    Version(u8),
    /// The record is corrupt.
    Crc {
        /// CRC stored in the record.
        expected: u32,
        /// CRC computed over the record.
        actual: u32,
    },
    /// The source-profile payload's explicit length field is not four.
    PayloadLength(u8),
    /// The source-profile payload names a slot outside the fixed capacity.
    SourceSlot(u8),
}

/// Encodes one profile into its sixteen-byte flash record.
pub fn encode_profile(profile: &StoredProfile) -> [u8; PROFILE_RECORD_LEN] {
    let mut bytes = [0; PROFILE_RECORD_LEN];
    bytes[0..4].copy_from_slice(&PROFILE_MAGIC.to_le_bytes());
    bytes[4] = PROFILE_VERSION;
    bytes[5] = profile_flags(profile);
    let crc = crc32_ieee(&bytes[..CRC_OFFSET]);
    bytes[CRC_OFFSET..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

/// Decodes one profile flash record, rejecting anything that is not current.
pub fn decode_profile(bytes: &[u8]) -> Result<StoredProfile, ProfileRecordError> {
    if bytes.len() != PROFILE_RECORD_LEN {
        return Err(ProfileRecordError::Length(bytes.len()));
    }

    let magic = u32::from_le_bytes(
        bytes[0..4]
            .try_into()
            .expect("a four-byte magic was just sliced from a longer record"),
    );
    if magic != PROFILE_MAGIC {
        return Err(ProfileRecordError::Empty);
    }
    if bytes[4] != PROFILE_VERSION {
        return Err(ProfileRecordError::Version(bytes[4]));
    }

    let expected = u32::from_le_bytes(
        bytes[CRC_OFFSET..]
            .try_into()
            .expect("a four-byte CRC was just sliced from a longer record"),
    );
    let actual = crc32_ieee(&bytes[..CRC_OFFSET]);
    if expected != actual {
        return Err(ProfileRecordError::Crc { expected, actual });
    }

    if bytes[6..12].iter().any(|&byte| byte != 0) {
        return Err(ProfileRecordError::Reserved);
    }

    profile_from_flags(bytes[5])
}

/// Encodes the compact two-byte profile payload used by the host protocol.
pub fn encode_profile_payload(profile: &StoredProfile) -> [u8; PROFILE_PAYLOAD_LEN] {
    [PROFILE_VERSION, profile_flags(profile)]
}

/// Decodes the compact two-byte profile payload.
pub fn decode_profile_payload(bytes: &[u8]) -> Result<StoredProfile, ProfileRecordError> {
    if bytes.len() != PROFILE_PAYLOAD_LEN {
        return Err(ProfileRecordError::Length(bytes.len()));
    }
    if bytes[0] != PROFILE_VERSION {
        return Err(ProfileRecordError::Version(bytes[0]));
    }

    profile_from_flags(bytes[1])
}

/// Encodes a source-specific profile payload with an explicit version, length,
/// and target slot. It is intentionally distinct from the legacy global
/// two-byte profile payload.
pub fn encode_source_profile_payload(
    slot: u8,
    profile: &StoredProfile,
) -> [u8; SOURCE_PROFILE_PAYLOAD_LEN] {
    [
        SOURCE_PROFILE_PAYLOAD_VERSION,
        SOURCE_PROFILE_PAYLOAD_LEN as u8,
        slot,
        profile_flags(profile),
    ]
}

/// Decodes a source-specific profile payload without applying an ambiguous
/// legacy payload to a source slot.
pub fn decode_source_profile_payload(
    bytes: &[u8],
) -> Result<(u8, StoredProfile), ProfileRecordError> {
    if bytes.len() != SOURCE_PROFILE_PAYLOAD_LEN {
        return Err(ProfileRecordError::Length(bytes.len()));
    }
    if bytes[0] != SOURCE_PROFILE_PAYLOAD_VERSION {
        return Err(ProfileRecordError::Version(bytes[0]));
    }
    if bytes[1] != SOURCE_PROFILE_PAYLOAD_LEN as u8 {
        return Err(ProfileRecordError::PayloadLength(bytes[1]));
    }
    if usize::from(bytes[2]) >= SOURCE_SLOT_COUNT {
        return Err(ProfileRecordError::SourceSlot(bytes[2]));
    }
    Ok((bytes[2], profile_from_flags(bytes[3])?))
}

impl From<StoredProfile> for BridgeProfile {
    fn from(profile: StoredProfile) -> Self {
        Self {
            us_to_jis: profile.us_to_jis,
            caps_to_ctrl: profile.caps_to_ctrl,
            swap_alt_gui: profile.swap_alt_gui,
        }
    }
}

impl From<BridgeProfile> for StoredProfile {
    fn from(profile: BridgeProfile) -> Self {
        Self {
            us_to_jis: profile.us_to_jis,
            caps_to_ctrl: profile.caps_to_ctrl,
            swap_alt_gui: profile.swap_alt_gui,
        }
    }
}

fn profile_flags(profile: &StoredProfile) -> u8 {
    u8::from(profile.us_to_jis)
        | (u8::from(profile.caps_to_ctrl) << 1)
        | (u8::from(profile.swap_alt_gui) << 2)
}

fn profile_from_flags(flags: u8) -> Result<StoredProfile, ProfileRecordError> {
    if flags & !PROFILE_FLAGS_MASK != 0 {
        return Err(ProfileRecordError::UnknownFlags(flags));
    }

    Ok(StoredProfile {
        us_to_jis: flags & 0x01 != 0,
        caps_to_ctrl: flags & 0x02 != 0,
        swap_alt_gui: flags & 0x04 != 0,
    })
}
