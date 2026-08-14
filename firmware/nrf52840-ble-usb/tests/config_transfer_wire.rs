//! The frames the UI sends have to be the frames the firmware parses.
//!
//! Both sides write these offsets by hand — the firmware in `config_hid.rs`,
//! the UI in `ui/src/transfer.ts`. Every test on either side of that boundary
//! agrees with itself by construction; only a test that builds a frame the way
//! one side does and parses it the way the other does can catch them drifting
//! apart. The failure that shape produces is the worst kind available here: a
//! field lands one byte over, nothing errors, and the board acts on a number
//! nobody sent.
//!
//! When this file and `ui/src/transfer.test.ts` disagree, the firmware is the
//! authority and the UI is the bug.

use ukf_nrf52840_ble_usb::config_hid::{ConfigRequest, classify_feature_report};
use ukf_nrf52840_ble_usb::config_transfer::{ConfigTransfer, TARGET_SCRATCH, TransferState};
use ukf_nrf52840_ble_usb::uf2_reset::{CONFIG_PROTOCOL_VERSION, CONFIG_REPORT_ID, crc32_ieee};

const REPORT_LEN: usize = 32;
/// Payload bytes one frame carries, as `ui/src/transfer.ts` computes it.
const CHUNK_BYTES: usize = 23;

const UKF_WRITE_BEGIN: u8 = 104;
const UKF_WRITE_CHUNK: u8 = 105;
const UKF_WRITE_COMMIT: u8 = 106;

fn sealed(mut report: [u8; REPORT_LEN]) -> [u8; REPORT_LEN] {
    let crc = crc32_ieee(&report[..28]);
    report[28..].copy_from_slice(&crc.to_le_bytes());
    report
}

/// Builds a declaration exactly as `buildWriteBegin`/`buildWriteCommit` do.
fn declaration(command: u8, target: u8, payload: &[u8]) -> [u8; REPORT_LEN] {
    let mut report = [0; REPORT_LEN];
    report[0] = CONFIG_PROTOCOL_VERSION;
    report[1] = command;
    report[2] = target;
    report[4..6].copy_from_slice(&(payload.len() as u16).to_le_bytes());
    report[6..10].copy_from_slice(&crc32_ieee(payload).to_le_bytes());
    sealed(report)
}

/// Builds a chunk exactly as `buildWriteChunk` does.
fn chunk_frame(index: u16, data: &[u8]) -> [u8; REPORT_LEN] {
    let mut report = [0; REPORT_LEN];
    report[0] = CONFIG_PROTOCOL_VERSION;
    report[1] = UKF_WRITE_CHUNK;
    report[2..4].copy_from_slice(&index.to_le_bytes());
    report[4] = data.len() as u8;
    report[5..5 + data.len()].copy_from_slice(data);
    sealed(report)
}

fn payload_of(length: usize) -> Vec<u8> {
    (0..length).map(|index| (index * 7 + 3) as u8).collect()
}

/// Drives a whole transfer through the parser and the state machine.
fn transfer(payload: &[u8]) -> ConfigTransfer {
    let mut state = ConfigTransfer::new();

    match classify_feature_report(
        CONFIG_REPORT_ID,
        &declaration(UKF_WRITE_BEGIN, TARGET_SCRATCH, payload),
    ) {
        ConfigRequest::WriteBegin {
            target,
            total_len,
            payload_crc,
        } => state
            .begin(target, total_len, payload_crc)
            .expect("the board accepts a declaration it can hold"),
        other => panic!("the begin frame was not parsed as a begin: {other:?}"),
    }

    for (index, slice) in payload.chunks(CHUNK_BYTES).enumerate() {
        match classify_feature_report(CONFIG_REPORT_ID, &chunk_frame(index as u16, slice)) {
            ConfigRequest::WriteChunk { index, data, count } => state
                .chunk(index, &data[..usize::from(count)])
                .expect("the board accepts a chunk in order"),
            other => panic!("the chunk frame was not parsed as a chunk: {other:?}"),
        }
    }

    match classify_feature_report(
        CONFIG_REPORT_ID,
        &declaration(UKF_WRITE_COMMIT, TARGET_SCRATCH, payload),
    ) {
        ConfigRequest::WriteCommit {
            target,
            total_len,
            payload_crc,
        } => {
            let committed = state
                .commit(target, total_len, payload_crc)
                .expect("a complete transfer commits");
            assert_eq!(
                committed, payload,
                "the bytes that arrived are the bytes sent"
            );
        }
        other => panic!("the commit frame was not parsed as a commit: {other:?}"),
    }

    state
}

