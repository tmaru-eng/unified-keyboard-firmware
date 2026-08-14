//! Fixed-capacity configuration transfer state machine.

use crate::uf2_reset::crc32_ieee;

/// Maximum payload that can be staged by one transfer.
pub const TRANSFER_CAPACITY: usize = 512;
/// Target identifier for the validation-only scratch area.
pub const TARGET_SCRATCH: u8 = 1;
/// Target identifier for the active profile.
pub const TARGET_PROFILE: u8 = 2;
/// Target identifier for the versioned source-slot profile payload.
pub const TARGET_SOURCE_PROFILE: u8 = 3;
/// Target carrying a rename/delete request for one registered bond slot.
pub const TARGET_BOND_MANAGEMENT: u8 = 4;
/// Target carrying the versioned fixed-capacity keymap payload.
pub const TARGET_KEYMAP: u8 = 5;

/// State of a configuration transfer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TransferState {
    /// No transfer has been started.
    Idle = 0,
    /// A BEGIN was accepted and chunks are being collected.
    Receiving = 1,
    /// The staged payload passed COMMIT validation.
    Committed = 2,
    /// A transfer operation failed and requires a new BEGIN.
    Failed = 3,
}

/// Why a configuration transfer operation was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferError {
    /// A CHUNK arrived without an active transfer.
    BeginRequired,
    /// The chunk index was not the next expected index.
    UnexpectedIndex {
        /// Index that the state machine expected.
        expected: u16,
        /// Index supplied by the caller.
        actual: u16,
    },
    /// The chunk did not contain between one and twenty-three bytes.
    InvalidChunkCount {
        /// Number of bytes supplied by the caller.
        count: usize,
    },
    /// The BEGIN length is larger than the fixed staging area.
    TotalLengthExceedsCapacity {
        /// Length supplied by the caller.
        total_len: u16,
    },
    /// COMMIT arrived before the declared number of bytes was received.
    CommitBeforeComplete {
        /// Number of bytes declared by BEGIN.
        expected: u16,
        /// Number of bytes received so far.
        received: u16,
    },
    /// The computed payload CRC differs from the declared CRC.
    CrcMismatch {
        /// CRC declared by BEGIN and COMMIT.
        declared: u32,
        /// CRC computed over the received payload.
        computed: u32,
    },
    /// One or more COMMIT parameters differ from BEGIN.
    CommitParametersMismatch,
    /// The target is not supported by this transfer implementation.
    UnknownTarget(u8),
}

impl TransferError {
    /// Returns the stable diagnostic number for this error.
    pub const fn code(&self) -> u8 {
        match self {
            Self::BeginRequired => 1,
            Self::UnexpectedIndex { .. } => 2,
            Self::InvalidChunkCount { .. } => 3,
            Self::TotalLengthExceedsCapacity { .. } => 4,
            Self::CommitBeforeComplete { .. } => 5,
            Self::CrcMismatch { .. } => 6,
            Self::CommitParametersMismatch => 7,
            Self::UnknownTarget(_) => 8,
        }
    }
}

/// Owns one fixed-size, allocation-free configuration transfer.
pub struct ConfigTransfer {
    buffer: [u8; TRANSFER_CAPACITY],
    state: TransferState,
    last_error: u8,
    target: u8,
    expected_len: u16,
    received_len: u16,
    next_index: u16,
    declared_crc: u32,
    computed_crc: u32,
}

impl Default for ConfigTransfer {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigTransfer {
    /// Creates an idle transfer with an empty staging area.
    pub const fn new() -> Self {
        Self {
            buffer: [0; TRANSFER_CAPACITY],
            state: TransferState::Idle,
            last_error: 0,
            target: 0,
            expected_len: 0,
            received_len: 0,
            next_index: 0,
            declared_crc: 0,
            computed_crc: 0,
        }
    }

