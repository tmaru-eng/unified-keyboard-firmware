//! Host tests for the fixed-capacity configuration transfer state machine.

use ukf_nrf52840_ble_usb::config_transfer::{
    ConfigTransfer, TARGET_BOND_MANAGEMENT, TARGET_KEYMAP, TARGET_PROFILE, TARGET_SCRATCH,
    TARGET_SOURCE_PROFILE, TRANSFER_CAPACITY, TransferError, TransferState,
};
use ukf_nrf52840_ble_usb::uf2_reset::crc32_ieee;

fn send_chunks(transfer: &mut ConfigTransfer, payload: &[u8]) {
    for (index, chunk) in payload.chunks(23).enumerate() {
        transfer
            .chunk(
                u16::try_from(index).expect("test payload has few chunks"),
                chunk,
            )
            .expect("valid chunk is accepted");
    }
}

#[test]
fn a_payload_that_fits_in_one_chunk_commits() {
    let payload = [1, 2, 3, 4, 5];
    let mut transfer = ConfigTransfer::new();

    transfer
        .begin(TARGET_SCRATCH, payload.len() as u16, crc32_ieee(&payload))
        .expect("begin succeeds");
    transfer.chunk(0, &payload).expect("one chunk succeeds");

    assert_eq!(transfer.state(), TransferState::Receiving);
    assert_eq!(transfer.received_len(), payload.len() as u16);
    assert_eq!(
        transfer.commit(TARGET_SCRATCH, payload.len() as u16, crc32_ieee(&payload)),
        Ok(&payload[..])
    );
    assert_eq!(transfer.state(), TransferState::Committed);
    assert_eq!(transfer.computed_crc(), crc32_ieee(&payload));
}

#[test]
fn target_profile_is_accepted_by_begin_and_commit() {
    let payload = [6, 7, 8];
    let payload_crc = crc32_ieee(&payload);
    let mut transfer = ConfigTransfer::new();

    transfer
        .begin(TARGET_PROFILE, payload.len() as u16, payload_crc)
        .expect("profile begin succeeds");
    transfer.chunk(0, &payload).expect("profile chunk succeeds");

    assert_eq!(
        transfer.commit(TARGET_PROFILE, payload.len() as u16, payload_crc),
        Ok(&payload[..])
    );
}

#[test]
fn source_profile_target_is_distinct_from_the_legacy_global_profile_target() {
    let payload = [2, 4, 4, 5];
    let payload_crc = crc32_ieee(&payload);
    let mut transfer = ConfigTransfer::new();

    transfer
        .begin(TARGET_SOURCE_PROFILE, payload.len() as u16, payload_crc)
        .expect("source profile target is supported");
    transfer.chunk(0, &payload).unwrap();

    assert_eq!(
        transfer.commit(TARGET_SOURCE_PROFILE, payload.len() as u16, payload_crc),
        Ok(&payload[..])
    );
    assert_ne!(TARGET_SOURCE_PROFILE, TARGET_PROFILE);
}

#[test]
fn bond_management_target_is_accepted_by_begin_and_commit() {
    let payload = [1, 2, 3];
    let payload_crc = crc32_ieee(&payload);
    let mut transfer = ConfigTransfer::new();

    transfer
        .begin(TARGET_BOND_MANAGEMENT, payload.len() as u16, payload_crc)
        .expect("bond-management target is supported");
    transfer.chunk(0, &payload).unwrap();

    assert_eq!(
        transfer.commit(TARGET_BOND_MANAGEMENT, payload.len() as u16, payload_crc),
        Ok(&payload[..])
    );
}

