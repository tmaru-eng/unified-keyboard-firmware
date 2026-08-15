//! Validation for the factory-compatible UF2 bootloader reset feature report.

/// HID report ID used by the configuration interface.
pub const CONFIG_REPORT_ID: u8 = 100;
/// Number of payload bytes in the configuration feature report.
pub const CONFIG_REPORT_LEN: usize = 32;
/// Protocol version emitted by the factory firmware.
pub const CONFIG_PROTOCOL_VERSION: u8 = 18;
/// Command requesting a reset into the UF2 bootloader.
pub const RESET_INTO_BOOTSEL: u8 = 1;
/// First command value reserved for this repository's own extensions.
///
/// The factory firmware allocates `0` through `50`, with `40..=50` reserved for
/// its unimplemented keymap API. Starting at `100` leaves that whole range
/// free, so a future factory command can never collide with one of ours.
pub const UKF_COMMAND_BASE: u8 = 100;
/// Feeds one eight-byte source report through the bridge pipeline.
///
/// This exists so the conversion can be exercised on hardware before a BLE
/// keyboard is connected. The payload is the report the source would have
/// sent; the firmware applies the profile and emits the result over USB.
pub const UKF_INJECT_SOURCE_REPORT: u8 = UKF_COMMAND_BASE;
/// Number of payload bytes carried by an injected source report.
pub const INJECTED_REPORT_LEN: usize = 8;
/// Selects which slice of the recorded panic message the next read returns.
///
/// The message does not fit in one 32-byte report, and the configuration
/// interface has a single report ID, so the host names the slice it wants
/// before reading it.
pub const UKF_SELECT_PANIC_CHUNK: u8 = UKF_COMMAND_BASE + 1;
/// Opens the bridge to a keyboard it has never met.
///
/// Adopting a new keyboard is deliberate. Once one is bonded the bridge waits
/// for that one alone, so there has to be a way to say "a different keyboard
/// is coming" without reflashing. The data byte is non-zero to open pairing
/// and zero to close it again.
pub const UKF_SET_PAIRING_MODE: u8 = UKF_COMMAND_BASE + 2;
/// Chooses how the next pairing authenticates.
///
/// Data byte zero selects Just Works, one selects passkey entry. Just Works
/// offers no protection against someone in radio range completing the pairing
/// in the keyboard's place, which for a device that carries keystrokes is not
/// a defensible default — it is a bring-up convenience.
pub const UKF_SET_PAIRING_METHOD: u8 = UKF_COMMAND_BASE + 3;
/// Begins a fixed-capacity configuration transfer.
pub const UKF_WRITE_BEGIN: u8 = UKF_COMMAND_BASE + 4;
/// Carries one chunk of a fixed-capacity configuration transfer.
pub const UKF_WRITE_CHUNK: u8 = UKF_COMMAND_BASE + 5;
/// Validates and commits a fixed-capacity configuration transfer.
pub const UKF_WRITE_COMMIT: u8 = UKF_COMMAND_BASE + 6;
/// Selects the source slot returned by the next diagnostics read.
pub const UKF_SELECT_SOURCE: u8 = UKF_COMMAND_BASE + 7;
/// Selects a keymap payload chunk returned by the next diagnostics read.
pub const UKF_SELECT_KEYMAP: u8 = UKF_COMMAND_BASE + 8;
/// Selects a source-slot keymap payload chunk returned by the next read.
pub const UKF_SELECT_SOURCE_KEYMAP: u8 = UKF_COMMAND_BASE + 9;
/// GPREGRET value recognized by the UF2 bootloader.
pub const UF2_RESET_MAGIC: u8 = 0x57;

const CRC_OFFSET: usize = CONFIG_REPORT_LEN - size_of::<u32>();

/// A validated configuration command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResetCommand {
    /// Reset into the board's UF2 bootloader.
    IntoBootloader,
}

