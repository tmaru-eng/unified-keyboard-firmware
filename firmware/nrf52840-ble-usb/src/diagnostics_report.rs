//! Host-testable status report for the configuration HID interface.

use crate::config_transfer::TransferState;
use crate::keymap_store::KEYMAP_PAYLOAD_MAX_LEN;
use crate::uf2_reset::{CONFIG_PROTOCOL_VERSION, crc32_ieee};
use ukf_core::{
    BridgeProfile, InputTransport, SOURCE_NAME_LEN, SOURCE_SLOT_COUNT, SourceIdentity, SourceName,
    SourceSnapshot, SourceState,
};

/// Status report kind carried in byte one.
pub const STATUS_KIND: u8 = 1;
/// Block kind carrying a slice of the recorded panic message.
///
/// The message identifies which assertion fired, which a line number alone
/// cannot: a panic raised inside a dependency reports that dependency's
/// location, not the failing condition.
pub const PANIC_TEXT_KIND: u8 = 2;
/// Message bytes carried by one panic-text block.
///
/// The payload starts at byte four and the CRC occupies the last four bytes, so
/// twenty-four is the largest slice that fits between them. Twenty-six fit the
/// block but ran into the CRC, and the two message bytes at the end of every
/// chunk came back as CRC bytes — which read as a corrupt message from the
/// controller rather than as a defect in this encoder.
pub const PANIC_TEXT_CHUNK_LEN: usize = DIAGNOSTICS_REPORT_LEN - CRC_LEN - PANIC_TEXT_HEADER_LEN;
/// Block kind carrying the pairing passkey this bridge is displaying.
///
/// A keyboard types its passkey and cannot show one, so the bridge has to.
/// With no screen on the board, the configuration interface is where it goes.
pub const PASSKEY_KIND: u8 = 3;
/// Block kind carrying the firmware version and build identity.
pub const IDENTITY_KIND: u8 = 4;
/// Block kind carrying the configuration transfer state.
pub const TRANSFER_KIND: u8 = 5;
/// Block kind carrying the active compatibility profile.
pub const PROFILE_KIND: u8 = 6;
/// Block kind carrying one fixed-capacity source slot.
pub const SOURCE_KIND: u8 = 7;
/// Block kind carrying one chunk of the active keymap payload.
pub const KEYMAP_KIND: u8 = 8;
/// Selector value asking for the identity block.
pub const SELECT_IDENTITY: u8 = 0xfd;
/// Selector value asking for the configuration transfer block.
pub const SELECT_TRANSFER: u8 = 0xfc;
/// Selector value asking for the compatibility profile block.
pub const SELECT_PROFILE: u8 = 0xfb;
/// Selector value asking for the source slot selected by the command path.
pub const SELECT_SOURCE: u8 = 0xfa;
/// Version of the source record inside a source diagnostics block.
pub const SOURCE_RECORD_VERSION: u8 = 1;
/// Source identity address is present in the source block.
pub const SOURCE_FLAG_IDENTITY_ADDRESS: u8 = 1 << 0;
/// A private IRK exists for the source; the bytes are never transmitted.
pub const SOURCE_FLAG_IRK_PRESENT: u8 = 1 << 1;
const SOURCE_FLAGS_MASK: u8 = SOURCE_FLAG_IDENTITY_ADDRESS | SOURCE_FLAG_IRK_PRESENT;
const PROFILE_FLAGS_MASK: u8 = 0b111;
/// A pairing from a previous boot was found in flash at start-up.
pub const BOND_FLAG_LOADED: u8 = 1 << 0;
/// A pairing has been written to flash since start-up.
pub const BOND_FLAG_SAVED: u8 = 1 << 1;
/// The peer of the most recent connection was already bonded.
///
/// This is what separates a first pairing from a reconnection, and the two
/// take different paths: one runs the pairing procedure, the other only
/// switches encryption back on using the stored key.
pub const BOND_FLAG_PEER_BONDED: u8 = 1 << 2;
/// Pairing completed but produced no keys to keep.
///
/// Separates "the two sides never agreed to bond" from "we agreed and the
/// write failed", which look identical from the absence of a saved key.
pub const BOND_FLAG_PAIRED_WITHOUT_KEYS: u8 = 1 << 3;
/// Keys were produced but could not be written to flash.
pub const BOND_FLAG_WRITE_FAILED: u8 = 1 << 4;
/// Version, kind, chunk index, and byte count preceding a panic-text payload.
const PANIC_TEXT_HEADER_LEN: usize = 4;
/// Width of the CRC suffix every diagnostics block carries.
const CRC_LEN: usize = 4;
/// Wire size of the diagnostics payload.
pub const DIAGNOSTICS_REPORT_LEN: usize = 32;
/// Bytes of keymap payload carried by one keymap diagnostics block.
pub const KEYMAP_CHUNK_DATA_LEN: usize = 21;
/// Maximum number of keymap diagnostics blocks for the fixed payload limit.
pub const KEYMAP_CHUNK_COUNT_MAX: u8 = KEYMAP_PAYLOAD_MAX_LEN.div_ceil(KEYMAP_CHUNK_DATA_LEN) as u8;
const KEYMAP_REPORT_VERSION: u8 = 1;

