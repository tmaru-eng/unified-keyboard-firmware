//! Reference keyboard models used to drive the core without hardware.
//!
//! A mock keyboard answers two questions a raw [`BootKeyboardReport`] cannot:
//! which usages the physical layout is actually able to produce, and how that
//! layout's reports arrive over each transport. Keeping those separate is the
//! point. **The same physical layout must produce the same bridge output
//! whether its reports arrive over USB or BLE**, because the transport is a
//! route and not a meaning. Tests written against these models fix that.
//!
//! This module is `no_std` and allocation-free, so target builds and host tests
//! use the same model.

use crate::{BOOT_KEY_SLOTS, BootKeyboardReport, BridgeProfile, InputTransport};

/// Modifier bit for Left Shift, as defined by the HID Usage Tables.
pub const MOD_LEFT_SHIFT: u8 = 0b0000_0010;

/// Usages a JIS keyboard has and an ANSI US keyboard does not.
pub mod jis_usage {
    /// `ろ`, HID `International1`.
    pub const RO: u8 = 0x87;
    /// `¥`, HID `International3`.
    pub const YEN: u8 = 0x89;
    /// `変換`, HID `International4`.
    pub const HENKAN: u8 = 0x8A;
    /// `無変換`, HID `International5`.
    pub const MUHENKAN: u8 = 0x8B;
    /// `かな`, HID `Lang1`.
    pub const KANA: u8 = 0x90;
    /// `英数`, HID `Lang2`.
    pub const EISU: u8 = 0x91;
}

/// The physical key arrangement of a modelled keyboard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhysicalLayout {
    /// ANSI US. Has no Japanese-specific keys.
    AnsiUs,
    /// Japanese JIS. Adds the `International` and `Lang` usages.
    Jis,
}

impl PhysicalLayout {
    /// Whether a key with this usage physically exists on the layout.
    ///
    /// Modelling this is what stops a test from asserting behavior for a
    /// keystroke the keyboard could never have sent.
    pub const fn can_produce(self, usage: u8) -> bool {
        match self {
            Self::Jis => true,
            Self::AnsiUs => !matches!(
                usage,
                jis_usage::RO
                    | jis_usage::YEN
                    | jis_usage::HENKAN
                    | jis_usage::MUHENKAN
                    | jis_usage::KANA
                    | jis_usage::EISU
            ),
        }
    }

    /// The compatibility profile this layout should be attached with.
    ///
    /// An ANSI US keyboard on a JIS-configured host needs the conversion; a JIS
    /// keyboard already matches the host and must be passed through untouched.
    pub const fn default_profile(self) -> BridgeProfile {
        match self {
            Self::AnsiUs => BridgeProfile::US_JIS_PRESET,
            Self::Jis => BridgeProfile::NONE,
        }
    }
}

/// Reason a modelled keystroke could not be produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MockError {
    /// The layout has no key with this usage.
    NotOnLayout {
        /// The rejected HID usage.
        usage: u8,
        /// The layout that rejected it.
        layout: PhysicalLayout,
    },
    /// All six boot-protocol key slots are already occupied.
    NoFreeKeySlot,
}

/// Largest wire report this model emits: six keys, modifiers, reserved byte,
/// and an optional leading HOGP report ID.
pub const MAX_WIRE_REPORT_LEN: usize = BOOT_KEY_SLOTS + 3;

/// A keyboard of a known layout attached over a known transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MockKeyboard {
    layout: PhysicalLayout,
    transport: InputTransport,
    report_id: Option<u8>,
    modifiers: u8,
    keys: [u8; BOOT_KEY_SLOTS],
}

impl MockKeyboard {
    /// An ANSI US keyboard attached over USB.
    pub const fn ansi_us_usb() -> Self {
        Self::new(PhysicalLayout::AnsiUs, InputTransport::Usb, None)
    }

    /// An ANSI US keyboard attached over BLE, sending report-ID-prefixed HOGP
    /// notifications.
    pub const fn ansi_us_ble() -> Self {
        Self::new(PhysicalLayout::AnsiUs, InputTransport::Ble, Some(1))
    }

    /// A JIS keyboard attached over USB.
    pub const fn jis_usb() -> Self {
        Self::new(PhysicalLayout::Jis, InputTransport::Usb, None)
    }

    /// A JIS keyboard attached over BLE, sending report-ID-prefixed HOGP
    /// notifications.
    pub const fn jis_ble() -> Self {
        Self::new(PhysicalLayout::Jis, InputTransport::Ble, Some(1))
    }