/// Reason a configuration feature report was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResetReportError {
    /// The control request or report-ID prefix used an unexpected HID report ID.
    ReportId(u8),
    /// The raw feature report or canonical payload had an unsupported length.
    Length(usize),
    /// The report used an unsupported protocol version.
    Version(u8),
    /// The report did not request the supported bootloader reset command.
    Command(u8),
    /// The transmitted CRC did not match the computed CRC.
    Crc {
        /// CRC encoded in the final four bytes of the report.
        expected: u32,
        /// CRC computed over the first 28 bytes of the report.
        actual: u32,
    },
    /// A command carried non-zero reserved bytes.
    Reserved,
    /// A source selector named a slot outside the fixed source capacity.
    SourceSlot(u8),
    /// A keymap selector named a chunk outside the fixed diagnostics capacity.
    KeymapChunk(u8),
    /// A source-keymap selector named a slot outside the fixed source capacity.
    SourceKeymapSlot(u8),
}

/// Computes reflected CRC-32/ISO-HDLC (also called CRC-32/IEEE).
pub fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let reflected_polynomial = 0xedb8_8320 & (0_u32.wrapping_sub(crc & 1));
            crc = (crc >> 1) ^ reflected_polynomial;
        }
    }
    !crc
}

/// Normalizes one raw HID feature report to its 32-byte configuration payload.
///
/// `usbd-hid` returns the report ID separately for a control transfer, while
/// hidapi includes that ID as the first byte of the packet it sends. Accept
/// both wire representations, but require the ID in either representation to
/// be the factory-compatible configuration report ID.
pub fn normalize_reset_report(report_id: u8, report: &[u8]) -> Result<&[u8], ResetReportError> {
    if report_id != CONFIG_REPORT_ID {
        return Err(ResetReportError::ReportId(report_id));
    }

    match report.len() {
        CONFIG_REPORT_LEN => Ok(report),
        len if len == CONFIG_REPORT_LEN + 1 => {
            if report[0] != report_id {
                return Err(ResetReportError::ReportId(report[0]));
            }
            Ok(&report[1..])
        }
        len => Err(ResetReportError::Length(len)),
    }
}

/// One validated configuration report: its command and its data bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigFrame<'a> {
    /// Command byte selecting what the report asks for.
    pub command: u8,
    /// Bytes between the command and the CRC suffix.
    pub data: &'a [u8],
}

/// Validates the envelope of one canonical 32-byte configuration payload.
///
/// Checks the report ID, length, protocol version, and CRC, then hands back the
/// command and its data without interpreting either. Every command shares this
/// envelope, so a malformed report is rejected in one place rather than once
/// per command.
pub fn parse_config_report(
    report_id: u8,
    payload: &[u8],
) -> Result<ConfigFrame<'_>, ResetReportError> {
    if report_id != CONFIG_REPORT_ID {
        return Err(ResetReportError::ReportId(report_id));
    }
    if payload.len() != CONFIG_REPORT_LEN {
        return Err(ResetReportError::Length(payload.len()));
    }
    if payload[0] != CONFIG_PROTOCOL_VERSION {
        return Err(ResetReportError::Version(payload[0]));
    }

    let expected = u32::from_le_bytes(
        payload[CRC_OFFSET..]
            .try_into()
            .expect("validated feature report has a four-byte CRC suffix"),
    );
    let actual = crc32_ieee(&payload[..CRC_OFFSET]);
    if actual != expected {
        return Err(ResetReportError::Crc { expected, actual });
    }

    Ok(ConfigFrame {
        command: payload[1],
        data: &payload[2..CRC_OFFSET],
    })
}

/// Validates one canonical 32-byte HID configuration feature payload as a
/// bootloader reset request.
///
/// Raw SET_REPORT data should first be passed to [`normalize_reset_report`].
/// The caller supplies the report ID separately because USB HID control
/// transfers return it in [`ReportInfo`](https://docs.rs/usbd-hid/latest/usbd_hid/hid_class/struct.ReportInfo.html).
pub fn parse_reset_report(report_id: u8, payload: &[u8]) -> Result<ResetCommand, ResetReportError> {
    let frame = parse_config_report(report_id, payload)?;
    if frame.command != RESET_INTO_BOOTSEL {
        return Err(ResetReportError::Command(frame.command));
    }
    Ok(ResetCommand::IntoBootloader)
}