const fn decimal_byte(value: &[u8]) -> u8 {
    let mut result = 0;
    let mut index = 0;
    while index < value.len() {
        result = result * 10 + (value[index] - b'0');
        index += 1;
    }
    result
}

const fn hex_nibble(value: u8) -> u32 {
    match value {
        b'0'..=b'9' => (value - b'0') as u32,
        b'a'..=b'f' => (value - b'a' + 10) as u32,
        b'A'..=b'F' => (value - b'A' + 10) as u32,
        _ => 0,
    }
}

const fn hex_u32(value: &[u8]) -> u32 {
    let mut result = 0;
    let mut index = 0;
    while index < value.len() {
        result = (result << 4) | hex_nibble(value[index]);
        index += 1;
    }
    result
}

const FIRMWARE_MAJOR: u8 = decimal_byte(env!("CARGO_PKG_VERSION_MAJOR").as_bytes());
const FIRMWARE_MINOR: u8 = decimal_byte(env!("CARGO_PKG_VERSION_MINOR").as_bytes());
const FIRMWARE_PATCH: u8 = decimal_byte(env!("CARGO_PKG_VERSION_PATCH").as_bytes());
const BUILD_ID: u32 = hex_u32(env!("UKF_BUILD_ID").as_bytes());
const BUILD_DIRTY: bool = env!("UKF_BUILD_DIRTY").as_bytes()[0] == b'1';

/// Firmware version and source-tree identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdentityReport {
    /// Major firmware version.
    pub major: u8,
    /// Minor firmware version.
    pub minor: u8,
    /// Patch firmware version.
    pub patch: u8,
    /// Short git commit hash interpreted as a little-endian integer.
    pub build_id: u32,
    /// Whether the source tree had uncommitted changes.
    pub dirty: bool,
}

