//! USB peripheral driver for nRF microcontrollers.

#![no_std]
#![allow(mismatched_lifetime_syntaxes)]

mod errata;
mod dma_gate;
mod pac;
mod usbd;

pub use usbd::{dma_timed_out, last_enable_succeeded, Usbd};

/// A trait for device-specific USB peripherals. Implement this to add support for a new hardware
/// platform. Peripherals that have this trait must have the same register block as NRF52 USBD
/// peripherals.
pub unsafe trait UsbPeripheral: Send {
    /// Pointer to the register block
    const REGISTERS: *const ();
}