#[test]
fn keymap_target_is_accepted_by_begin_and_commit() {
    let payload = [1, 0, 0x04, 0, 0x05, 0];
    let payload_crc = crc32_ieee(&payload);
    let mut transfer = ConfigTransfer::new();

    transfer
        .begin(TARGET_KEYMAP, payload.len() as u16, payload_crc)
        .expect("keymap target is supported");
    transfer.chunk(0, &payload).unwrap();

    assert_eq!(
        transfer.commit(TARGET_KEYMAP, payload.len() as u16, payload_crc),
        Ok(&payload[..])
    );
}

#[test]
fn a_full_capacity_payload_commits_with_a_short_final_chunk() {
    let payload: [u8; TRANSFER_CAPACITY] = core::array::from_fn(|index| index as u8);
    let payload_crc = crc32_ieee(&payload);
    let mut transfer = ConfigTransfer::new();

    transfer
        .begin(TARGET_SCRATCH, TRANSFER_CAPACITY as u16, payload_crc)
        .expect("begin succeeds");
    send_chunks(&mut transfer, &payload);

    assert_eq!(transfer.received_len(), TRANSFER_CAPACITY as u16);
    assert_eq!(transfer.next_index(), 23);
    assert_eq!(
        transfer.commit(TARGET_SCRATCH, TRANSFER_CAPACITY as u16, payload_crc),
        Ok(&payload[..])
    );
}

#[test]
fn a_chunk_without_begin_is_rejected_with_reason_one() {
    let mut transfer = ConfigTransfer::new();

    assert_eq!(transfer.chunk(0, &[1]), Err(TransferError::BeginRequired));
    assert_eq!(transfer.state(), TransferState::Failed);
    assert_eq!(transfer.last_error(), 1);
}

#[test]
fn a_skipped_index_is_rejected_and_next_index_is_unchanged() {
    let mut transfer = ConfigTransfer::new();
    transfer
        .begin(TARGET_SCRATCH, 2, crc32_ieee(&[1, 2]))
        .unwrap();

    assert_eq!(
        transfer.chunk(1, &[1]),
        Err(TransferError::UnexpectedIndex {
            expected: 0,
            actual: 1,
        })
    );
    assert_eq!(transfer.last_error(), 2);
    assert_eq!(transfer.next_index(), 0);
}

#[test]
fn sending_the_same_index_twice_is_rejected() {
    let mut transfer = ConfigTransfer::new();
    transfer
        .begin(TARGET_SCRATCH, 2, crc32_ieee(&[1, 2]))
        .unwrap();
    transfer.chunk(0, &[1]).unwrap();

    assert_eq!(
        transfer.chunk(0, &[2]),
        Err(TransferError::UnexpectedIndex {
            expected: 1,
            actual: 0,
        })
    );
    assert_eq!(transfer.last_error(), 2);
}

#[test]
fn zero_and_24_byte_chunks_are_rejected_with_reason_three() {
    let mut zero = ConfigTransfer::new();
    zero.begin(TARGET_SCRATCH, 1, crc32_ieee(&[1])).unwrap();
    assert_eq!(
        zero.chunk(0, &[]),
        Err(TransferError::InvalidChunkCount { count: 0 })
    );
    assert_eq!(zero.last_error(), 3);

    let mut too_many = ConfigTransfer::new();
    too_many
        .begin(TARGET_SCRATCH, 24, crc32_ieee(&[0; 24]))
        .unwrap();
    assert_eq!(
        too_many.chunk(0, &[0; 24]),
        Err(TransferError::InvalidChunkCount { count: 24 })
    );
    assert_eq!(too_many.last_error(), 3);
}

#[test]
fn a_begin_larger_than_the_fixed_capacity_is_rejected_with_reason_four() {
    let mut transfer = ConfigTransfer::new();

    assert_eq!(
        transfer.begin(TARGET_SCRATCH, (TRANSFER_CAPACITY as u16) + 1, 0),
        Err(TransferError::TotalLengthExceedsCapacity {
            total_len: (TRANSFER_CAPACITY as u16) + 1,
        })
    );
    assert_eq!(transfer.state(), TransferState::Failed);
    assert_eq!(transfer.last_error(), 4);
}