#[test]
fn a_payload_that_fits_one_chunk_survives_the_wire() {
    let payload = payload_of(4);

    let state = transfer(&payload);

    assert_eq!(state.state(), TransferState::Committed);
    assert_eq!(state.received_len(), 4);
    assert_eq!(state.computed_crc(), crc32_ieee(&payload));
}

#[test]
fn a_payload_that_needs_many_chunks_survives_the_wire() {
    // Not a multiple of the chunk size, so the last frame is short. That
    // boundary is where an off-by-one in either encoder would show.
    let payload = payload_of(CHUNK_BYTES * 3 + 1);

    let state = transfer(&payload);

    assert_eq!(state.state(), TransferState::Committed);
    assert_eq!(usize::from(state.received_len()), payload.len());
}

#[test]
fn a_payload_that_fills_the_staging_area_survives_the_wire() {
    let payload = payload_of(512);

    let state = transfer(&payload);

    assert_eq!(state.state(), TransferState::Committed);
    assert_eq!(state.received_len(), 512);
}

#[test]
fn the_declaration_fields_land_where_the_ui_puts_them() {
    // Named offsets rather than a round trip, so a change that moves two fields
    // consistently — which a round trip cannot see — still fails here.
    let payload = payload_of(300);
    let frame = declaration(UKF_WRITE_BEGIN, TARGET_SCRATCH, &payload);

    match classify_feature_report(CONFIG_REPORT_ID, &frame) {
        ConfigRequest::WriteBegin {
            target,
            total_len,
            payload_crc,
        } => {
            assert_eq!(target, TARGET_SCRATCH);
            assert_eq!(total_len, 300);
            assert_eq!(payload_crc, crc32_ieee(&payload));
        }
        other => panic!("not parsed as a begin: {other:?}"),
    }
}

#[test]
fn the_chunk_fields_land_where_the_ui_puts_them() {
    let data = payload_of(CHUNK_BYTES);
    let frame = chunk_frame(0x0102, &data);

    match classify_feature_report(CONFIG_REPORT_ID, &frame) {
        ConfigRequest::WriteChunk {
            index,
            data: got,
            count,
        } => {
            assert_eq!(index, 0x0102);
            assert_eq!(usize::from(count), CHUNK_BYTES);
            assert_eq!(&got[..usize::from(count)], &data[..]);
        }
        other => panic!("not parsed as a chunk: {other:?}"),
    }
}

#[test]
fn a_commit_that_does_not_match_what_arrived_applies_nothing() {
    // The property the whole shape exists for, driven through the real parser
    // rather than the state machine alone.
    let payload = payload_of(30);
    let mut wrong = payload.clone();
    wrong[0] ^= 0xff;
    let mut state = ConfigTransfer::new();

    match classify_feature_report(
        CONFIG_REPORT_ID,
        &declaration(UKF_WRITE_BEGIN, TARGET_SCRATCH, &wrong),
    ) {
        ConfigRequest::WriteBegin {
            target,
            total_len,
            payload_crc,
        } => state
            .begin(target, total_len, payload_crc)
            .expect("declared"),
        other => panic!("not parsed as a begin: {other:?}"),
    }
    for (index, slice) in payload.chunks(CHUNK_BYTES).enumerate() {
        if let ConfigRequest::WriteChunk { index, data, count } =
            classify_feature_report(CONFIG_REPORT_ID, &chunk_frame(index as u16, slice))
        {
            state
                .chunk(index, &data[..usize::from(count)])
                .expect("chunks arrive in order");
        }
    }

    let refused = match classify_feature_report(
        CONFIG_REPORT_ID,
        &declaration(UKF_WRITE_COMMIT, TARGET_SCRATCH, &wrong),
    ) {
        ConfigRequest::WriteCommit {
            target,
            total_len,
            payload_crc,
        } => state.commit(target, total_len, payload_crc),
        other => panic!("not parsed as a commit: {other:?}"),
    };

    assert!(refused.is_err(), "a mismatched commit must not be accepted");
    assert_eq!(state.state(), TransferState::Failed);
}
