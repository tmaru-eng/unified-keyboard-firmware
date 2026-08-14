#![no_std]
#![forbid(unsafe_code)]

//! Platform-independent primitives for the first Unified Keyboard Firmware bridge.
//!
//! The core accepts one boot-keyboard report per attached source, applies that
//! source's profile, and produces one merged USB boot-keyboard report. Hardware
//! drivers are deliberately outside this crate.

pub mod mock;
pub mod watchdog;

pub use watchdog::StuckKeyWatchdog;

/// Number of registered physical source slots.
pub const REGISTERED_SOURCE_SLOTS: usize = 4;
/// Slot reserved for input injected through the configuration interface.
pub const VIRTUAL_SOURCE_SLOT: u8 = 4;
/// Stable source identifier for the configuration-injection slot.
pub const VIRTUAL_SOURCE_ID: SourceId = SourceId(VIRTUAL_SOURCE_SLOT);
/// Total source slots, including the virtual source.
pub const SOURCE_SLOT_COUNT: usize = REGISTERED_SOURCE_SLOTS + 1;
/// Maximum simultaneously active bridge inputs in the private prototype.
pub const MAX_SOURCES: usize = SOURCE_SLOT_COUNT;
/// Number of keys in a USB HID boot keyboard input report.
pub const BOOT_KEY_SLOTS: usize = 6;
/// Maximum UTF-8/name bytes retained for one source without allocation.
pub const SOURCE_NAME_LEN: usize = 13;
/// Length of a Bluetooth IRK kept in private source registration state.
pub const IRK_LEN: usize = 16;
/// Maximum number of single-usage rules in the first data-driven keymap.
pub const KEYMAP_RULE_CAPACITY: usize = 32;

const LEFT_CTRL: u8 = 0b0000_0001;
const LEFT_SHIFT: u8 = 0b0000_0010;
const RIGHT_CTRL: u8 = 0b0001_0000;
const RIGHT_SHIFT: u8 = 0b0010_0000;
const LEFT_ALT: u8 = 0b0000_0100;
const LEFT_GUI: u8 = 0b0000_1000;
const RIGHT_ALT: u8 = 0b0100_0000;
const RIGHT_GUI: u8 = 0b1000_0000;
const SHIFT_MASK: u8 = LEFT_SHIFT | RIGHT_SHIFT;
const SHORTCUT_MODIFIER_MASK: u8 =
    LEFT_CTRL | RIGHT_CTRL | LEFT_ALT | LEFT_GUI | RIGHT_ALT | RIGHT_GUI;
const CAPS_LOCK: u8 = 0x39;
const HID_RO: u8 = 0x87;
const HID_YEN: u8 = 0x89;

/// One usage-and-shift-sensitive keymap rule.
///
/// The first data-driven keymap milestone deliberately models only the
/// behavior that the bridge already supports: one keyboard usage can become
/// another usage and may request a synthesized Shift state. Layers, macros,
/// and other actions remain later protocol features.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeymapRule {
    /// Source keyboard usage.
    pub input_usage: u8,
    /// Whether the source report has a Shift modifier for this rule.
    pub input_shifted: bool,
    /// Output keyboard usage. Zero suppresses the source usage.
    pub output_usage: u8,
    /// Whether the output report should carry Shift for this rule.
    pub output_shifted: bool,
}

impl KeymapRule {
    /// Creates a usage mapping rule.
    pub const fn new(
        input_usage: u8,
        input_shifted: bool,
        output_usage: u8,
        output_shifted: bool,
    ) -> Self {
        Self {
            input_usage,
            input_shifted,
            output_usage,
            output_shifted,
        }
    }

    const EMPTY: Self = Self::new(0, false, 0, false);

    const fn matches(self, input_usage: u8, input_shifted: bool) -> bool {
        self.input_usage == input_usage && self.input_shifted == input_shifted
    }
}

/// A fixed-capacity, allocation-free single-layer keymap.
///
/// `US_JIS` is the compatibility table used by the current bridge. A caller
/// can start with [`Self::new`] and replace rules with [`Self::set_rule`] for
/// TDD and future configuration-transfer work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Keymap {
    rules: [KeymapRule; KEYMAP_RULE_CAPACITY],
    len: u8,
}

