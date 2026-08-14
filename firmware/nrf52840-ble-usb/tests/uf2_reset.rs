//! Factory-compatible UF2 reset feature-report contract tests.

use ukf_nrf52840_ble_usb::uf2_reset::{
    CONFIG_PROTOCOL_VERSION, CONFIG_REPORT_ID, CONFIG_REPORT_LEN, ResetCommand, ResetReportError,
    crc32_ieee, normalize_reset_report, parse_reset_report,
};

const VALID_RESET_REPORT: [u8; CONFIG_REPORT_LEN] = [
    18, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x42,
    0x47, 0xc8, 0xf6,
];

/// Builds a well-formed report carrying an arbitrary command.
///
/// The CRC is verified before any field is interpreted, so a test that wants
/// the command check to fire has to present a report whose integrity is intact.
fn report_with_command(command: u8) -> [u8; CONFIG_REPORT_LEN] {
    let mut report = [0; CONFIG_REPORT_LEN];
    report[0] = CONFIG_PROTOCOL_VERSION;
    report[1] = command;
    let crc = crc32_ieee(&report[..CONFIG_REPORT_LEN - 4]);
    report[CONFIG_REPORT_LEN - 4..].copy_from_slice(&crc.to_le_bytes());
    report
}

#[test]
fn crc32_matches_the_ieee_check_value() {
    assert_eq!(crc32_ieee(b"123456789"), 0xcbf4_3926);
}

#[test]
fn accepts_only_the_factory_compatible_bootloader_reset_report() {
    assert_eq!(
        parse_reset_report(CONFIG_REPORT_ID, &VALID_RESET_REPORT),
        Ok(ResetCommand::IntoBootloader)
    );
}

#[test]
fn normalizer_accepts_native_and_hidapi_prefixed_reports() {
    assert_eq!(
        normalize_reset_report(CONFIG_REPORT_ID, &VALID_RESET_REPORT),
        Ok(&VALID_RESET_REPORT[..])
    );

    let mut prefixed = [0_u8; CONFIG_REPORT_LEN + 1];
    prefixed[0] = CONFIG_REPORT_ID;
    prefixed[1..].copy_from_slice(&VALID_RESET_REPORT);
    assert_eq!(
        normalize_reset_report(CONFIG_REPORT_ID, &prefixed),
        Ok(&prefixed[1..])
    );
    assert_eq!(
        normalize_reset_report(CONFIG_REPORT_ID, &prefixed)
            .and_then(|payload| parse_reset_report(CONFIG_REPORT_ID, payload)),
        Ok(ResetCommand::IntoBootloader)
    );
}

#[test]
fn normalizer_rejects_wrong_report_id_or_prefix() {
    assert_eq!(
        normalize_reset_report(CONFIG_REPORT_ID - 1, &VALID_RESET_REPORT),
        Err(ResetReportError::ReportId(CONFIG_REPORT_ID - 1))
    );

    let mut prefixed = [0_u8; CONFIG_REPORT_LEN + 1];
    prefixed[0] = CONFIG_REPORT_ID - 1;
    prefixed[1..].copy_from_slice(&VALID_RESET_REPORT);
    assert_eq!(
        normalize_reset_report(CONFIG_REPORT_ID, &prefixed),
        Err(ResetReportError::ReportId(CONFIG_REPORT_ID - 1))
    );
}

#[test]
fn normalizer_rejects_non_wire_lengths() {
    let short = [0_u8; CONFIG_REPORT_LEN - 1];
    assert_eq!(
        normalize_reset_report(CONFIG_REPORT_ID, &short),
        Err(ResetReportError::Length(CONFIG_REPORT_LEN - 1))
    );

    let long = [0_u8; CONFIG_REPORT_LEN + 2];
    assert_eq!(
        normalize_reset_report(CONFIG_REPORT_ID, &long),
        Err(ResetReportError::Length(CONFIG_REPORT_LEN + 2))
    );
}

#[test]
fn prefixed_reports_keep_parser_validation() {
    let mut prefixed = [0_u8; CONFIG_REPORT_LEN + 1];
    prefixed[0] = CONFIG_REPORT_ID;
    prefixed[1..].copy_from_slice(&VALID_RESET_REPORT);

    prefixed[1] = 17;
    assert_eq!(
        normalize_reset_report(CONFIG_REPORT_ID, &prefixed)
            .and_then(|payload| parse_reset_report(CONFIG_REPORT_ID, payload)),
        Err(ResetReportError::Version(17))
    );

    let unsupported = report_with_command(0);
    let mut prefixed_unsupported = [0_u8; CONFIG_REPORT_LEN + 1];
    prefixed_unsupported[0] = CONFIG_REPORT_ID;
    prefixed_unsupported[1..].copy_from_slice(&unsupported);
    assert_eq!(
        normalize_reset_report(CONFIG_REPORT_ID, &prefixed_unsupported)
            .and_then(|payload| parse_reset_report(CONFIG_REPORT_ID, payload)),
        Err(ResetReportError::Command(0))
    );

    prefixed[1] = CONFIG_PROTOCOL_VERSION;
    prefixed[28] ^= 1;
    assert!(matches!(
        normalize_reset_report(CONFIG_REPORT_ID, &prefixed)
            .and_then(|payload| parse_reset_report(CONFIG_REPORT_ID, payload)),
        Err(ResetReportError::Crc { .. })
    ));
}

#[test]
fn rejects_the_wrong_report_id() {
    assert_eq!(
        parse_reset_report(CONFIG_REPORT_ID - 1, &VALID_RESET_REPORT),
        Err(ResetReportError::ReportId(CONFIG_REPORT_ID - 1))
    );
}

#[test]
fn rejects_every_wrong_payload_length() {
    assert_eq!(
        parse_reset_report(
            CONFIG_REPORT_ID,
            &VALID_RESET_REPORT[..CONFIG_REPORT_LEN - 1]
        ),
        Err(ResetReportError::Length(CONFIG_REPORT_LEN - 1))
    );

    let oversized = [0_u8; CONFIG_REPORT_LEN + 1];
    assert_eq!(
        parse_reset_report(CONFIG_REPORT_ID, &oversized),
        Err(ResetReportError::Length(CONFIG_REPORT_LEN + 1))
    );
}

#[test]
fn rejects_an_unsupported_protocol_version() {
    let mut report = VALID_RESET_REPORT;
    report[0] = 17;

    assert_eq!(
        parse_reset_report(CONFIG_REPORT_ID, &report),
        Err(ResetReportError::Version(17))
    );
}

#[test]
fn rejects_a_non_bootloader_command() {
    assert_eq!(
        parse_reset_report(CONFIG_REPORT_ID, &report_with_command(0)),
        Err(ResetReportError::Command(0))
    );
}

#[test]
fn rejects_a_corrupt_crc() {
    let mut report = VALID_RESET_REPORT;
    report[27] = 1;

    assert_eq!(
        parse_reset_report(CONFIG_REPORT_ID, &report),
        Err(ResetReportError::Crc {
            expected: 0xf6c8_4742,
            actual: 0x81cf_77d4,
        })
    );
}
