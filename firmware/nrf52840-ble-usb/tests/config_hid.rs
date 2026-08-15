//! Host tests for the factory-compatible configuration HID interface.

use ukf_nrf52840_ble_usb::config_hid::{
    CONFIG_CONTROL_BUFFER_LEN, CONFIG_REPORT_DESCRIPTOR, ConfigRequest, classify_feature_report,
};
use ukf_nrf52840_ble_usb::uf2_reset::{
    CONFIG_PROTOCOL_VERSION, CONFIG_REPORT_ID, CONFIG_REPORT_LEN, INJECTED_REPORT_LEN,
    RESET_INTO_BOOTSEL, ResetReportError, UKF_COMMAND_BASE, UKF_INJECT_SOURCE_REPORT,
    UKF_SELECT_KEYMAP, UKF_SELECT_SOURCE, UKF_SELECT_SOURCE_KEYMAP, UKF_WRITE_BEGIN,
    UKF_WRITE_CHUNK, UKF_WRITE_COMMIT, crc32_ieee,
};

fn payload_for(command: u8, data: &[u8]) -> [u8; CONFIG_REPORT_LEN] {
    let mut payload = [0; CONFIG_REPORT_LEN];
    payload[0] = CONFIG_PROTOCOL_VERSION;
    payload[1] = command;
    payload[2..2 + data.len()].copy_from_slice(data);
    let crc = crc32_ieee(&payload[..CONFIG_REPORT_LEN - 4]);
    payload[CONFIG_REPORT_LEN - 4..].copy_from_slice(&crc.to_le_bytes());
    payload
}

fn valid_reset_payload() -> [u8; CONFIG_REPORT_LEN] {
    payload_for(RESET_INTO_BOOTSEL, &[])
}

#[test]
fn descriptor_declares_the_factory_vendor_page_usage_and_report_id() {
    // Usage Page (Vendor Defined 0xff00) then Usage (0x20): the exact pair the
    // legacy hidapi and WebHID tools use to select one configuration interface.
    assert_eq!(
        &CONFIG_REPORT_DESCRIPTOR[..5],
        &[0x06, 0x00, 0xff, 0x09, 0x20]
    );

    let report_id_index = CONFIG_REPORT_DESCRIPTOR
        .windows(2)
        .position(|pair| pair == [0x85, CONFIG_REPORT_ID])
        .expect("descriptor declares the factory configuration report ID");
    assert!(report_id_index > 0);

    // Report Size (8) and Report Count (32) must describe the 32-byte payload.
    assert!(
        CONFIG_REPORT_DESCRIPTOR
            .windows(4)
            .any(|window| window == [0x75, 0x08, 0x95, CONFIG_REPORT_LEN as u8])
    );

    // Feature (Data, Variable, Absolute) is the transport the reset command uses.
    assert!(
        CONFIG_REPORT_DESCRIPTOR
            .windows(2)
            .any(|pair| pair == [0xb1, 0x02])
    );
    assert_eq!(CONFIG_REPORT_DESCRIPTOR.last(), Some(&0xc0));
}

#[test]
fn control_buffer_holds_the_hidapi_prefixed_feature_report() {
    assert_eq!(CONFIG_CONTROL_BUFFER_LEN, CONFIG_REPORT_LEN + 1);
}

#[test]
fn a_canonical_reset_report_requests_the_bootloader() {
    let payload = valid_reset_payload();

    assert_eq!(
        classify_feature_report(CONFIG_REPORT_ID, &payload),
        ConfigRequest::ResetIntoBootloader
    );
}

#[test]
fn a_hidapi_prefixed_reset_report_requests_the_bootloader() {
    let payload = valid_reset_payload();
    let mut prefixed = [0; CONFIG_REPORT_LEN + 1];
    prefixed[0] = CONFIG_REPORT_ID;
    prefixed[1..].copy_from_slice(&payload);

    assert_eq!(
        classify_feature_report(CONFIG_REPORT_ID, &prefixed),
        ConfigRequest::ResetIntoBootloader
    );
}

#[test]
fn another_interfaces_report_id_never_resets_the_board() {
    let payload = valid_reset_payload();

    assert_eq!(
        classify_feature_report(CONFIG_REPORT_ID + 1, &payload),
        ConfigRequest::Rejected(ResetReportError::ReportId(CONFIG_REPORT_ID + 1))
    );
}