const DEFAULT_US_JIS_RULES: [KeymapRule; KEYMAP_RULE_CAPACITY] = {
    let mut rules = [KeymapRule::EMPTY; KEYMAP_RULE_CAPACITY];
    rules[0] = KeymapRule::new(0x35, false, 0x2f, true);
    rules[1] = KeymapRule::new(0x35, true, 0x2e, true);
    rules[2] = KeymapRule::new(0x1f, true, 0x2f, false);
    rules[3] = KeymapRule::new(0x23, true, 0x2e, false);
    rules[4] = KeymapRule::new(0x24, true, 0x23, true);
    rules[5] = KeymapRule::new(0x25, true, 0x34, true);
    rules[6] = KeymapRule::new(0x26, true, 0x25, true);
    rules[7] = KeymapRule::new(0x27, true, 0x26, true);
    rules[8] = KeymapRule::new(0x2d, true, HID_RO, true);
    rules[9] = KeymapRule::new(0x2e, false, 0x2d, true);
    rules[10] = KeymapRule::new(0x2e, true, 0x33, true);
    rules[11] = KeymapRule::new(0x2f, false, 0x30, false);
    rules[12] = KeymapRule::new(0x2f, true, 0x30, true);
    rules[13] = KeymapRule::new(0x30, false, 0x31, false);
    rules[14] = KeymapRule::new(0x30, true, 0x31, true);
    rules[15] = KeymapRule::new(0x31, false, HID_RO, false);
    rules[16] = KeymapRule::new(0x31, true, HID_YEN, true);
    rules[17] = KeymapRule::new(0x33, true, 0x34, false);
    rules[18] = KeymapRule::new(0x34, false, 0x24, true);
    rules[19] = KeymapRule::new(0x34, true, 0x1f, true);
    rules
};

impl Keymap {
    /// The empty map, useful as a fixed-capacity editing buffer.
    pub const fn new() -> Self {
        Self {
            rules: [KeymapRule::EMPTY; KEYMAP_RULE_CAPACITY],
            len: 0,
        }
    }

    /// The built-in ANSI US to JIS compatibility map.
    pub const US_JIS: Self = Self {
        rules: DEFAULT_US_JIS_RULES,
        len: 20,
    };

    /// Returns the number of populated rules.
    pub const fn len(self) -> usize {
        self.len as usize
    }

    /// Returns whether no mapping rules are populated.
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Returns one populated rule in insertion order.
    pub fn rule(&self, index: usize) -> Option<KeymapRule> {
        if index < self.len() {
            Some(self.rules[index])
        } else {
            None
        }
    }

    /// Returns whether an input usage and shift state already have a rule.
    pub fn contains_rule(&self, input_usage: u8, input_shifted: bool) -> bool {
        self.lookup(input_usage, input_shifted).is_some()
    }

    /// Inserts a rule or replaces the rule for the same input usage and shift.
    pub fn set_rule(&mut self, rule: KeymapRule) -> Result<(), KeymapError> {
        if rule.input_usage == 0 {
            return Err(KeymapError::InvalidInputUsage);
        }
        for index in 0..self.len() {
            if self.rules[index].matches(rule.input_usage, rule.input_shifted) {
                self.rules[index] = rule;
                return Ok(());
            }
        }
        if self.len() >= KEYMAP_RULE_CAPACITY {
            return Err(KeymapError::Full);
        }
        self.rules[self.len as usize] = rule;
        self.len += 1;
        Ok(())
    }

    fn lookup(&self, input_usage: u8, input_shifted: bool) -> Option<KeymapRule> {
        self.rules[..self.len()]
            .iter()
            .copied()
            .find(|rule| rule.matches(input_usage, input_shifted))
    }

    fn contains_mapping(&self, report: BootKeyboardReport) -> bool {
        let shifted = report.modifiers & SHIFT_MASK != 0;
        report
            .keys
            .iter()
            .any(|usage| self.lookup(*usage, shifted).is_some())
    }
}

impl Default for Keymap {
    fn default() -> Self {
        Self::new()
    }
}

/// Failure while editing a fixed-capacity keymap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeymapError {
    /// A zero source usage cannot identify an input key.
    InvalidInputUsage,
    /// The fixed rule capacity has been reached.
    Full,
}

/// Stable identifier for an attached physical input device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceId(pub u8);