/// Encodes the firmware version and source-tree identity.
pub fn encode_identity(report: &IdentityReport) -> [u8; DIAGNOSTICS_REPORT_LEN] {
    let mut bytes = [0; DIAGNOSTICS_REPORT_LEN];
    bytes[0] = CONFIG_PROTOCOL_VERSION;
    bytes[1] = IDENTITY_KIND;
    bytes[2] = report.major;
    bytes[3] = report.minor;
    bytes[4] = report.patch;
    bytes[5..9].copy_from_slice(&report.build_id.to_le_bytes());
    bytes[9] = u8::from(report.dirty);
    let crc = crc32_ieee(&bytes[..28]);
    bytes[28..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

/// Configuration transfer state encoded in the diagnostic block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransferReport {
    /// Current transfer state.
    pub state: TransferState,
    /// Stable number of the last transfer failure, or zero.
    pub last_error: u8,
    /// Current transfer target.
    pub target: u8,
    /// Number of bytes declared by BEGIN.
    pub expected_len: u16,
    /// Number of bytes accepted from CHUNK requests.
    pub received_len: u16,
    /// Next sequential chunk index.
    pub next_index: u16,
    /// CRC declared by BEGIN.
    pub declared_crc: u32,
    /// CRC computed during COMMIT validation, or zero before then.
    pub computed_crc: u32,
}

/// Encodes the configuration transfer state and CRC.
pub fn encode_transfer(report: &TransferReport) -> [u8; DIAGNOSTICS_REPORT_LEN] {
    let mut bytes = [0; DIAGNOSTICS_REPORT_LEN];
    bytes[0] = CONFIG_PROTOCOL_VERSION;
    bytes[1] = TRANSFER_KIND;
    bytes[2] = report.state as u8;
    bytes[3] = report.last_error;
    bytes[4] = report.target;
    bytes[6..8].copy_from_slice(&report.expected_len.to_le_bytes());
    bytes[8..10].copy_from_slice(&report.received_len.to_le_bytes());
    bytes[10..12].copy_from_slice(&report.next_index.to_le_bytes());
    bytes[12..16].copy_from_slice(&report.declared_crc.to_le_bytes());
    bytes[16..20].copy_from_slice(&report.computed_crc.to_le_bytes());
    let crc = crc32_ieee(&bytes[..28]);
    bytes[28..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

/// Compatibility profile state encoded in the diagnostic block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProfileReport {
    /// Current profile flags.
    pub profile: crate::profile_store::StoredProfile,
    /// Whether the profile came from a valid flash record.
    pub stored: bool,
    /// Whether the active profile is waiting to be written to flash.
    pub pending: bool,
}

/// Encodes the active compatibility profile and CRC.
pub fn encode_profile_block(report: &ProfileReport) -> [u8; DIAGNOSTICS_REPORT_LEN] {
    let mut bytes = [0; DIAGNOSTICS_REPORT_LEN];
    bytes[0] = CONFIG_PROTOCOL_VERSION;
    bytes[1] = PROFILE_KIND;
    bytes[2] = 1;
    bytes[3] = u8::from(report.profile.us_to_jis)
        | (u8::from(report.profile.caps_to_ctrl) << 1)
        | (u8::from(report.profile.swap_alt_gui) << 2);
    bytes[4] = u8::from(report.stored);
    bytes[5] = u8::from(report.pending);
    let crc = crc32_ieee(&bytes[..28]);
    bytes[28..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

/// Public source information carried by one source diagnostics block.
///
/// The transport is absent for an unused registration slot. IRK material is
/// deliberately represented only by [`SourceIdentity::irk_present`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceReport {
    /// Registration slot number.
    pub slot: u8,
    /// Registered transport, or `None` for an unused slot.
    pub transport: Option<InputTransport>,
    /// Source-local lifecycle state.
    pub state: SourceState,
    /// Immutable identity facts and private-key presence.
    pub identity: SourceIdentity,
    /// Compatibility profile owned by this source slot.
    pub profile: BridgeProfile,
    /// Fixed-capacity user-visible source name.
    pub name: SourceName,
}

impl SourceReport {
    /// Converts a bridge snapshot without exposing private IRK bytes.
    pub const fn from_snapshot(snapshot: &SourceSnapshot) -> Self {
        Self {
            slot: snapshot.slot.0,
            transport: snapshot.transport,
            state: snapshot.state,
            identity: snapshot.identity,
            profile: snapshot.profile,
            name: snapshot.name,
        }
    }
}

/// Error returned when a source diagnostics block is malformed or ambiguous.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceReportError {
    /// Payload is not exactly one 32-byte block.
    Length(usize),
    /// Protocol envelope is unsupported.
    Version(u8),
    /// Payload is not a source block.
    Kind(u8),
    /// Source record layout version is unsupported.
    RecordVersion(u8),
    /// Slot is outside the four registered plus one virtual slots.
    Slot(u8),
    /// Transport code is unknown.
    Transport(u8),
    /// Lifecycle state is unknown.
    State(u8),
    /// Reserved source flags are set.
    Flags(u8),
    /// Reserved profile flags are set.
    ProfileFlags(u8),
    /// An absent identity address must have zero bytes on the wire.
    IdentityAddress,
    /// An IRK cannot be present without the identity address it resolves.
    IdentityKeyWithoutAddress,
    /// Name length does not fit the fixed source-name field.
    NameLength(u8),
    /// Name bytes are not valid UTF-8.
    NameEncoding,
    /// Bytes after the meaningful name are not zero-filled.
    NamePadding,
    /// Transport presence and lifecycle state disagree.
    StateTransport,
    /// CRC suffix does not match bytes zero through twenty-seven.
    Crc {
        /// CRC encoded by the sender.
        expected: u32,
        /// CRC computed over the payload.
        actual: u32,
    },
}

/// Encodes one source snapshot into the fixed 32-byte diagnostics block.
pub fn encode_source_block(report: &SourceReport) -> [u8; DIAGNOSTICS_REPORT_LEN] {
    let mut bytes = [0; DIAGNOSTICS_REPORT_LEN];
    bytes[0] = CONFIG_PROTOCOL_VERSION;
    bytes[1] = SOURCE_KIND;
    bytes[2] = SOURCE_RECORD_VERSION;
    bytes[3] = report.slot;
    bytes[4] = report.transport.map_or(0, InputTransport::wire_code);
    bytes[5] = report.state as u8;
    if report.identity.address.is_some() {
        bytes[6] |= SOURCE_FLAG_IDENTITY_ADDRESS;
    }
    if report.identity.irk_present {
        bytes[6] |= SOURCE_FLAG_IRK_PRESENT;
    }
    bytes[7] = profile_flags(report.profile);
    if let Some(address) = report.identity.address {
        bytes[8..14].copy_from_slice(&address);
    }
    let name = report.name.as_bytes();
    bytes[14] = name.len() as u8;
    bytes[15..15 + name.len()].copy_from_slice(name);
    let crc = crc32_ieee(&bytes[..28]);
    bytes[28..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

/// Encodes one selected chunk of the active keymap payload.
///
/// The payload length and chunk count are repeated in every block so a host
/// can reject a mixed or truncated read before handing bytes to the keymap
/// decoder. The final chunk is zero-padded before the block CRC.
pub fn encode_keymap_chunk(
    payload: &[u8],
    chunk_index: u8,
) -> Option<[u8; DIAGNOSTICS_REPORT_LEN]> {
    if payload.is_empty() || payload.len() > KEYMAP_PAYLOAD_MAX_LEN {
        return None;
    }
    let chunk_count = payload.len().div_ceil(KEYMAP_CHUNK_DATA_LEN);
    if usize::from(chunk_index) >= chunk_count || chunk_count > usize::from(KEYMAP_CHUNK_COUNT_MAX)
    {
        return None;
    }

    let start = usize::from(chunk_index) * KEYMAP_CHUNK_DATA_LEN;
    let end = (start + KEYMAP_CHUNK_DATA_LEN).min(payload.len());
    let mut bytes = [0; DIAGNOSTICS_REPORT_LEN];
    bytes[0] = CONFIG_PROTOCOL_VERSION;
    bytes[1] = KEYMAP_KIND;
    bytes[2] = KEYMAP_REPORT_VERSION;
    bytes[3] = chunk_index;
    bytes[4] = chunk_count as u8;
    bytes[5..7].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    bytes[7..7 + end - start].copy_from_slice(&payload[start..end]);
    let crc = crc32_ieee(&bytes[..28]);
    bytes[28..].copy_from_slice(&crc.to_le_bytes());
    Some(bytes)
}

/// Decodes one source diagnostics block and rejects unknown slots, reserved
/// bits, non-canonical padding, and inconsistent unregistered records.
pub fn decode_source_block(payload: &[u8]) -> Result<SourceReport, SourceReportError> {
    if payload.len() != DIAGNOSTICS_REPORT_LEN {
        return Err(SourceReportError::Length(payload.len()));
    }
    if payload[0] != CONFIG_PROTOCOL_VERSION {
        return Err(SourceReportError::Version(payload[0]));
    }
    if payload[1] != SOURCE_KIND {
        return Err(SourceReportError::Kind(payload[1]));
    }
    if payload[2] != SOURCE_RECORD_VERSION {
        return Err(SourceReportError::RecordVersion(payload[2]));
    }
    if usize::from(payload[3]) >= SOURCE_SLOT_COUNT {
        return Err(SourceReportError::Slot(payload[3]));
    }
    let expected = u32::from_le_bytes(
        payload[28..]
            .try_into()
            .expect("validated source block has a four-byte CRC suffix"),
    );
    let actual = crc32_ieee(&payload[..28]);
    if actual != expected {
        return Err(SourceReportError::Crc { expected, actual });
    }
    if payload[6] & !SOURCE_FLAGS_MASK != 0 {
        return Err(SourceReportError::Flags(payload[6]));
    }
    if payload[7] & !PROFILE_FLAGS_MASK != 0 {
        return Err(SourceReportError::ProfileFlags(payload[7]));
    }

    let transport = match payload[4] {
        0 => None,
        value => {
            Some(InputTransport::from_wire_code(value).ok_or(SourceReportError::Transport(value))?)
        }
    };
    let state =
        SourceState::try_from(payload[5]).map_err(|_| SourceReportError::State(payload[5]))?;
    if transport.is_none() != (state == SourceState::Unregistered) {
        return Err(SourceReportError::StateTransport);
    }

    let address_bytes: [u8; 6] = payload[8..14]
        .try_into()
        .expect("source block has a fixed six-byte address field");
    let has_address = payload[6] & SOURCE_FLAG_IDENTITY_ADDRESS != 0;
    if !has_address && address_bytes != [0; 6] {
        return Err(SourceReportError::IdentityAddress);
    }
    if !has_address && payload[6] & SOURCE_FLAG_IRK_PRESENT != 0 {
        return Err(SourceReportError::IdentityKeyWithoutAddress);
    }
    let identity = SourceIdentity {
        address: has_address.then_some(address_bytes),
        irk_present: payload[6] & SOURCE_FLAG_IRK_PRESENT != 0,
    };

    let name_len = usize::from(payload[14]);
    if name_len > SOURCE_NAME_LEN {
        return Err(SourceReportError::NameLength(payload[14]));
    }
    if payload[15 + name_len..28].iter().any(|byte| *byte != 0) {
        return Err(SourceReportError::NamePadding);
    }
    let name = SourceName::try_from_bytes(&payload[15..15 + name_len])
        .map_err(|_| SourceReportError::NameEncoding)?;
    let profile = BridgeProfile {
        us_to_jis: payload[7] & 1 != 0,
        caps_to_ctrl: payload[7] & 2 != 0,
        swap_alt_gui: payload[7] & 4 != 0,
    };

    if state == SourceState::Unregistered && (payload[6] != 0 || payload[7] != 0 || name_len != 0) {
        return Err(SourceReportError::StateTransport);
    }

    Ok(SourceReport {
        slot: payload[3],
        transport,
        state,
        identity,
        profile,
        name,
    })
}

fn profile_flags(profile: BridgeProfile) -> u8 {
    u8::from(profile.us_to_jis)
        | (u8::from(profile.caps_to_ctrl) << 1)
        | (u8::from(profile.swap_alt_gui) << 2)
}

/// Identity compiled into this firmware.
pub const fn firmware_identity() -> IdentityReport {
    IdentityReport {
        major: FIRMWARE_MAJOR,
        minor: FIRMWARE_MINOR,
        patch: FIRMWARE_PATCH,
        build_id: BUILD_ID,
        dirty: BUILD_DIRTY,
    }
}

/// BLE central lifecycle state encoded in the report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BridgeState {
    /// The radio task has not started scanning.
    Starting = 0,
    /// The central is scanning.
    Scanning = 1,
    /// A HID advertiser was selected and a connection is in progress.
    Connecting = 2,
    /// Link security is being established.
    Securing = 3,
    /// HID GATT discovery is in progress.
    Discovering = 4,
    /// HID input notifications are subscribed.
    Subscribed = 5,
    /// The most recent central operation failed.
    Failed = 6,
}

impl TryFrom<u8> for BridgeState {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Starting),
            1 => Ok(Self::Scanning),
            2 => Ok(Self::Connecting),
            3 => Ok(Self::Securing),
            4 => Ok(Self::Discovering),
            5 => Ok(Self::Subscribed),
            6 => Ok(Self::Failed),
            _ => Err(()),
        }
    }
}

