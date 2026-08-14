//! Factory-compatible vendor configuration HID interface.
//!
//! The report descriptor and the accept/reject decision live here so both the
//! USB-only milestone binary and the S140 bridge share one host-tested
//! definition. Only the side effect — resetting the MCU — stays in the binary.

use crate::diagnostics_report::KEYMAP_CHUNK_COUNT_MAX;
use crate::uf2_reset::{
    CONFIG_REPORT_ID, CONFIG_REPORT_LEN, INJECTED_REPORT_LEN, RESET_INTO_BOOTSEL, ResetReportError,
    UKF_INJECT_SOURCE_REPORT, UKF_SELECT_KEYMAP, UKF_SELECT_PANIC_CHUNK, UKF_SELECT_SOURCE,
    UKF_SET_PAIRING_METHOD, UKF_SET_PAIRING_MODE, UKF_WRITE_BEGIN, UKF_WRITE_CHUNK,
    UKF_WRITE_COMMIT, normalize_reset_report, parse_config_report,
};
use ukf_core::SOURCE_SLOT_COUNT;

/// HID report descriptor for the vendor configuration interface.
///
/// The vendor usage page, usage, report ID, and 32-byte feature report match
/// the factory firmware so the existing hidapi and WebHID tools keep selecting
/// exactly one configuration interface.
// A HID report descriptor is only auditable when each item stays on one line
// with its meaning beside it.
#[rustfmt::skip]
pub const CONFIG_REPORT_DESCRIPTOR: &[u8] = &[
    0x06, 0x00, 0xff, // Usage Page (Vendor Defined 0xff00)
    0x09, 0x20, // Usage (0x20), selected by the legacy hidapi/WebHID tools
    0xa1, 0x01, // Collection (Application)
    0x85, CONFIG_REPORT_ID, // Report ID (100)
    0x15, 0x00, // Logical Minimum (0)
    0x26, 0xff, 0x00, // Logical Maximum (255)
    0x75, 0x08, // Report Size (8 bits)
    0x95, CONFIG_REPORT_LEN as u8, // Report Count (32)
    0x09, 0x20, // Usage (0x20)
    0xb1, 0x02, // Feature (Data, Variable, Absolute)
    0xc0, // End Collection
];

/// Largest SET_REPORT payload the configuration interface has to buffer.
///
/// hidapi on Windows prefixes the report ID to the feature-report buffer, so
/// the control endpoint must accept one more byte than the canonical payload.
pub const CONFIG_CONTROL_BUFFER_LEN: usize = CONFIG_REPORT_LEN + 1;

/// Decision produced by one observed configuration control request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigRequest {
    /// A valid factory-compatible bootloader reset command was received.
    ResetIntoBootloader,
    /// A source report to push through the bridge pipeline as if a keyboard
    /// had sent it. Used to exercise the conversion on hardware before a BLE
    /// keyboard is connected.
    InjectSourceReport([u8; INJECTED_REPORT_LEN]),
    /// Selects which slice of the recorded panic message to read next.
    SelectPanicChunk(u8),
    /// Open or close the bridge to keyboards it has not bonded with.
    SetPairingMode(bool),
    /// Require a passkey for the next pairing, rather than Just Works.
    SetPairingMethod {
        /// True to require passkey entry, false for Just Works.
        passkey: bool,
    },
    /// Starts a fixed-capacity configuration transfer.
    WriteBegin {
        /// Destination selected by the transfer protocol.
        target: u8,
        /// Number of payload bytes expected.
        total_len: u16,
        /// CRC declared for the complete payload.
        payload_crc: u32,
    },
    /// Carries one chunk of a fixed-capacity configuration transfer.
    WriteChunk {
        /// Sequential chunk index.
        index: u16,
        /// Fixed-width chunk storage from the feature report.
        data: [u8; 23],
        /// Number of meaningful bytes in `data`.
        count: u8,
    },
    /// Validates and commits a fixed-capacity configuration transfer.
    WriteCommit {
        /// Destination selected by the transfer protocol.
        target: u8,
        /// Number of payload bytes expected.
        total_len: u16,
        /// CRC declared for the complete payload.
        payload_crc: u32,
    },
    /// Selects the source slot returned by the next diagnostics read.
    SelectSource(u8),
    /// Selects the keymap payload chunk returned by the next diagnostics read.
    SelectKeymapChunk(u8),
    /// The report addressed this interface but failed validation.
    Rejected(ResetReportError),
}