impl SourceId {
    /// Returns the array position represented by this identifier.
    pub const fn index(self) -> Option<usize> {
        if (self.0 as usize) < MAX_SOURCES {
            Some(self.0 as usize)
        } else {
            None
        }
    }
}

/// The lifecycle state of one registered source slot.
///
/// The state is deliberately source-local. The BLE central's state can be
/// changing while the virtual source remains available, and a diagnostic
/// reader must be able to tell those cases apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SourceState {
    /// No source has been registered in this slot.
    Unregistered = 0,
    /// The source is known but is not currently attached.
    Disconnected = 1,
    /// A transport is attempting to establish a link.
    Connecting = 2,
    /// Link security is being established.
    Securing = 3,
    /// Transport discovery is in progress.
    Discovering = 4,
    /// The source can submit reports.
    Connected = 5,
    /// The most recent lifecycle operation failed.
    Failed = 6,
}

impl TryFrom<u8> for SourceState {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Unregistered),
            1 => Ok(Self::Disconnected),
            2 => Ok(Self::Connecting),
            3 => Ok(Self::Securing),
            4 => Ok(Self::Discovering),
            5 => Ok(Self::Connected),
            6 => Ok(Self::Failed),
            _ => Err(()),
        }
    }
}

/// The link through which a source is attached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputTransport {
    /// A directly attached USB HID keyboard.
    Usb,
    /// A Bluetooth Low Energy keyboard.
    Ble,
    /// A source injected through the configuration interface, with no
    /// physical peer. It must be distinguishable from USB host input.
    Virtual,
}

impl InputTransport {
    /// Stable byte used by the source diagnostics block.
    pub const fn wire_code(self) -> u8 {
        match self {
            Self::Usb => 1,
            Self::Ble => 2,
            Self::Virtual => 3,
        }
    }

    /// Decodes the source diagnostics transport byte.
    pub const fn from_wire_code(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Usb),
            2 => Some(Self::Ble),
            3 => Some(Self::Virtual),
            _ => None,
        }
    }
}

/// A fixed-capacity source name suitable for `no_std` firmware state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceName {
    bytes: [u8; SOURCE_NAME_LEN],
    len: u8,
}

impl SourceName {
    /// An empty name for an unregistered slot.
    pub const EMPTY: Self = Self {
        bytes: [0; SOURCE_NAME_LEN],
        len: 0,
    };

    /// Builds a name from a fixed array and an explicit byte length.
    ///
    /// This is a const constructor so board code can keep default names in
    /// static storage. Runtime callers should prefer [`Self::try_from_bytes`]
    /// when the length is not already known to fit.
    pub const fn from_raw(bytes: [u8; SOURCE_NAME_LEN], len: u8) -> Self {
        let bounded = if len as usize > SOURCE_NAME_LEN {
            SOURCE_NAME_LEN as u8
        } else {
            len
        };
        Self {
            bytes,
            len: bounded,
        }
    }

    /// Copies a name into the fixed-capacity representation.
    pub fn try_from_bytes(bytes: &[u8]) -> Result<Self, SourceNameError> {
        if bytes.len() > SOURCE_NAME_LEN {
            return Err(SourceNameError::TooLong(bytes.len()));
        }
        if core::str::from_utf8(bytes).is_err() {
            return Err(SourceNameError::InvalidUtf8);
        }
        let mut name = Self::EMPTY;
        name.bytes[..bytes.len()].copy_from_slice(bytes);
        name.len = bytes.len() as u8;
        Ok(name)
    }

    /// Returns the meaningful name bytes without exposing trailing storage.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    /// Returns the meaningful byte count.
    pub const fn len(self) -> u8 {
        self.len
    }

    /// Returns whether the fixed-capacity name contains no bytes.
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// Returns all fixed-width bytes for a wire encoder.
    pub const fn to_bytes(self) -> [u8; SOURCE_NAME_LEN] {
        self.bytes
    }
}

/// Failure while constructing a fixed-capacity source name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceNameError {
    /// The supplied bytes do not fit in the source record.
    TooLong(usize),
    /// The supplied bytes are not valid UTF-8.
    InvalidUtf8,
}