#[test]
fn integrity_is_checked_before_any_field_is_interpreted() {
    // Tampering with the command byte of an otherwise valid report must be
    // reported as corruption, not as an unsupported command. Interpreting a
    // field of a report that failed its integrity check is how a flipped bit
    // turns into an unintended action.
    let mut tampered = valid_reset_payload();
    tampered[1] = 0x7f;

    assert!(matches!(
        classify_feature_report(CONFIG_REPORT_ID, &tampered),
        ConfigRequest::Rejected(ResetReportError::Crc { .. })
    ));
}

#[test]
fn a_corrupt_crc_never_resets_the_board() {
    let mut payload = valid_reset_payload();
    payload[2] ^= 0xff;

    assert!(matches!(
        classify_feature_report(CONFIG_REPORT_ID, &payload),
        ConfigRequest::Rejected(ResetReportError::Crc { .. })
    ));
}

#[test]
fn an_unsupported_version_or_command_never_resets_the_board() {
    let mut wrong_version = valid_reset_payload();
    wrong_version[0] = CONFIG_PROTOCOL_VERSION.wrapping_add(1);
    assert_eq!(
        classify_feature_report(CONFIG_REPORT_ID, &wrong_version),
        ConfigRequest::Rejected(ResetReportError::Version(wrong_version[0]))
    );

    // The CRC is verified before any field is interpreted, so an unsupported
    // command has to be presented in an otherwise well-formed report to prove
    // the command check itself rejects it.
    let unsupported = RESET_INTO_BOOTSEL.wrapping_add(1);
    assert_eq!(
        classify_feature_report(CONFIG_REPORT_ID, &payload_for(unsupported, &[])),
        ConfigRequest::Rejected(ResetReportError::Command(unsupported))
    );
}

#[test]
fn an_injection_command_carries_the_source_report_through() {
    let source = [0x02, 0x00, 0x1f, 0x00, 0x00, 0x00, 0x00, 0x00];

    assert_eq!(
        classify_feature_report(
            CONFIG_REPORT_ID,
            &payload_for(UKF_INJECT_SOURCE_REPORT, &source)
        ),
        ConfigRequest::InjectSourceReport(source)
    );
}

#[test]
fn write_begin_uses_the_fixed_field_offsets() {
    let target = 1;
    let total_len: u16 = 0x1234;
    let payload_crc: u32 = 0x89ab_cdef;
    let mut data = [0; 26];
    data[0] = target;
    data[2..4].copy_from_slice(&total_len.to_le_bytes());
    data[4..8].copy_from_slice(&payload_crc.to_le_bytes());

    assert_eq!(
        classify_feature_report(CONFIG_REPORT_ID, &payload_for(UKF_WRITE_BEGIN, &data)),
        ConfigRequest::WriteBegin {
            target,
            total_len,
            payload_crc,
        }
    );
}

#[test]
fn write_chunk_uses_the_fixed_field_offsets() {
    let index: u16 = 0x1234;
    let count = 23;
    let data: [u8; 23] = core::array::from_fn(|byte| byte as u8);
    let mut fields = [0; 26];
    fields[0..2].copy_from_slice(&index.to_le_bytes());
    fields[2] = count;
    fields[3..].copy_from_slice(&data);

    assert_eq!(
        classify_feature_report(CONFIG_REPORT_ID, &payload_for(UKF_WRITE_CHUNK, &fields)),
        ConfigRequest::WriteChunk { index, data, count }
    );
}

#[test]
fn write_commit_uses_the_fixed_field_offsets() {
    let target = 1;
    let total_len: u16 = 0x4321;
    let payload_crc: u32 = 0x0123_4567;
    let mut data = [0; 26];
    data[0] = target;
    data[2..4].copy_from_slice(&total_len.to_le_bytes());
    data[4..8].copy_from_slice(&payload_crc.to_le_bytes());

    assert_eq!(
        classify_feature_report(CONFIG_REPORT_ID, &payload_for(UKF_WRITE_COMMIT, &data)),
        ConfigRequest::WriteCommit {
            target,
            total_len,
            payload_crc,
        }
    );
}

#[test]
fn write_commands_use_the_reserved_extension_slots() {
    assert_eq!(UKF_WRITE_BEGIN, UKF_COMMAND_BASE + 4);
    assert_eq!(UKF_WRITE_CHUNK, UKF_COMMAND_BASE + 5);
    assert_eq!(UKF_WRITE_COMMIT, UKF_COMMAND_BASE + 6);
}

#[test]
fn injection_uses_a_command_value_the_factory_firmware_has_not_allocated() {
    // The factory enum allocates 0..=50, with 40..=50 reserved for its
    // unimplemented keymap API. Our extensions must start above all of it so a
    // future factory command can never mean two things. Checked at compile
    // time: a runtime assertion on two constants can never fail usefully.
    const _: () = assert!(UKF_COMMAND_BASE > 50);
    assert_eq!(UKF_INJECT_SOURCE_REPORT, UKF_COMMAND_BASE);
    assert_eq!(INJECTED_REPORT_LEN, 8);
}