/// Decoded diagnostics payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticsReport {
    /// Current central lifecycle state.
    pub state: BridgeState,
    /// Last controller/host error code, or zero.
    pub last_error: u8,
    /// Number of advertisements observed since boot.
    pub advertisements_seen: u16,
    /// Number of advertisements declaring the HID service.
    pub hid_advertisements: u16,
    /// Most recent HID advertiser address, in advertising byte order.
    pub last_address: [u8; 6],
    /// RSSI of the most recent HID advertisement.
    pub last_rssi: i8,
    /// Number of accepted connections since boot.
    pub connection_count: u8,
    /// Number of input reports received from the peer.
    pub input_reports_received: u32,
    /// Number of reports forwarded to USB.
    pub reports_forwarded: u32,
    /// Source line of the most recent panic, or zero if none was recorded.
    ///
    /// A panic resets the board into the UF2 bootloader, which is the right
    /// recovery but erases every other signal: the board simply reappears as a
    /// UF2 volume with no explanation. Carrying the line across the reset is
    /// what makes a crash diagnosable instead of merely observable.
    pub panic_line: u16,
    /// Number of panics recorded since the record was last cleared.
    pub panic_count: u8,
    /// How far GATT discovery got before it stopped, or zero if not discovering.
    ///
    /// Discovery walks a service, a report map, a characteristic list, a
    /// descriptor per report, and finally a subscription. Half of those steps
    /// report the same "not found" when they fail, so the error alone does not
    /// say which one gave up. Byte twenty-seven was the only spare in the
    /// block, and this is what it is worth spending on.
    pub discovery_step: u8,
}