/// Stable identity facts for one source.
///
/// IRK bytes never appear in this public snapshot. The boolean records whether
/// private registration state contains an IRK so diagnostics can explain why
/// a source is trusted without turning diagnostics into a secret extractor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceIdentity {
    /// The immutable identity address, or `None` for a source without one.
    pub address: Option<[u8; 6]>,
    /// Whether private registration state contains an IRK.
    pub irk_present: bool,
}

impl SourceIdentity {
    /// Identity for an unregistered or virtual source.
    pub const NONE: Self = Self {
        address: None,
        irk_present: false,
    };

    /// Builds an identity from its immutable address and IRK presence.
    pub const fn from_address(address: [u8; 6], irk_present: bool) -> Self {
        Self {
            address: Some(address),
            irk_present,
        }
    }
}

/// Read-only source state returned by the bridge engine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceSnapshot {
    /// The registration slot, never an address-derived key.
    pub slot: SourceId,
    /// Registered transport, or `None` for an unused slot.
    pub transport: Option<InputTransport>,
    /// Current source-local lifecycle state.
    pub state: SourceState,
    /// Immutable identity facts and non-secret IRK presence.
    pub identity: SourceIdentity,
    /// Fixed-capacity user-visible name.
    pub name: SourceName,
    /// Profile applied to reports from this source.
    pub profile: BridgeProfile,
    /// Whether reports from this slot currently participate in aggregation.
    pub attached: bool,
}

/// A keyboard report in USB HID boot protocol form.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootKeyboardReport {
    /// Modifier bitmap as defined by HID Usage Tables.
    pub modifiers: u8,
    /// Six simultaneous key slots. A zero is an empty slot.
    pub keys: [u8; BOOT_KEY_SLOTS],
}

impl BootKeyboardReport {
    /// An empty report that releases every key.
    pub const EMPTY: Self = Self {
        modifiers: 0,
        keys: [0; BOOT_KEY_SLOTS],
    };

    /// Serializes this report for a USB HID boot endpoint.
    pub const fn to_bytes(self) -> [u8; 8] {
        [
            self.modifiers,
            0,
            self.keys[0],
            self.keys[1],
            self.keys[2],
            self.keys[3],
            self.keys[4],
            self.keys[5],
        ]
    }

    /// Reads back a report in the same wire form [`Self::to_bytes`] produces.
    ///
    /// Byte one is the reserved OEM byte and carries no state, so a round trip
    /// through the wire form is lossless.
    pub const fn from_bytes(bytes: [u8; 8]) -> Self {
        Self {
            modifiers: bytes[0],
            keys: [bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]],
        }
    }

    /// Whether a non-zero usage occupies one of the key slots.
    pub fn contains(&self, usage: u8) -> bool {
        usage != 0 && self.keys.contains(&usage)
    }
}

/// Per-keyboard compatibility behavior, represented as editable presets in the UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BridgeProfile {
    /// Apply the ANSI US to JIS compatibility preset.
    pub us_to_jis: bool,
    /// Convert Caps Lock presses into Left Control presses.
    pub caps_to_ctrl: bool,
    /// Swap left/right Alt and GUI modifiers.
    pub swap_alt_gui: bool,
}

impl BridgeProfile {
    /// A transparent profile with no conversion.
    pub const NONE: Self = Self {
        us_to_jis: false,
        caps_to_ctrl: false,
        swap_alt_gui: false,
    };
    /// Layout conversion only, with no other remapping.
    ///
    /// This is what a bridge should carry by default. The keyboard already has
    /// its own opinions about Caps Lock and about Alt against the meta key —
    /// the keyboard has switches for both — and applying them again here undoes
    /// them. Swapping Alt and GUI in particular breaks `Alt` + `` ` ``, which
    /// is how Windows toggles the IME: it arrives as `Meta` + `` ` `` and does
    /// nothing.
    pub const US_JIS: Self = Self {
        us_to_jis: true,
        caps_to_ctrl: false,
        swap_alt_gui: false,
    };
    /// An editable compatibility preset for common ANSI US to JIS usage.
    ///
    /// Bundles two remaps beyond the layout conversion its name describes.
    /// Opt into it deliberately; [`Self::US_JIS`] is the plain conversion.
    pub const US_JIS_PRESET: Self = Self {
        us_to_jis: true,
        caps_to_ctrl: true,
        swap_alt_gui: true,
    };
}

