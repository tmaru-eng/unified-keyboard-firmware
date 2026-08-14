//! Bounded S140 USBD preflight with the nrf-usbd 0.3.0 errata prefix.
//!
//! The operation order is derived from `nrf-usbd` 0.3.0's private
//! `src/errata.rs::pre_enable` implementation (Errata 187 and 171). This
//! module intentionally does not copy the crate's unsafe MMIO implementation:
//! callers provide the narrow register boundary and retain ownership of the
//! final USB pull-up, READY clear, and post-enable operations.

#![forbid(unsafe_code)]

use crate::usb_preflight::{EVENTCAUSE_READY_BIT, ReadyOutcome, ReadyRegisters};

/// nRF52840 errata key register used by nrf-usbd's pre-enable sequence.
pub const ERRATA_KEY_ADDR: usize = 0x4006_EC00;
/// nRF52840 Erratum 187 control register.
pub const ERRATA_187_ADDR: usize = 0x4006_ED14;
/// nRF52840 Erratum 171 wakeup register.
pub const ERRATA_171_ADDR: usize = 0x4006_EC14;
/// Key value used by nrf-usbd 0.3.0.
pub const ERRATA_KEY: u32 = 0x0000_9375;
/// Erratum 187 pre-enable value.
pub const ERRATA_187_ENABLE: u32 = 0x0000_0003;
/// Erratum 171 pre-wakeup value.
pub const ERRATA_171_WAKE: u32 = 0x0000_00C0;

/// Minimal MMIO boundary needed by the errata-aware preflight.
pub trait ErrataRegisters: ReadyRegisters {
    /// Read a word from an errata control address.
    fn read_word(&mut self, address: usize) -> u32;

    /// Write a word to an errata control address.
    fn write_word(&mut self, address: usize, value: u32);
}

fn pre_enable<R: ErrataRegisters>(registers: &mut R) {
    // Mirrors nrf-usbd 0.3.0 errata::pre_enable(): Erratum 187 prefix,
    // followed by errata::pre_wakeup() for Erratum 171.
    registers.write_word(ERRATA_KEY_ADDR, ERRATA_KEY);
    registers.write_word(ERRATA_187_ADDR, ERRATA_187_ENABLE);
    registers.write_word(ERRATA_KEY_ADDR, ERRATA_KEY);

    if registers.read_word(ERRATA_KEY_ADDR) == 0 {
        registers.write_word(ERRATA_KEY_ADDR, ERRATA_KEY);
    }
    registers.write_word(ERRATA_171_ADDR, ERRATA_171_WAKE);
    registers.write_word(ERRATA_KEY_ADDR, ERRATA_KEY);
}

/// Apply the finite errata-aware ENABLE/READY preflight.
///
/// A successful result deliberately leaves READY uncleared and the pull-up
/// untouched so `nrf-usbd` can perform its normal post-enable sequence. A
/// timeout performs no additional MMIO and lets the caller recover to UF2.
pub fn preflight<R: ErrataRegisters>(registers: &mut R, max_polls: u32) -> ReadyOutcome {
    pre_enable(registers);
    registers.enable();
    for polls in 0..max_polls {
        if registers.eventcause() & EVENTCAUSE_READY_BIT != 0 {
            return ReadyOutcome::Ready { polls };
        }
    }
    ReadyOutcome::TimedOut { polls: max_polls }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    const EVENTCAUSE_ADDR: usize = 0x4002_7400;
    const PULLUP_ADDR: usize = 0x4002_750c;

    #[derive(Debug, Eq, PartialEq)]
    enum Operation {
        Read(usize),
        Write(usize, u32),
        Enable,
        Eventcause,
    }

    #[derive(Default)]
    struct FakeMmio {
        operations: Vec<Operation>,
        key_value: u32,
        eventcause: u32,
        ready_at: Option<u32>,
        polls: u32,
    }

    impl FakeMmio {
        fn record(&mut self, operation: Operation) {
            self.operations.push(operation);
        }
    }

    impl ReadyRegisters for FakeMmio {
        fn enable(&mut self) {
            self.record(Operation::Enable);
        }

        fn eventcause(&mut self) -> u32 {
            let poll = self.polls;
            self.polls += 1;
            self.record(Operation::Eventcause);
            let mut value = self.eventcause;
            if self.ready_at == Some(poll) {
                value |= EVENTCAUSE_READY_BIT;
            }
            value
        }
    }

    impl ErrataRegisters for FakeMmio {
        fn read_word(&mut self, address: usize) -> u32 {
            self.record(Operation::Read(address));
            if address == ERRATA_KEY_ADDR {
                self.key_value
            } else {
                0
            }
        }

        fn write_word(&mut self, address: usize, value: u32) {
            self.record(Operation::Write(address, value));
            if address == ERRATA_KEY_ADDR {
                self.key_value = value;
            }
        }
    }

    #[test]
    fn errata_prefix_is_ordered_before_enable() {
        let mut mmio = FakeMmio {
            ready_at: Some(2),
            ..Default::default()
        };
        assert_eq!(preflight(&mut mmio, 8), ReadyOutcome::Ready { polls: 2 });
        assert_eq!(
            &mmio.operations[..7],
            &[
                Operation::Write(ERRATA_KEY_ADDR, ERRATA_KEY),
                Operation::Write(ERRATA_187_ADDR, ERRATA_187_ENABLE),
                Operation::Write(ERRATA_KEY_ADDR, ERRATA_KEY),
                Operation::Read(ERRATA_KEY_ADDR),
                Operation::Write(ERRATA_171_ADDR, ERRATA_171_WAKE),
                Operation::Write(ERRATA_KEY_ADDR, ERRATA_KEY),
                Operation::Enable,
            ]
        );
    }

    #[test]
    fn never_ready_times_out_without_ready_clear_or_pullup() {
        let mut mmio = FakeMmio::default();
        assert_eq!(preflight(&mut mmio, 3), ReadyOutcome::TimedOut { polls: 3 });
        assert!(!mmio.operations.iter().any(|operation| matches!(
            operation,
            Operation::Write(EVENTCAUSE_ADDR, _) | Operation::Write(PULLUP_ADDR, _)
        )));
    }

    #[test]
    fn only_eventcause_ready_bit_11_succeeds() {
        let mut mmio = FakeMmio {
            eventcause: 1 << 10,
            ..Default::default()
        };
        assert_eq!(preflight(&mut mmio, 1), ReadyOutcome::TimedOut { polls: 1 });
        mmio.eventcause = EVENTCAUSE_READY_BIT | (1 << 10);
        mmio.polls = 0;
        assert_eq!(preflight(&mut mmio, 1), ReadyOutcome::Ready { polls: 0 });
    }

    #[test]
    fn zero_budget_still_runs_errata_and_enable_once() {
        let mut mmio = FakeMmio::default();
        assert_eq!(preflight(&mut mmio, 0), ReadyOutcome::TimedOut { polls: 0 });
        assert_eq!(
            mmio.operations
                .iter()
                .filter(|operation| **operation == Operation::Enable)
                .count(),
            1
        );
        assert!(
            !mmio
                .operations
                .iter()
                .any(|operation| matches!(operation, Operation::Eventcause))
        );
    }
}