    /// Builds a keyboard with an explicit layout, transport, and HOGP report ID.
    pub const fn new(
        layout: PhysicalLayout,
        transport: InputTransport,
        report_id: Option<u8>,
    ) -> Self {
        Self {
            layout,
            transport,
            report_id,
            modifiers: 0,
            keys: [0; BOOT_KEY_SLOTS],
        }
    }

    /// The modelled physical layout.
    pub const fn layout(&self) -> PhysicalLayout {
        self.layout
    }

    /// The transport this keyboard is attached through.
    pub const fn transport(&self) -> InputTransport {
        self.transport
    }

    /// The profile this keyboard should be attached to the bridge with.
    pub const fn profile(&self) -> BridgeProfile {
        self.layout.default_profile()
    }

    /// The report the keyboard would currently be sending.
    pub const fn report(&self) -> BootKeyboardReport {
        BootKeyboardReport {
            modifiers: self.modifiers,
            keys: self.keys,
        }
    }

    /// Presses one key and returns the resulting report.
    pub fn press(&mut self, usage: u8) -> Result<BootKeyboardReport, MockError> {
        if !self.layout.can_produce(usage) {
            return Err(MockError::NotOnLayout {
                usage,
                layout: self.layout,
            });
        }
        if self.keys.contains(&usage) {
            return Ok(self.report());
        }
        let slot = self
            .keys
            .iter_mut()
            .find(|slot| **slot == 0)
            .ok_or(MockError::NoFreeKeySlot)?;
        *slot = usage;
        Ok(self.report())
    }

    /// Releases one key. Releasing a key that is not held is not an error;
    /// a real keyboard cannot report a release it never pressed.
    pub fn release(&mut self, usage: u8) -> BootKeyboardReport {
        for slot in &mut self.keys {
            if *slot == usage {
                *slot = 0;
            }
        }
        self.compact();
        self.report()
    }

    /// Holds the given modifier bits.
    pub fn hold_modifiers(&mut self, mask: u8) -> BootKeyboardReport {
        self.modifiers |= mask;
        self.report()
    }

    /// Releases the given modifier bits.
    pub fn release_modifiers(&mut self, mask: u8) -> BootKeyboardReport {
        self.modifiers &= !mask;
        self.report()
    }

    /// Presses a key while the given modifiers are held, then releases both.
    ///
    /// Returns the report observed while the chord is down. This is the shape
    /// most conversion tests need: `Shift+2` on an ANSI US keyboard.
    pub fn chord(&mut self, modifiers: u8, usage: u8) -> Result<BootKeyboardReport, MockError> {
        self.hold_modifiers(modifiers);
        let held = self.press(usage)?;
        self.release(usage);
        self.release_modifiers(modifiers);
        Ok(held)
    }

    /// Releases every key and modifier, as a disconnect or reset would.
    pub fn release_all(&mut self) -> BootKeyboardReport {
        self.modifiers = 0;
        self.keys = [0; BOOT_KEY_SLOTS];
        self.report()
    }

    /// Serialises the current report the way this transport carries it.
    ///
    /// USB boot protocol is the bare eight bytes. HOGP may prefix the report
    /// ID, which is why the bridge's BLE path has to accept both framings.
    /// Returns the buffer and the number of bytes that are actually on the wire.
    pub fn wire_report(&self) -> ([u8; MAX_WIRE_REPORT_LEN], usize) {
        let mut wire = [0; MAX_WIRE_REPORT_LEN];
        let mut len = 0;
        if let Some(report_id) = self.report_id {
            wire[len] = report_id;
            len += 1;
        }
        wire[len] = self.modifiers;
        // Byte 1 of a boot report is reserved and always zero.
        len += 2;
        for usage in self.keys {
            wire[len] = usage;
            len += 1;
        }
        (wire, len)
    }

    /// Moves held keys into the lowest free slots.
    ///
    /// Real keyboards differ on whether they compact after a release. Doing it
    /// here keeps the model deterministic, and the core must not depend on slot
    /// position either way.
    fn compact(&mut self) {
        let mut compacted = [0; BOOT_KEY_SLOTS];
        let mut next = 0;
        for usage in self.keys {
            if usage != 0 {
                compacted[next] = usage;
                next += 1;
            }
        }
        self.keys = compacted;
    }
}