#[test]
fn source_selector_accepts_only_a_registered_slot_and_zero_reserved_bytes() {
    assert_eq!(
        classify_feature_report(CONFIG_REPORT_ID, &payload_for(UKF_SELECT_SOURCE, &[4])),
        ConfigRequest::SelectSource(4)
    );

    assert!(matches!(
        classify_feature_report(CONFIG_REPORT_ID, &payload_for(UKF_SELECT_SOURCE, &[5])),
        ConfigRequest::Rejected(ResetReportError::SourceSlot(5))
    ));

    let mut data = [0; 26];
    data[0] = 4;
    data[1] = 1;
    assert!(matches!(
        classify_feature_report(CONFIG_REPORT_ID, &payload_for(UKF_SELECT_SOURCE, &data)),
        ConfigRequest::Rejected(ResetReportError::Reserved)
    ));
}

#[test]
fn keymap_selector_carries_a_chunk_index_and_rejects_nonzero_reserved_bytes() {
    assert_eq!(
        classify_feature_report(CONFIG_REPORT_ID, &payload_for(UKF_SELECT_KEYMAP, &[2])),
        ConfigRequest::SelectKeymapChunk(2)
    );

    let mut data = [0; 26];
    data[0] = 2;
    data[1] = 1;
    assert!(matches!(
        classify_feature_report(CONFIG_REPORT_ID, &payload_for(UKF_SELECT_KEYMAP, &data)),
        ConfigRequest::Rejected(ResetReportError::Reserved)
    ));

    assert_eq!(
        classify_feature_report(CONFIG_REPORT_ID, &payload_for(UKF_SELECT_KEYMAP, &[7])),
        ConfigRequest::Rejected(ResetReportError::KeymapChunk(7))
    );
}

#[test]
fn source_keymap_selector_carries_slot_and_chunk_and_rejects_reserved_bytes() {
    assert_eq!(
        classify_feature_report(
            CONFIG_REPORT_ID,
            &payload_for(UKF_SELECT_SOURCE_KEYMAP, &[4, 2]),
        ),
        ConfigRequest::SelectSourceKeymapChunk { slot: 4, chunk: 2 }
    );

    let mut data = [0; 26];
    data[0] = 4;
    data[1] = 2;
    data[2] = 1;
    assert!(matches!(
        classify_feature_report(
            CONFIG_REPORT_ID,
            &payload_for(UKF_SELECT_SOURCE_KEYMAP, &data)
        ),
        ConfigRequest::Rejected(ResetReportError::Reserved)
    ));

    assert!(matches!(
        classify_feature_report(
            CONFIG_REPORT_ID,
            &payload_for(UKF_SELECT_SOURCE_KEYMAP, &[5, 0])
        ),
        ConfigRequest::Rejected(ResetReportError::SourceKeymapSlot(5))
    ));
}

#[test]
fn write_chunk_rejects_nonzero_unused_bytes_after_the_declared_count() {
    let mut fields = [0; 26];
    fields[2] = 1;
    fields[3] = 0x04;
    fields[4] = 0xff;

    assert!(matches!(
        classify_feature_report(CONFIG_REPORT_ID, &payload_for(UKF_WRITE_CHUNK, &fields)),
        ConfigRequest::Rejected(ResetReportError::Reserved)
    ));
}

#[test]
fn an_unallocated_command_is_rejected_rather_than_guessed() {
    let unknown = 7;

    assert_eq!(
        classify_feature_report(CONFIG_REPORT_ID, &payload_for(unknown, &[])),
        ConfigRequest::Rejected(ResetReportError::Command(unknown))
    );
}

#[test]
fn a_truncated_or_oversized_data_stage_never_resets_the_board() {
    let payload = valid_reset_payload();

    assert_eq!(
        classify_feature_report(CONFIG_REPORT_ID, &payload[..CONFIG_REPORT_LEN - 1]),
        ConfigRequest::Rejected(ResetReportError::Length(CONFIG_REPORT_LEN - 1))
    );

    let oversized = [0_u8; CONFIG_REPORT_LEN + 2];
    assert_eq!(
        classify_feature_report(CONFIG_REPORT_ID, &oversized),
        ConfigRequest::Rejected(ResetReportError::Length(CONFIG_REPORT_LEN + 2))
    );
}