/// Result of merging reports from all sources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BridgeOutput {
    /// The report ready for the USB HID output adapter.
    pub report: BootKeyboardReport,
    /// More than six distinct non-modifier keys were held across sources.
    pub rollover: bool,
}

#[derive(Clone, Copy)]
struct SourceSlot {
    report: BootKeyboardReport,
    profile: BridgeProfile,
    transport: Option<InputTransport>,
    state: SourceState,
    identity: SourceIdentity,
    name: SourceName,
    attached: bool,
    /// Private registration material. No public snapshot exposes these bytes.
    #[allow(dead_code)]
    irk: [u8; IRK_LEN],
    irk_present: bool,
}

impl core::fmt::Debug for SourceSlot {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("SourceSlot")
            .field("report", &self.report)
            .field("profile", &self.profile)
            .field("transport", &self.transport)
            .field("state", &self.state)
            .field("identity", &self.identity)
            .field("name", &self.name)
            .field("attached", &self.attached)
            .field("irk_present", &self.irk_present)
            .finish()
    }
}

impl SourceSlot {
    const EMPTY: Self = Self {
        report: BootKeyboardReport::EMPTY,
        profile: BridgeProfile::NONE,
        transport: None,
        state: SourceState::Unregistered,
        identity: SourceIdentity::NONE,
        name: SourceName::EMPTY,
        attached: false,
        irk: [0; IRK_LEN],
        irk_present: false,
    };

    fn snapshot(self, slot: SourceId) -> SourceSnapshot {
        SourceSnapshot {
            slot,
            transport: self.transport,
            state: self.state,
            identity: SourceIdentity {
                address: self.identity.address,
                irk_present: self.irk_present,
            },
            name: self.name,
            profile: self.profile,
            attached: self.attached,
        }
    }
}

/// Fixed-capacity bridge state. It performs no allocation and is usable on MCU targets.
#[derive(Clone, Copy, Debug)]
pub struct BridgeEngine {
    slots: [SourceSlot; MAX_SOURCES],
    keymap: Keymap,
}

impl BridgeEngine {
    /// Creates a bridge with no registered or attached sources.
    pub const fn new() -> Self {
        Self {
            slots: [SourceSlot::EMPTY; MAX_SOURCES],
            keymap: Keymap::US_JIS,
        }
    }

    /// Registers a source slot without attaching its live report path.
    ///
    /// The IRK is copied into private fixed storage. Callers can later inspect
    /// only its presence through [`Self::source_snapshot`].
    pub fn register_source(
        &mut self,
        id: SourceId,
        transport: InputTransport,
        identity: SourceIdentity,
        name: SourceName,
        irk: Option<[u8; IRK_LEN]>,
        profile: BridgeProfile,
    ) -> Result<(), BridgeError> {
        let index = id.index().ok_or(BridgeError::UnknownSource)?;
        let (irk_bytes, irk_present) = match irk {
            Some(bytes) => (bytes, true),
            None => ([0; IRK_LEN], identity.irk_present),
        };
        self.slots[index] = SourceSlot {
            report: BootKeyboardReport::EMPTY,
            profile,
            transport: Some(transport),
            state: SourceState::Disconnected,
            identity: SourceIdentity {
                address: identity.address,
                irk_present,
            },
            name,
            attached: false,
            irk: irk_bytes,
            irk_present,
        };
        Ok(())
    }

    /// Attaches a source and assigns its editable compatibility profile.
    ///
    /// This compatibility entry point also registers an otherwise empty slot,
    /// retaining the original core API for adapters that do not carry identity
    /// metadata yet.
    pub fn attach(
        &mut self,
        id: SourceId,
        transport: InputTransport,
        profile: BridgeProfile,
    ) -> Result<(), BridgeError> {
        let index = id.index().ok_or(BridgeError::UnknownSource)?;
        if self.slots[index].transport.is_none() {
            self.slots[index].transport = Some(transport);
            self.slots[index].state = SourceState::Disconnected;
        }
        self.slots[index].profile = profile;
        self.slots[index].attached = true;
        self.slots[index].state = SourceState::Connected;
        self.slots[index].report = BootKeyboardReport::EMPTY;
        Ok(())
    }