#[test]
fn a_commit_before_all_bytes_are_received_is_rejected_with_reason_five() {
    let mut transfer = ConfigTransfer::new();
    transfer
        .begin(TARGET_SCRATCH, 2, crc32_ieee(&[1, 2]))
        .unwrap();
    transfer.chunk(0, &[1]).unwrap();

    assert_eq!(
        transfer.commit(TARGET_SCRATCH, 2, crc32_ieee(&[1, 2])),
        Err(TransferError::CommitBeforeComplete {
            expected: 2,
            received: 1,
        })
    );
    assert_eq!(transfer.state(), TransferState::Failed);
    assert_eq!(transfer.last_error(), 5);
    assert_eq!(transfer.computed_crc(), 0);
}

#[test]
fn a_crc_mismatch_fails_without_returning_partial_data() {
    let payload = [9, 8, 7];
    let actual_crc = crc32_ieee(&payload);
    let mut transfer = ConfigTransfer::new();
    transfer
        .begin(TARGET_SCRATCH, payload.len() as u16, actual_crc ^ 1)
        .unwrap();
    transfer.chunk(0, &payload).unwrap();

    assert_eq!(
        transfer.commit(TARGET_SCRATCH, payload.len() as u16, actual_crc ^ 1),
        Err(TransferError::CrcMismatch {
            declared: actual_crc ^ 1,
            computed: actual_crc,
        })
    );
    assert_eq!(transfer.state(), TransferState::Failed);
    assert_eq!(transfer.last_error(), 6);
    assert_eq!(transfer.computed_crc(), actual_crc);
}

#[test]
fn commit_parameters_different_from_begin_are_rejected_with_reason_seven() {
    let payload = [4, 5, 6];
    let crc = crc32_ieee(&payload);
    let mut transfer = ConfigTransfer::new();
    transfer
        .begin(TARGET_SCRATCH, payload.len() as u16, crc)
        .unwrap();
    transfer.chunk(0, &payload).unwrap();

    assert_eq!(
        transfer.commit(TARGET_SCRATCH, payload.len() as u16 + 1, crc),
        Err(TransferError::CommitParametersMismatch)
    );
    assert_eq!(transfer.state(), TransferState::Failed);
    assert_eq!(transfer.last_error(), 7);
}

#[test]
fn unknown_targets_are_rejected_with_reason_eight() {
    let mut begin = ConfigTransfer::new();
    assert_eq!(begin.begin(9, 0, 0), Err(TransferError::UnknownTarget(9)));
    assert_eq!(begin.last_error(), 8);

    let mut commit = ConfigTransfer::new();
    commit.begin(TARGET_SCRATCH, 0, 0).unwrap();
    assert_eq!(commit.commit(9, 0, 0), Err(TransferError::UnknownTarget(9)));
    assert_eq!(commit.last_error(), 8);
}

#[test]
fn a_new_begin_discards_an_abandoned_transfer() {
    let first = [1, 2, 3];
    let second = [7, 8];
    let mut transfer = ConfigTransfer::new();
    transfer
        .begin(TARGET_SCRATCH, first.len() as u16, crc32_ieee(&first))
        .unwrap();
    transfer.chunk(0, &first).unwrap();

    transfer
        .begin(TARGET_SCRATCH, second.len() as u16, crc32_ieee(&second))
        .unwrap();
    assert_eq!(transfer.state(), TransferState::Receiving);
    assert_eq!(transfer.expected_len(), second.len() as u16);
    assert_eq!(transfer.received_len(), 0);
    assert_eq!(transfer.next_index(), 0);
    assert_eq!(transfer.last_error(), 0);

    transfer.chunk(0, &second).unwrap();
    assert_eq!(
        transfer.commit(TARGET_SCRATCH, second.len() as u16, crc32_ieee(&second)),
        Ok(&second[..])
    );
}