    /// Starts a new transfer, discarding any previous transfer state.
    pub fn begin(
        &mut self,
        target: u8,
        total_len: u16,
        payload_crc: u32,
    ) -> Result<(), TransferError> {
        self.buffer = [0; TRANSFER_CAPACITY];
        self.state = TransferState::Idle;
        self.last_error = 0;
        self.target = target;
        self.expected_len = total_len;
        self.received_len = 0;
        self.next_index = 0;
        self.declared_crc = payload_crc;
        self.computed_crc = 0;

        if !valid_target(target) {
            return Err(self.fail(TransferError::UnknownTarget(target)));
        }
        if usize::from(total_len) > TRANSFER_CAPACITY {
            return Err(self.fail(TransferError::TotalLengthExceedsCapacity { total_len }));
        }

        self.state = TransferState::Receiving;
        Ok(())
    }

    /// Accepts the next sequential chunk of the transfer.
    pub fn chunk(&mut self, index: u16, data: &[u8]) -> Result<(), TransferError> {
        if self.state != TransferState::Receiving {
            return Err(self.fail(TransferError::BeginRequired));
        }
        if data.is_empty() || data.len() >= 24 {
            return Err(self.fail(TransferError::InvalidChunkCount { count: data.len() }));
        }
        if index != self.next_index {
            return Err(self.fail(TransferError::UnexpectedIndex {
                expected: self.next_index,
                actual: index,
            }));
        }

        let start = usize::from(self.received_len);
        let end = start + data.len();
        if end > TRANSFER_CAPACITY {
            return Err(self.fail(TransferError::TotalLengthExceedsCapacity {
                total_len: self.expected_len,
            }));
        }
        self.buffer[start..end].copy_from_slice(data);
        self.received_len = end as u16;
        self.next_index += 1;
        Ok(())
    }

    /// Validates and commits the staged payload, returning its bytes on success.
    pub fn commit(
        &mut self,
        target: u8,
        total_len: u16,
        payload_crc: u32,
    ) -> Result<&[u8], TransferError> {
        if !valid_target(target) {
            return Err(self.fail(TransferError::UnknownTarget(target)));
        }
        if self.state != TransferState::Receiving {
            return Err(self.fail(TransferError::CommitBeforeComplete {
                expected: self.expected_len,
                received: self.received_len,
            }));
        }
        if target != self.target
            || total_len != self.expected_len
            || payload_crc != self.declared_crc
        {
            return Err(self.fail(TransferError::CommitParametersMismatch));
        }
        if self.received_len != self.expected_len {
            return Err(self.fail(TransferError::CommitBeforeComplete {
                expected: self.expected_len,
                received: self.received_len,
            }));
        }

        let computed = crc32_ieee(&self.buffer[..usize::from(self.expected_len)]);
        self.computed_crc = computed;
        if computed != payload_crc {
            return Err(self.fail(TransferError::CrcMismatch {
                declared: payload_crc,
                computed,
            }));
        }

        self.state = TransferState::Committed;
        Ok(&self.buffer[..usize::from(self.expected_len)])
    }

    /// Records a failure and returns the same typed error to the caller.
    fn fail(&mut self, error: TransferError) -> TransferError {
        if self.state != TransferState::Failed {
            self.state = TransferState::Failed;
            self.last_error = error.code();
        }
        error
    }

    /// Returns the current transfer state.
    pub const fn state(&self) -> TransferState {
        self.state
    }

    /// Returns the stable number of the last failed operation, or zero.
    pub const fn last_error(&self) -> u8 {
        self.last_error
    }

    /// Returns the current transfer target.
    pub const fn target(&self) -> u8 {
        self.target
    }

    /// Returns the length declared by BEGIN.
    pub const fn expected_len(&self) -> u16 {
        self.expected_len
    }

    /// Returns the number of bytes accepted from CHUNK requests.
    pub const fn received_len(&self) -> u16 {
        self.received_len
    }

    /// Returns the next chunk index required by the state machine.
    pub const fn next_index(&self) -> u16 {
        self.next_index
    }

    /// Returns the CRC declared by BEGIN.
    pub const fn declared_crc(&self) -> u32 {
        self.declared_crc
    }

    /// Returns the computed CRC after a complete CRC check, or zero otherwise.
    pub const fn computed_crc(&self) -> u32 {
        self.computed_crc
    }
}

const fn valid_target(target: u8) -> bool {
    matches!(
        target,
        TARGET_SCRATCH
            | TARGET_PROFILE
            | TARGET_SOURCE_PROFILE
            | TARGET_BOND_MANAGEMENT
            | TARGET_KEYMAP
    )
}