    /// Changes a registered source's lifecycle state without changing ownership.
    pub fn set_source_state(
        &mut self,
        id: SourceId,
        state: SourceState,
    ) -> Result<(), BridgeError> {
        let index = self.registered_index(id)?;
        self.slots[index].state = state;
        self.slots[index].attached = state == SourceState::Connected;
        if !self.slots[index].attached {
            self.slots[index].report = BootKeyboardReport::EMPTY;
        }
        Ok(())
    }

    /// Returns one source slot, including its profile and current lifecycle.
    pub fn source_snapshot(&self, id: SourceId) -> Result<SourceSnapshot, BridgeError> {
        let index = id.index().ok_or(BridgeError::UnknownSource)?;
        Ok(self.slots[index].snapshot(id))
    }

    /// Returns every fixed-capacity source slot in slot-number order.
    pub fn source_snapshots(&self) -> [SourceSnapshot; MAX_SOURCES] {
        core::array::from_fn(|index| self.slots[index].snapshot(SourceId(index as u8)))
    }

    /// Alias for callers that describe a slot read as a lookup.
    pub fn lookup_source(&self, id: SourceId) -> Option<SourceSnapshot> {
        id.index().map(|index| self.slots[index].snapshot(id))
    }

    /// Returns one registered source's profile, even while it is disconnected.
    pub fn profile(&self, id: SourceId) -> Result<BridgeProfile, BridgeError> {
        Ok(self.registered_slot(id)?.profile)
    }

    /// Detaches a source and releases only keys attributable to it.
    pub fn detach(&mut self, id: SourceId) -> Result<BridgeOutput, BridgeError> {
        let index = self.attached_index(id)?;
        self.slots[index].attached = false;
        self.slots[index].state = SourceState::Disconnected;
        self.slots[index].report = BootKeyboardReport::EMPTY;
        Ok(self.output())
    }

    /// Replaces the profile of one registered source without changing its keys.
    ///
    /// The report pipeline releases the USB aggregate before calling this
    /// method. Keeping the core setter side-effect free preserves its useful
    /// in-process semantics and lets adapters choose their output boundary.
    pub fn set_profile(&mut self, id: SourceId, profile: BridgeProfile) -> Result<(), BridgeError> {
        let index = self.registered_index(id)?;
        self.slots[index].profile = profile;
        Ok(())
    }

    /// Returns the current data-driven keymap.
    pub const fn keymap(&self) -> Keymap {
        self.keymap
    }

    /// Replaces the data-driven keymap used by layout-enabled sources.
    ///
    /// Hardware adapters should release the aggregate report before calling
    /// this method, just as they do before changing a source profile.
    pub fn set_keymap(&mut self, keymap: Keymap) {
        self.keymap = keymap;
    }

    /// Processes a boot report from a connected source and returns the aggregate output.
    pub fn submit_boot_report(
        &mut self,
        id: SourceId,
        report: BootKeyboardReport,
    ) -> Result<BridgeOutput, BridgeError> {
        let index = self.attached_index(id)?;
        // Keep the physical report intact. Transforming only when producing
        // the aggregate output avoids applying a profile a second time and
        // lets output policy account for other currently-active sources.
        self.slots[index].report = report;
        Ok(self.output())
    }

    fn registered_index(&self, id: SourceId) -> Result<usize, BridgeError> {
        let index = id.index().ok_or(BridgeError::UnknownSource)?;
        if self.slots[index].transport.is_some() {
            Ok(index)
        } else {
            Err(BridgeError::UnregisteredSource)
        }
    }

    fn registered_slot(&self, id: SourceId) -> Result<SourceSlot, BridgeError> {
        let index = self.registered_index(id)?;
        Ok(self.slots[index])
    }

    fn attached_index(&self, id: SourceId) -> Result<usize, BridgeError> {
        let index = id.index().ok_or(BridgeError::UnknownSource)?;
        if self.slots[index].attached {
            Ok(index)
        } else {
            Err(BridgeError::DetachedSource)
        }
    }