/// Error returned while decoding a diagnostics payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticsReportError {
    /// Payload is not exactly 32 bytes.
    Length(usize),
    /// Protocol version is unsupported.
    Version(u8),
    /// Payload is not the diagnostics status kind.
    Kind(u8),
    /// Bridge state is not one of the defined values.
    State(u8),
    /// CRC suffix does not match bytes 0 through 27.
    Crc {
        /// CRC encoded by the sender.
        expected: u32,
        /// CRC computed over the payload.
        actual: u32,
    },
}

/// Encodes the current status into the configuration HID payload.
pub fn encode(report: &DiagnosticsReport) -> [u8; DIAGNOSTICS_REPORT_LEN] {
    let mut bytes = [0; DIAGNOSTICS_REPORT_LEN];
    bytes[0] = CONFIG_PROTOCOL_VERSION;
    bytes[1] = STATUS_KIND;
    bytes[2] = report.state as u8;
    bytes[3] = report.last_error;
    bytes[4..6].copy_from_slice(&report.advertisements_seen.to_le_bytes());
    bytes[6..8].copy_from_slice(&report.hid_advertisements.to_le_bytes());
    bytes[8..14].copy_from_slice(&report.last_address);
    bytes[14] = report.last_rssi as u8;
    bytes[15] = report.connection_count;
    bytes[16..20].copy_from_slice(&report.input_reports_received.to_le_bytes());
    bytes[20..24].copy_from_slice(&report.reports_forwarded.to_le_bytes());
    bytes[24..26].copy_from_slice(&report.panic_line.to_le_bytes());
    bytes[26] = report.panic_count;
    bytes[27] = report.discovery_step;
    let crc = crc32_ieee(&bytes[..28]);
    bytes[28..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

/// Everything the bridge knows about pairing and the report path.
///
/// Grouped rather than passed as nine arguments, which is both unreadable and
/// easy to transpose: two adjacent `u8` reasons swapped at a call site would
/// compile and quietly mislabel every failure.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SecurityReport {
    /// Passkey currently displayed, if any.
    pub passkey: u32,
    /// How many passkeys this boot has produced.
    pub passkey_serial: u32,
    /// HCI status the peer gave when it dropped the link.
    pub disconnect_reason: u8,
    /// Reason the security manager reported a pairing failure.
    pub pairing_failure: u8,
    /// Bit field of `BOND_FLAG_*`.
    pub bond_flags: u8,
    /// Why the bridge declined to subscribe.
    pub hogp_refusal: u8,
    /// Why the last notification was not forwarded.
    pub notify_refusal: u8,
    /// Handle the last notification arrived on.
    pub notify_handle: u16,
    /// Handle the bridge subscribed to.
    pub notify_expected: u16,
    /// Which flash operation refused while storing the bond, and why.
    pub bond_flash_error: u8,
    /// Raw MPSL errno behind that refusal, magnitude only.
    pub bond_flash_errno: u16,
    /// Why the USB half declined to subscribe, on its own state machine.
    pub usb_hogp_refusal: u8,
    /// Whether attaching a new link to the report path failed.
    pub link_setup_failed: bool,
    /// How many times the stuck-key watchdog released the host.
    ///
    /// Saturating rather than wrapping: "at least 255" is still the truth,
    /// while a counter that returned to zero would read as a healthy run.
    pub stuck_key_releases: u8,
    /// How many liveness probes went unanswered while a key was held.
    pub liveness_probe_failures: u8,
    /// Why the last injected report was refused, or zero if it was forwarded.
    ///
    /// The injection tool can only report that it handed the packet over. What
    /// the bridge did with it was invisible, and an injection silently dropped
    /// for a detached source is what hid the stuck-key watchdog's own test.
    pub inject_refusal: u8,
    /// Why a liveness probe last failed, or zero if none has.
    ///
    /// Kept after the link recovers. The counter beside it says whether the
    /// reason is current; erasing it on recovery would leave a release with no
    /// account of what caused it.
    pub liveness_probe_error: u8,
}

/// Encodes the pairing and report-path status.
pub fn encode_passkey(report: &SecurityReport) -> [u8; DIAGNOSTICS_REPORT_LEN] {
    let mut bytes = [0; DIAGNOSTICS_REPORT_LEN];
    bytes[0] = CONFIG_PROTOCOL_VERSION;
    bytes[1] = PASSKEY_KIND;
    bytes[2] = report.disconnect_reason;
    bytes[3] = report.pairing_failure;
    bytes[4..8].copy_from_slice(&report.passkey.to_le_bytes());
    bytes[8..12].copy_from_slice(&report.passkey_serial.to_le_bytes());
    bytes[12] = report.bond_flags;
    bytes[13] = report.hogp_refusal;
    bytes[14] = report.notify_refusal;
    bytes[15] = report.bond_flash_error;
    bytes[20..22].copy_from_slice(&report.bond_flash_errno.to_le_bytes());
    bytes[22] = report.usb_hogp_refusal;
    bytes[23] = u8::from(report.link_setup_failed);
    bytes[16..18].copy_from_slice(&report.notify_handle.to_le_bytes());
    bytes[18..20].copy_from_slice(&report.notify_expected.to_le_bytes());
    bytes[24] = report.stuck_key_releases;
    bytes[25] = report.liveness_probe_failures;
    bytes[26] = report.liveness_probe_error;
    bytes[27] = report.inject_refusal;
    let crc = crc32_ieee(&bytes[..28]);
    bytes[28..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

/// Encodes one slice of the recorded panic message.
///
/// Layout: version, kind, chunk index, byte count, then the bytes, then the
/// CRC. The host asks for a slice before reading, because the configuration
/// interface has one report ID and the message does not fit in one report.
pub fn encode_panic_text(chunk: u8, text: &[u8]) -> [u8; DIAGNOSTICS_REPORT_LEN] {
    let mut bytes = [0; DIAGNOSTICS_REPORT_LEN];
    bytes[0] = CONFIG_PROTOCOL_VERSION;
    bytes[1] = PANIC_TEXT_KIND;
    bytes[2] = chunk;
    let len = text.len().min(PANIC_TEXT_CHUNK_LEN);
    bytes[3] = len as u8;
    bytes[4..4 + len].copy_from_slice(&text[..len]);
    let crc = crc32_ieee(&bytes[..28]);
    bytes[28..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

/// Decodes and CRC-validates a diagnostics payload.
pub fn decode(bytes: &[u8]) -> Result<DiagnosticsReport, DiagnosticsReportError> {
    if bytes.len() != DIAGNOSTICS_REPORT_LEN {
        return Err(DiagnosticsReportError::Length(bytes.len()));
    }
    if bytes[0] != CONFIG_PROTOCOL_VERSION {
        return Err(DiagnosticsReportError::Version(bytes[0]));
    }
    if bytes[1] != STATUS_KIND {
        return Err(DiagnosticsReportError::Kind(bytes[1]));
    }
    let state =
        BridgeState::try_from(bytes[2]).map_err(|_| DiagnosticsReportError::State(bytes[2]))?;
    let expected = u32::from_le_bytes(bytes[28..].try_into().expect("validated CRC length"));
    let actual = crc32_ieee(&bytes[..28]);
    if expected != actual {
        return Err(DiagnosticsReportError::Crc { expected, actual });
    }
    Ok(DiagnosticsReport {
        state,
        last_error: bytes[3],
        advertisements_seen: u16::from_le_bytes(
            bytes[4..6].try_into().expect("validated field length"),
        ),
        hid_advertisements: u16::from_le_bytes(
            bytes[6..8].try_into().expect("validated field length"),
        ),
        last_address: bytes[8..14].try_into().expect("validated address length"),
        last_rssi: bytes[14] as i8,
        connection_count: bytes[15],
        input_reports_received: u32::from_le_bytes(
            bytes[16..20].try_into().expect("validated field length"),
        ),
        reports_forwarded: u32::from_le_bytes(
            bytes[20..24].try_into().expect("validated field length"),
        ),
        panic_line: u16::from_le_bytes(bytes[24..26].try_into().expect("validated field length")),
        panic_count: bytes[26],
        discovery_step: bytes[27],
    })
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    #[test]
    fn a_full_panic_text_chunk_survives_the_crc() {
        // Every byte distinct, so a byte lost to the CRC cannot be mistaken for
        // a neighbour. The two bytes at the end of each chunk were being
        // overwritten, which surfaced as garbage inside the recorded panic
        // message and was read as the controller sending a corrupt string.
        let text: [u8; PANIC_TEXT_CHUNK_LEN] =
            core::array::from_fn(|index| u8::try_from(index).expect("chunk is shorter than 256"));

        let block = encode_panic_text(3, &text);

        assert_eq!(block[1], PANIC_TEXT_KIND);
        assert_eq!(block[2], 3);
        assert_eq!(usize::from(block[3]), PANIC_TEXT_CHUNK_LEN);
        assert_eq!(&block[4..4 + PANIC_TEXT_CHUNK_LEN], &text);
    }

    #[test]
    fn a_panic_text_chunk_carries_a_valid_crc() {
        let block = encode_panic_text(0, b"SoftdeviceController: ");

        let expected = u32::from_le_bytes(block[28..].try_into().expect("four-byte CRC suffix"));
        assert_eq!(expected, crc32_ieee(&block[..28]));
    }

    #[test]
    fn the_security_block_carries_the_stuck_key_watchdog() {
        // The watchdog fires on hardware, in the middle of typing, and leaves
        // no other trace: the keys are simply released. Without a counter the
        // only evidence would be the user noticing, which is exactly the
        // situation this is supposed to end. Byte positions are asserted
        // because `read_diagnostics.py` decodes them by offset.
        let report = SecurityReport {
            stuck_key_releases: 3,
            liveness_probe_failures: 2,
            liveness_probe_error: 0x09,
            inject_refusal: 2,
            ..SecurityReport::default()
        };

        let block = encode_passkey(&report);

        assert_eq!(block[24], 3);
        assert_eq!(block[25], 2);
        assert_eq!(block[26], 0x09);
        assert_eq!(block[27], 2);
        let expected = u32::from_le_bytes(block[28..].try_into().expect("four-byte CRC suffix"));
        assert_eq!(expected, crc32_ieee(&block[..28]));
    }

    #[test]
    fn the_identity_block_uses_the_fixed_wire_offsets() {
        let report = IdentityReport {
            major: 1,
            minor: 2,
            patch: 3,
            build_id: 0x1a2b_3c4d,
            dirty: true,
        };

        let block = encode_identity(&report);

        assert_eq!(block[0], CONFIG_PROTOCOL_VERSION);
        assert_eq!(block[1], IDENTITY_KIND);
        assert_eq!(block[2], 1);
        assert_eq!(block[3], 2);
        assert_eq!(block[4], 3);
        assert_eq!(&block[5..9], &[0x4d, 0x3c, 0x2b, 0x1a]);
        assert_eq!(block[9], 1);
        assert!(block[10..28].iter().all(|byte| *byte == 0));
        let expected = u32::from_le_bytes(block[28..].try_into().expect("four-byte CRC suffix"));
        assert_eq!(expected, crc32_ieee(&block[..28]));
    }

    #[test]
    fn the_transfer_block_uses_the_fixed_wire_offsets() {
        let report = TransferReport {
            state: crate::config_transfer::TransferState::Failed,
            last_error: 6,
            target: 1,
            expected_len: 0x1234,
            received_len: 0x5678,
            next_index: 0x9abc,
            declared_crc: 0x1122_3344,
            computed_crc: 0x5566_7788,
        };

        let block = encode_transfer(&report);

        assert_eq!(block[0], CONFIG_PROTOCOL_VERSION);
        assert_eq!(block[1], TRANSFER_KIND);
        assert_eq!(block[2], 3);
        assert_eq!(block[3], 6);
        assert_eq!(block[4], 1);
        assert_eq!(block[5], 0);
        assert_eq!(&block[6..8], &[0x34, 0x12]);
        assert_eq!(&block[8..10], &[0x78, 0x56]);
        assert_eq!(&block[10..12], &[0xbc, 0x9a]);
        assert_eq!(&block[12..16], &[0x44, 0x33, 0x22, 0x11]);
        assert_eq!(&block[16..20], &[0x88, 0x77, 0x66, 0x55]);
        assert!(block[20..28].iter().all(|byte| *byte == 0));
        let expected = u32::from_le_bytes(block[28..].try_into().expect("four-byte CRC suffix"));
        assert_eq!(expected, crc32_ieee(&block[..28]));
    }

    #[test]
    fn the_profile_block_uses_the_fixed_wire_offsets() {
        let report = ProfileReport {
            profile: crate::profile_store::StoredProfile {
                us_to_jis: true,
                caps_to_ctrl: true,
                swap_alt_gui: true,
            },
            stored: true,
            pending: true,
        };

        let block = encode_profile_block(&report);

        assert_eq!(block[0], CONFIG_PROTOCOL_VERSION);
        assert_eq!(block[1], PROFILE_KIND);
        assert_eq!(block[2], 1);
        assert_eq!(block[3], 0x07);
        assert_eq!(block[4], 1);
        assert_eq!(block[5], 1);
        assert!(block[6..28].iter().all(|byte| *byte == 0));
        let expected = u32::from_le_bytes(block[28..].try_into().expect("four-byte CRC suffix"));
        assert_eq!(expected, crc32_ieee(&block[..28]));
    }

    #[test]
    fn the_keymap_block_carries_chunk_metadata_and_a_valid_crc() {
        let payload: [u8; 23] = core::array::from_fn(|index| index as u8);

        let block = encode_keymap_chunk(&payload, 1).expect("second chunk exists");

        assert_eq!(block[0], CONFIG_PROTOCOL_VERSION);
        assert_eq!(block[1], KEYMAP_KIND);
        assert_eq!(block[2], KEYMAP_REPORT_VERSION);
        assert_eq!(block[3], 1);
        assert_eq!(block[4], 2);
        assert_eq!(&block[5..7], &[23, 0]);
        assert_eq!(
            &block[7..28],
            &[
                21, 22, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        );
        let expected = u32::from_le_bytes(block[28..].try_into().expect("four-byte CRC suffix"));
        assert_eq!(expected, crc32_ieee(&block[..28]));
        assert!(encode_keymap_chunk(&payload, 2).is_none());
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let report = DiagnosticsReport {
            state: BridgeState::Subscribed,
            last_error: 9,
            advertisements_seen: 0x1234,
            hid_advertisements: 0x5678,
            last_address: [1, 2, 3, 4, 5, 6],
            last_rssi: -72,
            connection_count: 3,
            input_reports_received: 0x1020_3040,
            reports_forwarded: 0x5060_7080,
            panic_line: 0x0bad,
            panic_count: 7,
            discovery_step: 5,
        };
        assert_eq!(decode(&encode(&report)), Ok(report));
    }

    #[test]
    fn corrupted_byte_fails_crc() {
        let report = DiagnosticsReport {
            state: BridgeState::Starting,
            last_error: 0,
            advertisements_seen: 0,
            hid_advertisements: 0,
            last_address: [0; 6],
            last_rssi: 0,
            connection_count: 0,
            input_reports_received: 0,
            reports_forwarded: 0,
            panic_line: 0,
            panic_count: 0,
            discovery_step: 5,
        };
        let mut bytes = encode(&report);
        bytes[16] ^= 1;
        assert!(matches!(
            decode(&bytes),
            Err(DiagnosticsReportError::Crc { .. })
        ));
    }
}