/// Classifies one SET_REPORT(Feature) payload seen on the configuration interface.
///
/// `report_id` is the ID carried in the control request's `wValue`, and `data`
/// is the raw data stage. Both the canonical 32-byte payload and the
/// hidapi-prefixed 33-byte form are accepted; everything else is rejected so a
/// malformed or unrelated report can never reset the board.
pub fn classify_feature_report(report_id: u8, data: &[u8]) -> ConfigRequest {
    let frame = match normalize_reset_report(report_id, data)
        .and_then(|payload| parse_config_report(report_id, payload))
    {
        Ok(frame) => frame,
        Err(error) => return ConfigRequest::Rejected(error),
    };

    match frame.command {
        RESET_INTO_BOOTSEL => ConfigRequest::ResetIntoBootloader,
        UKF_SELECT_PANIC_CHUNK => ConfigRequest::SelectPanicChunk(frame.data[0]),
        UKF_SELECT_SOURCE => {
            if frame.data[1..].iter().any(|byte| *byte != 0) {
                ConfigRequest::Rejected(ResetReportError::Reserved)
            } else if usize::from(frame.data[0]) >= SOURCE_SLOT_COUNT {
                ConfigRequest::Rejected(ResetReportError::SourceSlot(frame.data[0]))
            } else {
                ConfigRequest::SelectSource(frame.data[0])
            }
        }
        UKF_SELECT_KEYMAP => {
            if frame.data[1..].iter().any(|byte| *byte != 0) {
                ConfigRequest::Rejected(ResetReportError::Reserved)
            } else if frame.data[0] >= KEYMAP_CHUNK_COUNT_MAX {
                ConfigRequest::Rejected(ResetReportError::KeymapChunk(frame.data[0]))
            } else {
                ConfigRequest::SelectKeymapChunk(frame.data[0])
            }
        }
        UKF_SET_PAIRING_MODE => ConfigRequest::SetPairingMode(frame.data[0] != 0),
        UKF_SET_PAIRING_METHOD => ConfigRequest::SetPairingMethod {
            passkey: frame.data[0] != 0,
        },
        UKF_WRITE_BEGIN => ConfigRequest::WriteBegin {
            target: frame.data[0],
            total_len: u16::from_le_bytes([frame.data[2], frame.data[3]]),
            payload_crc: u32::from_le_bytes([
                frame.data[4],
                frame.data[5],
                frame.data[6],
                frame.data[7],
            ]),
        },
        UKF_WRITE_CHUNK => {
            let count = usize::from(frame.data[2]);
            if count < 23 && frame.data[3 + count..26].iter().any(|byte| *byte != 0) {
                return ConfigRequest::Rejected(ResetReportError::Reserved);
            }
            let mut data = [0; 23];
            data.copy_from_slice(&frame.data[3..26]);
            ConfigRequest::WriteChunk {
                index: u16::from_le_bytes([frame.data[0], frame.data[1]]),
                data,
                count: frame.data[2],
            }
        }
        UKF_WRITE_COMMIT => ConfigRequest::WriteCommit {
            target: frame.data[0],
            total_len: u16::from_le_bytes([frame.data[2], frame.data[3]]),
            payload_crc: u32::from_le_bytes([
                frame.data[4],
                frame.data[5],
                frame.data[6],
                frame.data[7],
            ]),
        },
        UKF_INJECT_SOURCE_REPORT => {
            let mut report = [0; INJECTED_REPORT_LEN];
            report.copy_from_slice(&frame.data[..INJECTED_REPORT_LEN]);
            ConfigRequest::InjectSourceReport(report)
        }
        other => ConfigRequest::Rejected(ResetReportError::Command(other)),
    }
}