    fn output(&self) -> BridgeOutput {
        let mut report = BootKeyboardReport::EMPTY;
        let mut rollover = false;
        let active_sources = self
            .slots
            .iter()
            .filter(|slot| {
                slot.attached
                    && (slot.report.modifiers != 0
                        || slot.report.keys.iter().any(|usage| *usage != 0))
            })
            .count();
        for slot in &self.slots {
            if !slot.attached {
                continue;
            }
            // Boot keyboard output has one modifier bitmap for every merged
            // source. A per-source US-to-JIS symbol conversion can require a
            // different Shift state, so it is unsafe while keys from another
            // source are down. Keep the original report rather than emit a
            // corrupted chord; an NKRO/event output adapter will remove this
            // conservative limitation later.
            let mut profile = slot.profile;
            if active_sources > 1 && profile.us_to_jis && self.keymap.contains_mapping(slot.report)
            {
                profile.us_to_jis = false;
            }
            let source = transform(slot.report, profile, self.keymap);
            report.modifiers |= source.modifiers;
            for usage in source.keys {
                if usage == 0 || report.contains(usage) {
                    continue;
                }
                if let Some(output_slot) = report
                    .keys
                    .iter_mut()
                    .find(|output_slot| **output_slot == 0)
                {
                    *output_slot = usage;
                } else {
                    rollover = true;
                }
            }
        }
        BridgeOutput { report, rollover }
    }
}

impl Default for BridgeEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors returned for invalid bridge source operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BridgeError {
    /// The source identifier is outside the prototype's fixed capacity.
    UnknownSource,
    /// The source slot is within capacity but has not been registered.
    UnregisteredSource,
    /// The source is not currently attached.
    DetachedSource,
}

fn transform(
    mut report: BootKeyboardReport,
    profile: BridgeProfile,
    keymap: Keymap,
) -> BootKeyboardReport {
    if profile.swap_alt_gui {
        swap_alt_gui(&mut report.modifiers);
    }
    if profile.caps_to_ctrl {
        for usage in &mut report.keys {
            if *usage == CAPS_LOCK {
                report.modifiers |= LEFT_CTRL;
                *usage = 0;
            }
        }
    }
    if profile.us_to_jis {
        map_us_to_jis(&mut report, keymap);
    }
    report
}

fn swap_alt_gui(modifiers: &mut u8) {
    let left_alt = *modifiers & LEFT_ALT != 0;
    let left_gui = *modifiers & LEFT_GUI != 0;
    let right_alt = *modifiers & RIGHT_ALT != 0;
    let right_gui = *modifiers & RIGHT_GUI != 0;
    *modifiers &= !(LEFT_ALT | LEFT_GUI | RIGHT_ALT | RIGHT_GUI);
    if left_alt {
        *modifiers |= LEFT_GUI;
    }
    if left_gui {
        *modifiers |= LEFT_ALT;
    }
    if right_alt {
        *modifiers |= RIGHT_GUI;
    }
    if right_gui {
        *modifiers |= RIGHT_ALT;
    }
}

fn map_us_to_jis(report: &mut BootKeyboardReport, keymap: Keymap) {
    // Preserve OS/application shortcuts. Their physical US usages must not be
    // rewritten into text-entry usages just because a compatibility preset is
    // active.
    if report.modifiers & SHORTCUT_MODIFIER_MASK != 0 {
        return;
    }
    let shifted = report.modifiers & SHIFT_MASK != 0;
    let mut desired_shift = None;
    let mut changed = false;
    let mut pressed = 0;
    let mut changed_count = 0;
    for usage in report.keys {
        if usage == 0 {
            continue;
        }
        pressed += 1;
        if let Some(rule) = keymap.lookup(usage, shifted) {
            let target_shift = rule.output_shifted;
            if let Some(previous_shift) = desired_shift {
                // One boot report cannot express two different synthesized
                // Shift states. Preserve it instead of applying the last
                // mapping's modifier to every key.
                if previous_shift != target_shift {
                    return;
                }
            }
            changed = true;
            changed_count += 1;
            desired_shift = Some(target_shift);
        }
    }
    // A boot report cannot represent two independent shift states. Preserve the
    // source report rather than corrupting a chord with incompatible mappings.
    let desired_shift = desired_shift.unwrap_or(shifted);
    if !changed || (changed_count != pressed && desired_shift != shifted) {
        return;
    }
    for usage in &mut report.keys {
        if let Some(rule) = keymap.lookup(*usage, shifted) {
            *usage = rule.output_usage;
        }
    }
    if desired_shift {
        report.modifiers |= LEFT_SHIFT;
    } else {
        report.modifiers &= !SHIFT_MASK;
    }
}
