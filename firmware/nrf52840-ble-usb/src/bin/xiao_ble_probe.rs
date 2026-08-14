#![no_std]
#![no_main]

//! Coexistence probe: does the proven USB boundary survive MPSL and the
//! SoftDevice Controller running alongside it?
//!
//! This is `ukf-xiao-usb-hid` plus a live BLE controller and nothing else. It
//! does not scan, advertise, connect, or send a single HCI command after
//! start-up. The only question it answers is whether MPSL taking CLOCK, RTC0,
//! TIMER0, RADIO, and its PPI channels breaks USB enumeration or the software
//! reset that were accepted on hardware on 2026-08-11.
//!
//! Eight earlier bridge candidates failed because each one changed several
//! unverified things at once. This one changes exactly one.
//!
//! `CLOCK_POWER` carries both MPSL's clock handler and the USB VBUS detect
//! handler. `bind_interrupts!` chains them, so `HardwareVbusDetect` keeps
//! working and the accepted USB configuration does not have to change. The
//! same arrangement is used by the `rmk` keyboard firmware.

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, Ordering};

use core::cell::RefCell;
use cortex_m::peripheral::SCB;
use embassy_executor::Spawner;
use embassy_futures::join::join3;
use embassy_futures::select::{Either, Either3, Either5, select, select3, select5};
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_nrf::peripherals::{RNG, USBD};
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::{bind_interrupts, interrupt, pac, rng, usb};

use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Instant, TimeoutError, Timer, with_timeout};
use embassy_usb::class::hid::{
    Config as HidConfig, HidBootProtocol, HidSubclass, HidWriter, ReportId, RequestHandler, State,
};
use embassy_usb::control::OutResponse;
use embassy_usb::{Builder, Config};
use nrf_sdc::mpsl::MultiprotocolServiceLayer;
use nrf_sdc::{self as sdc, mpsl};
use static_cell::StaticCell;
use trouble_host::connection::ConnectionEvent;
use trouble_host::prelude::*;
use ukf_core::{
    BootKeyboardReport, BridgeProfile, InputTransport, Keymap, SOURCE_SLOT_COUNT, SourceId,
    SourceIdentity, SourceName, SourceState, StuckKeyWatchdog,
};
use ukf_nrf52840_ble_usb::bond_management::{
    BondManagementRequest, decode as decode_bond_management,
};
use ukf_nrf52840_ble_usb::bond_store::{
    BOND_PAGE_DATA_LEN, BOND_SLOT_COUNT, BOND_SLOT_RECORD_LEN, StoredBond, StoredBondSlot,
    decode_bond_page, encode_bond_slot, encode_bond_tombstone,
};
use ukf_nrf52840_ble_usb::central_policy::{CentralEvent, CentralPolicy};
use ukf_nrf52840_ble_usb::config_hid::{
    CONFIG_CONTROL_BUFFER_LEN, CONFIG_REPORT_DESCRIPTOR, ConfigRequest, classify_feature_report,
};
use ukf_nrf52840_ble_usb::config_transfer::{
    ConfigTransfer, TARGET_BOND_MANAGEMENT, TARGET_KEYMAP, TARGET_PROFILE, TARGET_SOURCE_PROFILE,
    TransferError,
};
use ukf_nrf52840_ble_usb::configuration_updates::ConfigurationUpdates;
use ukf_nrf52840_ble_usb::diagnostics_report::{
    self, BridgeState, DiagnosticsReport, SELECT_SOURCE, SourceReport, TransferReport,
};
use ukf_nrf52840_ble_usb::hogp::{HogpCentral, ReportCharacteristic};
use ukf_nrf52840_ble_usb::keymap_store::{
    KEYMAP_RECORD_LEN, KeymapRecordError, decode_keymap_record, encode_keymap_payload,
    encode_keymap_record,
};
use ukf_nrf52840_ble_usb::keymap_store::{KeymapPayloadError, decode_keymap_payload};
use ukf_nrf52840_ble_usb::multi_link::{LinkWorkerId, MAX_ACTIVE_BLE_LINKS, MultiLinkSet};
use ukf_nrf52840_ble_usb::profile_store::{
    PROFILE_RECORD_LEN, PendingProfile, ProfileRecordError, StoredProfile, decode_profile,
    decode_profile_payload, decode_source_profile_payload, encode_profile,
};
use ukf_nrf52840_ble_usb::s140::advertisement_has_hid_service;
use ukf_nrf52840_ble_usb::uf2_reset::{CONFIG_REPORT_LEN, UF2_RESET_MAGIC};
use ukf_nrf52840_ble_usb::{
    BleSourceRegistration, ReportPipeline, UsbReportSink, is_runtime_source_slot,
};
use usbd_hid::descriptor::{KeyboardReport, SerializedDescriptor};

/// Approved application identity for the independent bridge prototype.
const USB_VENDOR_ID: u16 = 0x1209;
/// Approved application product identity for the independent bridge prototype.
const USB_PRODUCT_ID: u16 = 0x0001;

/// Grace period letting the reset request's status stage reach the host.
const RESET_GRACE: Duration = Duration::from_millis(50);

/// How long to wait for the keys to be released before resetting anyway.
///
/// A board that will not return to the bootloader is worse than a key left
/// down: the second is undone by the next start-up, while the first can only be
/// recovered by the physical double reset this software path exists to avoid.
const RESET_RELEASE_TIMEOUT: Duration = Duration::from_millis(150);

// `reset_xiao.py` waits for the UF2 volume to appear. Everything added before
// the reset delays that, and a helper that looks hung is a helper people stop
// trusting.
const _: () = assert!(
    RESET_RELEASE_TIMEOUT.as_ticks() + RESET_GRACE.as_ticks()
        < Duration::from_millis(500).as_ticks(),
    "a reset must not be delayed long enough to look like a hang"
);
/// Interval at which the reset request flag is observed.
const RESET_POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Delay before the radio starts, so USB always enumerates first.
///
/// A panic anywhere in the radio path resets the whole MCU, taking the
/// configuration interface with it. Without this window the board can crash
/// before the host ever enumerates it, and the panic record — the one thing
/// that explains the crash — is unreadable.
const RADIO_START_DELAY: Duration = Duration::from_secs(15);

/// Bound on a single connection attempt.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// How many times to ask the scheduler for the timeslot the write needs.
const BOND_WRITE_ATTEMPTS: u8 = 6;
/// Gap between attempts, long enough for several connection events to pass.
const BOND_WRITE_RETRY_DELAY: Duration = Duration::from_millis(250);

/// How long to keep listening after encryption for the keys of a new pairing.
///
/// Short enough that a peer which sends nothing further does not hold up
/// discovery, long enough to cover the gap between the two events on a link
/// this slow.
const PAIRING_KEYS_GRACE: Duration = Duration::from_millis(1500);

/// Time allowed for the runner to actually disable scanning before connecting.
const SCAN_STOP_SETTLE: Duration = Duration::from_millis(50);

/// How long the host may hold a key with no proof the keyboard is still there.
///
/// The danger this bounds is a key the user has already let go of staying down
/// on the host. It happened: a meta key stayed down and broke Japanese input on
/// the machine's own keyboard, which this bridge never touches.
const STUCK_KEY_LIMIT: Duration = Duration::from_secs(5);

/// Silence after which a held key is worth asking the keyboard about.
///
/// A HID keyboard reports changes, so holding a key produces no traffic at all.
/// Silence therefore proves nothing on its own and the bridge has to ask.
const LIVENESS_PROBE_IDLE: Duration = Duration::from_secs(1);

/// How long to wait for the keyboard to answer a liveness probe.
///
/// An ATT round trip on this link takes about a connection interval, so this is
/// dozens of connection events of margin. The link is declared dead only when
/// nothing at all comes back.
const LIVENESS_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

// The longest a probe can leave the watchdog without evidence is one idle
// period plus one probe timeout. If that ever exceeded the limit, the watchdog
// would release keys on a healthy link while the probe that would have saved
// them was still in flight.
const _: () = assert!(
    LIVENESS_PROBE_IDLE.as_ticks() + LIVENESS_PROBE_TIMEOUT.as_ticks() < STUCK_KEY_LIMIT.as_ticks(),
    "a liveness probe must be able to finish before the stuck-key limit"
);

/// Controller memory.
///
/// Deliberately generous. `Builder::build` refuses to start when this is too
/// small and only warns when it is too large, so overshooting costs unused RAM
/// while undershooting costs a whole flash-and-read cycle to discover.
const SDC_MEMORY_BYTES: usize = 8192;

/// Weakest advertiser this bridge will try to connect to, in dBm.
///
/// This was set to -75 on the assumption that a keyboard on the same desk
/// reports around -60. A strict floor can reject the intended keyboard when
/// the antenna or enclosure attenuates the signal. Absolute signal strength
/// turns out to be a poor way to tell the intended peer from a stranger here.
///
/// What actually separates them is persistence: the keyboard in pairing mode
/// produced thousands of advertisements while the unrelated device produced
/// two. The candidate signal always holds the newest advertiser, so the bridge
/// converges on whichever peer keeps advertising. The floor is kept only as a
/// sanity bound against noise at the edge of the receiver.
const NEARBY_RSSI_FLOOR: i8 = -95;

/// Flash address of the stored pairing.
///
/// The last page of the application region, held back from the linker in
/// `memory.x`. The page above belongs to the bootloader.
const BOND_STORAGE_OFFSET: u32 = 0x000E_B000;
/// Flash address of the stored compatibility profile.
///
/// This is a separate page immediately below the bond page. Updating a
/// profile must never erase the pairing keys, because a power loss during the
/// update would otherwise turn a harmless configuration change into a new
/// pairing requirement.
const PROFILE_STORAGE_OFFSET: u32 = 0x000E_A000;
/// Flash address of the stored data-driven keymap.
const KEYMAP_STORAGE_OFFSET: u32 = 0x000E_9000;
/// Erase granularity of this part.
const FLASH_PAGE_LEN: u32 = 4096;

/// Outgoing L2CAP buffers per link, as in the `trouble-host` central example.
const L2CAP_TX_QUEUE: u8 = 3;
/// Incoming L2CAP buffers per link, as in the `trouble-host` central example.
const L2CAP_RX_QUEUE: u8 = 3;

/// Set by the configuration interface once a validated reset command arrives.
static RESET_REQUESTED: AtomicBool = AtomicBool::new(false);

/// What the next configuration read returns.
///
/// `0xff` is the status block, [`SELECT_PASSKEY`] is the pairing passkey, and
/// anything else names a slice of the recorded panic message. One selector
/// rather than one command per block keeps the host protocol to a single
/// round trip: name what you want, then read.
static PANIC_CHUNK: AtomicU8 = AtomicU8::new(0xff);
static KEYMAP_CHUNK: AtomicU8 = AtomicU8::new(0xff);
/// Source slot returned by the next source diagnostics read.
static SOURCE_SELECTOR: AtomicU8 = AtomicU8::new(0);

/// Whether a pairing from a previous boot was found in flash at start-up.
static BOND_LOADED: AtomicBool = AtomicBool::new(false);

/// Four registered bond slots, kept in RAM so diagnostics and scan filtering
/// never need to touch flash while the radio is active.
static BOND_SLOTS: BlockingMutex<
    CriticalSectionRawMutex,
    RefCell<[Option<StoredBondSlot>; BOND_SLOT_COUNT]>,
> = BlockingMutex::new(RefCell::new([None; BOND_SLOT_COUNT]));
/// Registered BLE source slots currently owning central links.
///
/// Bond slots are stable user-facing identities; worker indices are temporary
/// runtime resources. Keeping the active set by bond slot makes advertisement
/// filtering and bond management independent from worker reuse.
static ACTIVE_BLE_SLOTS: AtomicU8 = AtomicU8::new(0);
/// A bond-page rewrite is queued until a radio-quiet boundary.
static BOND_PAGE_DIRTY: AtomicBool = AtomicBool::new(false);
/// Logical slots that need one append-only record at the next quiet boundary.
static BOND_DIRTY_SLOTS: AtomicU8 = AtomicU8::new(0);
/// Which loaded records were accepted by the host stack at boot.
///
/// A flash record can be syntactically valid while the stack rejects it due to
/// capacity or an invalid security combination. Such a slot remains visible as
/// failed diagnostics, but it must not be selected as a reconnect candidate.
static BOND_STACK_LOADED_SLOTS: AtomicU8 = AtomicU8::new(0);

/// Address of the bonded keyboard, least significant byte first.
static BONDED_ADDRESS: [AtomicU8; 6] = [const { AtomicU8::new(0) }; 6];
/// Whether [`BONDED_ADDRESS`] names a real peer.
static HAS_BONDED_PEER: AtomicBool = AtomicBool::new(false);
/// Whether the private bond record contains an identity resolving key.
static HAS_BONDED_IRK: AtomicBool = AtomicBool::new(false);
/// Whether the bridge will pair with a keyboard it has never met.
///
/// Closed whenever a keyboard is bonded, and opened only on request. A bridge
/// sitting in pairing mode will adopt whatever advertises nearby, and for a
/// keyboard bridge that means someone else's device becoming a trusted source
/// of keystrokes.
///
/// A board with no bond at all is the exception: it opens on its own, because
/// there is nothing yet to be loyal to and no way to type the command that
/// would open it. This is what `ble-keyboard-bridge` does — its scan filters
/// fall through to matching any HID service when no peer is registered — and
/// it means first use needs no host-side tool. The moment a bond exists the
/// exception stops applying.
static PAIRING_MODE: AtomicBool = AtomicBool::new(true);

/// When the open pairing window closes on its own.
///
/// A window left open forever is the default this replaced, arrived at more
/// slowly. Two minutes is longer than pairing takes and short enough that
/// walking away does not leave the bridge adoptable.
const PAIRING_WINDOW: Duration = Duration::from_secs(120);

/// Set whenever the pairing window is opened, so the closer can time it.
static PAIRING_OPENED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Whether a pairing has been written to flash since start-up.
static BOND_SAVED: AtomicBool = AtomicBool::new(false);
/// Why the last notification was refused, or zero if it was accepted.
static NOTIFY_REFUSAL: AtomicU8 = AtomicU8::new(0);
/// Handle the last notification arrived on.
static NOTIFY_HANDLE: AtomicU16 = AtomicU16::new(0);
/// Handle the bridge subscribed to.
static NOTIFY_EXPECTED: AtomicU16 = AtomicU16::new(0);

/// Whether the next pairing must be authenticated with a passkey.
///
/// Just Works leaves pairing open to anyone in radio range at the moment it
/// happens, which for a bridge that carries keystrokes is a bring-up
/// convenience rather than a defensible default. Passkey entry closes that,
/// at the cost of the number having to reach a person within the thirty
/// seconds the specification allows for pairing — which is why it belongs in
/// the UI rather than in a message someone has to relay.
static REQUIRE_PASSKEY: AtomicBool = AtomicBool::new(false);
/// Raised when the method changes, so the stack is told without a restart.
static PAIRING_METHOD_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Whether attaching the link to the report path failed.
static LINK_SETUP_FAILED: AtomicBool = AtomicBool::new(false);
/// Why the USB half declined to subscribe, on its own `HogpCentral`.
static USB_HOGP_REFUSAL: AtomicU8 = AtomicU8::new(0);

/// Why `finish_discovery` refused, or zero if it has not refused.
static HOGP_REFUSAL: AtomicU8 = AtomicU8::new(0);

/// Whether the peer of the most recent connection was already bonded.
static PEER_WAS_BONDED: AtomicBool = AtomicBool::new(false);
/// Whether a pairing completed without producing keys to keep.
static BOND_PAIRED_WITHOUT_KEYS: AtomicBool = AtomicBool::new(false);
/// Whether keys were produced but could not be written.
static BOND_WRITE_FAILED: AtomicBool = AtomicBool::new(false);
/// Which flash operation refused and why. High nibble names the operation.
static BOND_FLASH_ERROR: AtomicU8 = AtomicU8::new(0);
/// Raw MPSL errno behind a refusal, magnitude only.
static BOND_FLASH_ERRNO: AtomicU16 = AtomicU16::new(0);

/// Flags of the profile currently used by both input sources.
///
/// The initial value is the built-in US-to-JIS profile. The radio task replaces
/// it after validating the flash record and the report task updates it only
/// after the new profile has been applied safely.
static ACTIVE_PROFILE_FLAGS: AtomicU8 = AtomicU8::new(1 << 0);
/// Lifecycle state mirrored into the source diagnostics blocks.
static SOURCE_STATES: [AtomicU8; SOURCE_SLOT_COUNT] = [
    AtomicU8::new(SourceState::Unregistered as u8),
    AtomicU8::new(SourceState::Unregistered as u8),
    AtomicU8::new(SourceState::Unregistered as u8),
    AtomicU8::new(SourceState::Unregistered as u8),
    AtomicU8::new(SourceState::Connected as u8),
];
/// Per-source profile flags mirrored after the report path applies them.
static SOURCE_PROFILE_FLAGS: [AtomicU8; SOURCE_SLOT_COUNT] = [
    AtomicU8::new(1 << 0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(1 << 0),
];

const VIRTUAL_SOURCE_NAME: SourceName = SourceName::from_raw(*b"Virtual Input", 13);
/// Whether the active profile came from a valid flash record.
static PROFILE_STORED: AtomicBool = AtomicBool::new(false);
/// Which profile flash operation refused, and why.
static PROFILE_FLASH_ERROR: AtomicU8 = AtomicU8::new(0);
/// Raw MPSL errno behind a profile flash refusal, magnitude only.
static PROFILE_FLASH_ERRNO: AtomicU16 = AtomicU16::new(0);
/// Whether the most recent profile write failed.
static PROFILE_WRITE_FAILED: AtomicBool = AtomicBool::new(false);
/// Why applying a successfully stored profile to the live report path failed.
static PROFILE_APPLY_ERROR: AtomicU8 = AtomicU8::new(0);
/// Why applying the latest volatile keymap to the live report path failed.
static KEYMAP_APPLY_ERROR: AtomicU8 = AtomicU8::new(0);
/// Which keymap flash operation refused and why.
static KEYMAP_FLASH_ERROR: AtomicU8 = AtomicU8::new(0);
/// Raw MPSL errno behind a keymap flash refusal, magnitude only.
static KEYMAP_FLASH_ERRNO: AtomicU16 = AtomicU16::new(0);
/// Whether the active keymap came from a valid flash record.
static KEYMAP_STORED: AtomicBool = AtomicBool::new(false);

/// How far GATT discovery got. See `diagnostics_report::DiagnosticsReport`.
///
/// Six of the nine steps report the same "not found" when they fail, so the
/// error code alone cannot say which gave up.
static DISCOVERY_STEP: AtomicU8 = AtomicU8::new(0);
/// Selector value asking for the passkey instead of a panic-message slice.
///
/// Chosen from the top of the range because the panic message is far shorter
/// than 254 chunks and always will be.
const SELECT_PASSKEY: u8 = 0xfe;

/// The passkey both sides enter during pairing.
///
/// Fixed rather than generated, because the number has to be known before
/// pairing starts: the thirty-second limit the spec puts on pairing is not
/// enough to read a freshly generated number off a host and type it on the
/// keyboard.
///
/// A fixed passkey is weaker than a random one — it removes the guarantee that
/// an attacker in range cannot complete pairing — and this value is chosen for
/// bring-up, not for use. Before this leaves the bench it should be per-board
/// and settable, with the UI showing it while pairing is open.
const PAIRING_PASSKEY: u32 = 111_111;

/// Whether the bridge insists on an encrypted link before touching GATT.
///
/// Turned off for bring-up. Pairing and the rest of the pipeline — service
/// discovery, subscribing to input reports, forwarding them over USB — are
/// separate problems, and with this off the second can be exercised while the
/// first is unresolved. It also makes connect, disconnect, and reconnect
/// testable on their own.
///
/// A HOGP keyboard is entitled to refuse an unencrypted client, and most do.
/// If discovery fails here, that is an answer too.
///
/// **This must return to `true` before the bridge is used.** An unencrypted
/// link means keystrokes travel in the clear and any device in range can
/// impersonate the keyboard.
const REQUIRE_SECURITY: bool = true;

/// The passkey the peer is being asked to type, valid while pairing.
static PASSKEY: AtomicU32 = AtomicU32::new(0);
/// How many passkeys this boot has produced, so a reader can spot a new one.
static PASSKEY_SERIAL: AtomicU32 = AtomicU32::new(0);
/// HCI status the peer gave when it dropped the link during pairing.
static DISCONNECT_REASON: AtomicU8 = AtomicU8::new(0);
/// Reason the security manager reported a pairing failure, if it reported one.
static PAIRING_FAILURE: AtomicU8 = AtomicU8::new(0);

static BRIDGE_STATE: AtomicU8 = AtomicU8::new(BridgeState::Starting as u8);
static LAST_ERROR: AtomicU8 = AtomicU8::new(0);
static ADVERTISEMENTS_SEEN: AtomicU16 = AtomicU16::new(0);
static HID_ADVERTISEMENTS: AtomicU16 = AtomicU16::new(0);
static LAST_RSSI: AtomicU8 = AtomicU8::new(0);
/// HCI address kind of the last HID advertiser: public or random.
///
/// A random address carries its class in the two most significant bits. A
/// keyboard using privacy advertises a resolvable private address that rotates,
/// and that is not something a filter accept list entry can chase.
static LAST_ADDR_KIND: AtomicU8 = AtomicU8::new(0xff);
static CONNECTION_COUNT: AtomicU8 = AtomicU8::new(0);
static INPUT_REPORTS_RECEIVED: AtomicU32 = AtomicU32::new(0);
static REPORTS_FORWARDED: AtomicU32 = AtomicU32::new(0);
static LAST_ADDRESS: [AtomicU8; 6] = [
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
];

/// Keeps the synchronous control request from waiting on the keyboard endpoint.
static INJECTED_REPORTS: Channel<CriticalSectionRawMutex, [u8; 8], 4> = Channel::new();

/// Whether the host is currently holding a key this bridge sent.
///
/// Read by the radio half, which only spends a probe on a link when letting it
/// die quietly would leave a key down.
static KEYS_HELD: AtomicBool = AtomicBool::new(false);
/// How many times the stuck-key watchdog released the host.
static STUCK_KEY_RELEASES: AtomicU8 = AtomicU8::new(0);
/// How many liveness probes went unanswered.
static LIVENESS_PROBE_FAILURES: AtomicU8 = AtomicU8::new(0);
/// Why the last probe was treated as a dead link, or zero.
static LIVENESS_PROBE_ERROR: AtomicU8 = AtomicU8::new(0);
/// Why the last injected report was refused, or zero if it was forwarded.
static INJECT_REFUSAL: AtomicU8 = AtomicU8::new(0);

/// Raised when a reset is imminent, so the report path can release the host.
static RELEASE_FOR_RESET: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Raised once that release has been handed to the keyboard endpoint.
static RELEASED_FOR_RESET: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Releases every held key, then resets into the UF2 bootloader.
///
/// The start-up report already ends a key left down by a reset — but only once
/// the application comes back, and a reset into the bootloader does not come
/// back. Reflashing while a key was held therefore left it down on the host for
/// as long as the board sat in UF2, which is exactly how a stuck meta key broke
/// Japanese input on the machine's own keyboard.
///
/// This covers the resets the firmware itself starts. A panic cannot use it:
/// the panic handler is synchronous and there is no way to await a USB write
/// from it, so that path still depends on the start-up report.
async fn release_then_reset_into_uf2() -> ! {
    RELEASE_FOR_RESET.signal(());
    if with_timeout(RESET_RELEASE_TIMEOUT, RELEASED_FOR_RESET.wait())
        .await
        .is_err()
    {
        // Deliberately unrecorded: RAM does not survive the reset, and the one
        // register that does is carrying the bootloader magic. Waiting longer
        // would strand the board, so this resets with the keys possibly still
        // down and lets the next start-up clear them.
    }
    Timer::after(RESET_GRACE).await;
    reset_into_uf2_bootloader()
}

/// Milliseconds since boot, in the form the watchdog takes.
fn now_ms() -> u64 {
    Instant::now().as_millis()
}

/// Sleeps until the watchdog is due, or forever while nothing is held.
///
/// `pending` rather than a long timer: an arbitrary poll interval would either
/// wake this task for nothing or delay a release, and neither is a choice worth
/// making when the deadline is already known exactly.
async fn stuck_key_deadline(deadline_ms: Option<u64>) {
    match deadline_ms {
        Some(deadline) => Timer::at(Instant::from_millis(deadline)).await,
        None => core::future::pending().await,
    }
}

/// Adds one without wrapping. A counter back at zero reads as a healthy run.
fn count_one(counter: &AtomicU8) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}

#[derive(Clone, Copy)]
enum RadioEvent {
    Connected {
        slot: u8,
    },
    /// The keyboard answered, so whatever the host is holding is still real.
    ///
    /// Sent only while a key is down: it exists to hold the stuck-key watchdog
    /// open, and there is nothing to hold open otherwise.
    LinkAlive {
        slot: u8,
    },
    /// Discovery finished. The report map itself is left in the slot-indexed
    /// report-map table.
    /// rather than carried here: every slot of the event channel would
    /// otherwise reserve 512 bytes for a value sent once per connection.
    Discovered {
        slot: u8,
        report_map_len: u16,
        reports: [ReportCharacteristic; 8],
        report_count: u8,
    },
    Notification {
        slot: u8,
        handle: u16,
        payload: [u8; 8],
    },
    Disconnected {
        slot: u8,
    },
}

static RADIO_EVENTS: Channel<CriticalSectionRawMutex, RadioEvent, 2> = Channel::new();

#[derive(Clone, Copy)]
struct LinkAssignment {
    address: Address,
    rssi: i8,
    slot: u8,
}

/// One assignment queue per worker. A worker owns all GATT state for its link.
static LINK_ASSIGNMENTS: [Channel<CriticalSectionRawMutex, LinkAssignment, 1>;
    MAX_ACTIVE_BLE_LINKS] = [const { Channel::new() }; MAX_ACTIVE_BLE_LINKS];
/// Lets the supervisor reclaim a worker after it has announced its disconnect.
static LINK_RELEASES: Channel<CriticalSectionRawMutex, (LinkWorkerId, u8), MAX_ACTIVE_BLE_LINKS> =
    Channel::new();
/// The supervisor must not start another discovery scan while a worker still
/// owns the controller's connection-establishment command. nRF-SDC reports
/// HCI 0x0C (Command Disallowed) when those two operations overlap.
static LINK_CONNECT_BOUNDARIES: Channel<
    CriticalSectionRawMutex,
    (LinkWorkerId, u8),
    MAX_ACTIVE_BLE_LINKS,
> = Channel::new();

/// The peer's HID report map, written once per connection by the radio task and
/// read once by the USB task. Both halves keep their own `HogpCentral`, so the
/// map has to cross the boundary; parking it here keeps 512 bytes out of every
/// slot of the event channel.
static REPORT_MAPS: BlockingMutex<CriticalSectionRawMutex, RefCell<[[u8; 512]; BOND_SLOT_COUNT]>> =
    BlockingMutex::new(RefCell::new([[0; 512]; BOND_SLOT_COUNT]));

/// Fixed-capacity configuration transfer state shared by USB and diagnostics.
static CONFIG_TRANSFER: BlockingMutex<CriticalSectionRawMutex, RefCell<ConfigTransfer>> =
    BlockingMutex::new(RefCell::new(ConfigTransfer::new()));

/// Profile commits are accepted by the synchronous USB handler and staged by
/// the radio task, which owns the MPSL timeslot-aware Flash object.
#[derive(Clone, Copy)]
enum RadioControl {
    Profile(StoredProfile),
    Keymap(Keymap),
    Bond(BondManagementRequest),
}

/// USB configuration writes are staged by the radio owner at a quiet
/// boundary, just like profile flash writes.
static RADIO_CONTROL_REQUESTS: Channel<CriticalSectionRawMutex, RadioControl, 2> = Channel::new();
/// Lets the report task wait for the first profile read before constructing its
/// pipeline, even though the radio task starts with a deliberate USB window.
static PROFILE_READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Lets the report task wait until the persisted BLE registrations are loaded.
///
/// `PROFILE_READY` alone was too early: the radio task reads the profile before
/// it restores bond slots, so an idle boot could construct a pipeline with only
/// the virtual source. A later BLE connection repaired that registration as a
/// side effect, which hid the race until a profile write was made with no link.
static BONDS_READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();

struct AdvertisementHandler;

impl EventHandler for AdvertisementHandler {
    fn on_adv_reports(&self, reports: trouble_host::prelude::LeAdvReportsIter) {
        for report in reports.flatten() {
            ADVERTISEMENTS_SEEN.fetch_add(1, Ordering::Relaxed);
            if advertisement_has_hid_service(report.data) {
                HID_ADVERTISEMENTS.fetch_add(1, Ordering::Relaxed);
                let address = Address::new(report.addr_kind, report.addr);
                let mut raw = report.addr.into_inner();
                raw.reverse();
                for (slot, byte) in LAST_ADDRESS.iter().zip(raw) {
                    slot.store(byte, Ordering::Relaxed);
                }
                LAST_RSSI.store(report.rssi as u8, Ordering::Relaxed);
                LAST_ADDR_KIND.store(report.addr_kind.into_inner(), Ordering::Relaxed);
                // Outside pairing mode only a registered keyboard is a
                // candidate. Pairing mode chooses the first free slot and
                // never opens when all four registration slots are occupied.
                let slot = if PAIRING_MODE.load(Ordering::Relaxed) {
                    first_free_bond_slot()
                } else {
                    bonded_slot_for_address(report.addr_kind.into_inner(), report.addr.into_inner())
                };
                if let Some(slot) = slot
                    && report.rssi >= NEARBY_RSSI_FLOOR
                    && !source_is_active(slot)
                {
                    HID_CANDIDATES.signal((address, report.rssi, slot));
                }
            }
        }
    }
}

/// The most recently advertised HID peer.
///
/// A queue is the wrong shape here. Advertisements arrive continuously, so a
/// bounded channel fills with whatever was seen first and then silently drops
/// everything after it — the connect attempt would keep targeting a stale peer
/// that may no longer be advertising. A signal always holds the newest.
static HID_CANDIDATES: Signal<CriticalSectionRawMutex, (Address, i8, u8)> = Signal::new();

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<USBD>;
    RNG => rng::InterruptHandler<RNG>;
    EGU0_SWI0 => mpsl::LowPrioInterruptHandler;
    // Both owners of this vector are chained rather than one displacing the
    // other. This is what lets the accepted `HardwareVbusDetect` survive.
    CLOCK_POWER => mpsl::ClockInterruptHandler, usb::vbus_detect::InterruptHandler;
    RADIO => mpsl::HighPrioInterruptHandler;
    TIMER0 => mpsl::HighPrioInterruptHandler;
    RTC0 => mpsl::HighPrioInterruptHandler;
});

/// Requests the UF2 bootloader on the next boot and resets the MCU.
fn reset_into_uf2_bootloader() -> ! {
    pac::POWER
        .gpregret()
        .write(|w| w.set_gpregret(UF2_RESET_MAGIC));
    SCB::sys_reset()
}

/// Panic record that survives the reset into the bootloader.
///
/// A panic resets the board, which is the right recovery but erases every
/// other signal: the board simply reappears as a UF2 volume with no
/// explanation. RAM keeps its contents across a software reset, so parking the
/// source line in an uninitialised section and checking a magic on the way back
/// up turns a silent reboot into a diagnosable crash.
#[unsafe(link_section = ".uninit.UKF_PANIC_RECORD")]
static mut PANIC_RECORD: PanicRecord = PanicRecord {
    magic: 0,
    line: 0,
    count: 0,
    message_len: 0,
    message: [0; PANIC_MESSAGE_LEN],
};

/// Bytes of panic message kept. Enough for the SoftDevice Controller's
/// `SoftdeviceController: <file>:<line>`, which is the message that matters.
const PANIC_MESSAGE_LEN: usize = 156;

#[repr(C)]
struct PanicRecord {
    magic: u32,
    line: u32,
    count: u32,
    message_len: u32,
    message: [u8; PANIC_MESSAGE_LEN],
}

/// Formats into a fixed buffer, discarding anything past the end.
struct MessageSink {
    buffer: [u8; PANIC_MESSAGE_LEN],
    len: usize,
}

impl core::fmt::Write for MessageSink {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        for byte in text.as_bytes() {
            if self.len == self.buffer.len() {
                break;
            }
            self.buffer[self.len] = *byte;
            self.len += 1;
        }
        Ok(())
    }
}

/// Chosen arbitrarily; it only has to be improbable in uninitialised RAM.
const PANIC_MAGIC: u32 = 0x554b_4650;

/// The stage the radio task had reached, recorded alongside a panic.
///
/// The SoftDevice Controller is distributed as a binary, so the file and line
/// its fault handler reports cannot be looked up even when they arrive intact.
/// What is actionable is which of *our* steps was in flight when it asserted,
/// and that this side can answer.
static LAST_PHASE: AtomicU8 = AtomicU8::new(0);

/// Opcode of the last HCI command handed to the controller.
static LAST_OPCODE: AtomicU16 = AtomicU16::new(0);
/// Opcode of the last HCI command the controller finished answering.
static COMPLETED_OPCODE: AtomicU16 = AtomicU16::new(0);

/// The most recent HCI commands, oldest first once wrapped.
///
/// One opcode names the command that was in flight but not the sequence that
/// led to it, and the sequence is the question here: whether scanning was
/// actually disabled before the initiator started is visible only in the order.
static OPCODE_RING: [AtomicU16; OPCODE_RING_LEN] = [const { AtomicU16::new(0) }; OPCODE_RING_LEN];
static OPCODE_RING_INDEX: AtomicU8 = AtomicU8::new(0);
const OPCODE_RING_LEN: usize = 16;

/// Appends one opcode to the ring.
fn trace_opcode(opcode: u16) {
    let slot = OPCODE_RING_INDEX.fetch_add(1, Ordering::Relaxed) as usize % OPCODE_RING_LEN;
    OPCODE_RING[slot].store(opcode, Ordering::Relaxed);
    LAST_OPCODE.store(opcode, Ordering::Relaxed);
}

/// Records every HCI command on its way to the controller.
///
/// The SoftDevice Controller ships as a binary, so the file and line its fault
/// handler reports cannot be looked up even when they arrive undamaged — and
/// here they arrive as four bytes of garbage. What this side can establish is
/// which command the controller was executing when it asserted, and comparing
/// the issued opcode against the completed one says whether it died inside that
/// command or after it.
struct TracingController<C>(C);

impl<C: embedded_io::ErrorType> embedded_io::ErrorType for TracingController<C> {
    type Error = C::Error;
}

impl<C: bt_hci::controller::Controller> bt_hci::controller::Controller for TracingController<C> {
    async fn write_acl_data(
        &self,
        packet: &bt_hci::data::AclPacket<'_>,
    ) -> Result<(), Self::Error> {
        self.0.write_acl_data(packet).await
    }

    async fn write_sync_data(
        &self,
        packet: &bt_hci::data::SyncPacket<'_>,
    ) -> Result<(), Self::Error> {
        self.0.write_sync_data(packet).await
    }

    async fn write_iso_data(
        &self,
        packet: &bt_hci::data::IsoPacket<'_>,
    ) -> Result<(), Self::Error> {
        self.0.write_iso_data(packet).await
    }

    async fn read<'a>(
        &self,
        buf: &'a mut [u8],
    ) -> Result<bt_hci::ControllerToHostPacket<'a>, Self::Error> {
        self.0.read(buf).await
    }
}

impl<C, Cmd> bt_hci::controller::ControllerCmdSync<Cmd> for TracingController<C>
where
    C: bt_hci::controller::ControllerCmdSync<Cmd>,
    Cmd: bt_hci::cmd::SyncCmd + ?Sized,
{
    async fn exec(&self, cmd: &Cmd) -> Result<Cmd::Return, bt_hci::cmd::Error<Self::Error>> {
        trace_opcode(Cmd::OPCODE.to_raw());
        let result = self.0.exec(cmd).await;
        COMPLETED_OPCODE.store(Cmd::OPCODE.to_raw(), Ordering::Relaxed);
        result
    }
}

impl<C, Cmd> bt_hci::controller::ControllerCmdAsync<Cmd> for TracingController<C>
where
    C: bt_hci::controller::ControllerCmdAsync<Cmd>,
    Cmd: bt_hci::cmd::AsyncCmd + ?Sized,
{
    async fn exec(&self, cmd: &Cmd) -> Result<(), bt_hci::cmd::Error<Self::Error>> {
        trace_opcode(Cmd::OPCODE.to_raw());
        let result = self.0.exec(cmd).await;
        COMPLETED_OPCODE.store(Cmd::OPCODE.to_raw(), Ordering::Relaxed);
        result
    }
}

/// Builds this board's static random address from its factory device ID.
///
/// The address bytes are little-endian, so index five is the most significant
/// byte, and a *static* random address is defined by its two most significant
/// bits being set. The literal this replaced was written most-significant byte
/// first, which put `0x01` in that position and produced an address the
/// controller has no reason to accept as an initiator identity.
///
/// Deriving it from `FICR.DEVICEID` is what the `nrf-sdc` example does. It also
/// makes the address unique per board and stable across resets, so a peer that
/// bonded with this bridge still recognises it after a reflash.
fn static_random_address() -> [u8; 6] {
    let ficr = pac::FICR;
    let low = u64::from(ficr.deviceid(0).read());
    let high = u64::from(ficr.deviceid(1).read());
    let address = (high << 32 | low) | 0x0000_c000_0000_0000;
    address.to_le_bytes()[..6]
        .try_into()
        .expect("six bytes taken from an eight-byte little-endian address")
}

/// Keeps the last path segment of a source file, dropping the directories.
///
/// A panic inside a dependency reports an absolute path into the cargo cache,
/// which is longer than the whole record and says nothing the file name does
/// not already say.
fn file_tail(path: &str) -> &str {
    let mut tail = path;
    for (index, byte) in path.as_bytes().iter().enumerate() {
        if *byte == b'/' || *byte == b'\\' {
            tail = &path[index + 1..];
        }
    }
    tail
}

/// Returns to the UF2 bootloader instead of halting on an unrecoverable fault.
#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    use core::fmt::Write as _;

    let location = info.location();
    let line = location.map_or(0, |location| location.line());
    let mut sink = MessageSink {
        buffer: [0; PANIC_MESSAGE_LEN],
        len: 0,
    };
    // The location alone is not enough. A panic raised inside a dependency
    // reports that dependency's file and line, which says nothing about the
    // condition that failed; `nrf-sdc` in particular funnels every controller
    // assertion through one line and puts the real detail in the message.
    //
    // The trailing sentinel separates a genuinely short message from a format
    // that stopped early. The first capture read `SoftdeviceController: `
    // followed by six bytes and no `:` separator, which the format string says
    // is impossible; without a terminator there is no way to tell which half of
    // that is wrong, and guessing is what this record exists to avoid.
    let _ = write!(
        sink,
        "{}@{}#{:02x}",
        info.message(),
        location.map_or("?", |location| file_tail(location.file())),
        LAST_PHASE.load(Ordering::Relaxed),
    );
    // Oldest first, so the command sequence reads left to right.
    let next = OPCODE_RING_INDEX.load(Ordering::Relaxed) as usize;
    for offset in 0..OPCODE_RING_LEN {
        let slot = (next + offset) % OPCODE_RING_LEN;
        let _ = write!(sink, ".{:04x}", OPCODE_RING[slot].load(Ordering::Relaxed));
    }
    // The peer, most significant byte first. Its top two bits classify a random
    // address, which decides whether a filter accept list entry can even name
    // this keyboard.
    let _ = write!(sink, "!{:02x}:", LAST_ADDR_KIND.load(Ordering::Relaxed));
    for byte in &LAST_ADDRESS {
        let _ = write!(sink, "{:02x}", byte.load(Ordering::Relaxed));
    }
    let _ = write!(sink, "<");
    // Safety: the panic handler cannot be re-entered, and by the time it runs
    // no other context is executing.
    unsafe {
        let record = &raw mut PANIC_RECORD;
        let previous = if (*record).magic == PANIC_MAGIC {
            (*record).count
        } else {
            0
        };
        (*record).magic = PANIC_MAGIC;
        (*record).line = line;
        (*record).count = previous.saturating_add(1);
        (*record).message_len = sink.len as u32;
        (*record).message = sink.buffer;
    }
    reset_into_uf2_bootloader()
}

/// Copies one slice of the recorded panic message out of the record.
fn panic_message_chunk(chunk: u8, dest: &mut [u8]) -> usize {
    // Safety: read long after the panic handler for this boot could run.
    unsafe {
        let record = &raw const PANIC_RECORD;
        if (*record).magic != PANIC_MAGIC {
            return 0;
        }
        let len = ((*record).message_len as usize).min(PANIC_MESSAGE_LEN);
        let start = usize::from(chunk) * diagnostics_report::PANIC_TEXT_CHUNK_LEN;
        if start >= len {
            return 0;
        }
        let end = (start + diagnostics_report::PANIC_TEXT_CHUNK_LEN).min(len);
        let taken = end - start;
        let message = &raw const (*record).message;
        core::ptr::copy_nonoverlapping((message as *const u8).add(start), dest.as_mut_ptr(), taken);
        taken
    }
}

/// Reads the panic record left by a previous run, if there is one.
///
/// Only the line survives, and `PanicInfo::location` reports whichever file
/// actually panicked — usually a dependency rather than this binary. The record
/// also persists across a reflash, so a line from a previous build can be read
/// back and misattributed. Both are why the reader labels it as a bare line
/// number rather than a source location.
fn last_panic() -> (u16, u8) {
    // Safety: read once from the diagnostics snapshot, long after the panic
    // handler for this boot could be running.
    unsafe {
        let record = &raw const PANIC_RECORD;
        if (*record).magic != PANIC_MAGIC {
            return (0, 0);
        }
        ((*record).line as u16, (*record).count.min(255) as u8)
    }
}

#[embassy_executor::task]
async fn mpsl_task(mpsl: &'static MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

/// Validates configuration feature reports and records a reset request.
struct ConfigRequestHandler;

/// Turns a transfer operation into the HID acknowledgement.
///
/// The typed error is retained by `ConfigTransfer` for the next diagnostic
/// read; the HID control request itself only has accepted/rejected outcomes.
fn transfer_response(result: Result<(), TransferError>) -> OutResponse {
    match result {
        Ok(()) => OutResponse::Accepted,
        Err(_) => OutResponse::Rejected,
    }
}

/// Packs the profile payload validation failure into the profile diagnostics
/// state without changing the generic transfer protocol's scratch behavior.
fn profile_payload_error_code(error: &ProfileRecordError) -> u8 {
    match error {
        ProfileRecordError::Length(_) => 1,
        ProfileRecordError::Empty => 2,
        ProfileRecordError::Version(_) => 3,
        ProfileRecordError::Crc { .. } => 4,
        // Kept apart from Empty: one says nobody has written a profile, the
        // others say one was written and cannot be trusted. Answering the
        // second with the default would adopt a profile nobody chose.
        ProfileRecordError::Reserved => 5,
        ProfileRecordError::UnknownFlags(_) => 6,
        ProfileRecordError::PayloadLength(_) => 7,
        ProfileRecordError::SourceSlot(_) => 8,
    }
}

/// Converts a keymap payload rejection into a stable diagnostics reason.
fn keymap_payload_error_code(error: &KeymapPayloadError) -> u8 {
    match error {
        KeymapPayloadError::Length(_) => 1,
        KeymapPayloadError::Version(_) => 2,
        KeymapPayloadError::RuleCount(_) => 3,
        KeymapPayloadError::Reserved(_) => 4,
        KeymapPayloadError::UnknownFlags(_) => 5,
        KeymapPayloadError::DuplicateRule { .. } => 6,
        KeymapPayloadError::Keymap(_) => 7,
    }
}

/// Converts a persisted keymap record rejection into a stable diagnostics reason.
fn keymap_record_error_code(error: &KeymapRecordError) -> u8 {
    match error {
        KeymapRecordError::Length(_) => 1,
        KeymapRecordError::Empty => 2,
        KeymapRecordError::Version(_) => 3,
        KeymapRecordError::Reserved => 4,
        KeymapRecordError::Crc { .. } => 5,
        KeymapRecordError::PayloadLength(_) => 6,
        KeymapRecordError::Payload(_) => 7,
    }
}

/// Queues a validated profile for the task that owns the MPSL Flash object.
fn queue_profile_write(profile: StoredProfile) -> OutResponse {
    match RADIO_CONTROL_REQUESTS.try_send(RadioControl::Profile(profile)) {
        Ok(()) => OutResponse::Accepted,
        Err(_) => {
            // A full queue means an earlier write is still in flight. Refuse
            // this request explicitly rather than silently losing a profile.
            PROFILE_FLASH_ERROR.store(0x50 | 1, Ordering::Relaxed);
            PROFILE_WRITE_FAILED.store(true, Ordering::Relaxed);
            LAST_ERROR.store(PHASE_PROFILE_FLASH | 5, Ordering::Relaxed);
            OutResponse::Rejected
        }
    }
}

/// Converts a pairing into the form written to flash.
/// Numbers the reasons this bridge declines to subscribe to a keyboard.
fn hogp_refusal_code(
    error: &ukf_nrf52840_ble_usb::hogp::HogpError<core::convert::Infallible>,
) -> u8 {
    use ukf_nrf52840_ble_usb::hogp::HogpError;
    match error {
        HogpError::InvalidState => 1,
        HogpError::NotKeyboard => 2,
        HogpError::MissingInputReport => 3,
        _ => 0x0f,
    }
}

/// Numbers the reasons a notification is not forwarded.
fn notify_refusal_code(
    error: &ukf_nrf52840_ble_usb::hogp::HogpError<core::convert::Infallible>,
) -> u8 {
    use ukf_nrf52840_ble_usb::hogp::HogpError;
    match error {
        HogpError::InvalidState => 1,
        HogpError::UnexpectedHandle(_) => 2,
        HogpError::InvalidReportLength(_) => 3,
        HogpError::Pipeline(_) => 4,
        _ => 0x0f,
    }
}

/// Numbers the reasons an injected report is not forwarded.
///
/// A refused injection used to be invisible: the host tool reports that it sent
/// the packet, and the firmware discarded whatever came back. The tool cannot
/// tell the difference, so the firmware has to say.
fn pipeline_refusal_code(
    error: &ukf_nrf52840_ble_usb::PipelineError<core::convert::Infallible>,
) -> u8 {
    use ukf_core::BridgeError;
    use ukf_nrf52840_ble_usb::PipelineError;
    match error {
        PipelineError::InvalidReportLength(_) => 1,
        PipelineError::Bridge(BridgeError::DetachedSource) => 2,
        PipelineError::Bridge(BridgeError::UnknownSource) => 3,
        PipelineError::Bridge(BridgeError::UnregisteredSource) => 5,
        PipelineError::Usb(_) => 4,
    }
}

/// Numbers the ways the flash driver can refuse.
///
/// An MPSL refusal carries an errno, and collapsing it to "the scheduler said
/// no" hid whether the request was malformed, whether no session was
/// available, or whether the radio simply had no room right then — only the
/// last of which is worth retrying.
fn flash_error_code(error: &nrf_sdc::mpsl::FlashError, errno: &AtomicU16) -> u8 {
    use nrf_sdc::mpsl::FlashError;
    match error {
        FlashError::OutOfBounds => 1,
        FlashError::Unaligned => 2,
        FlashError::Mpsl(mpsl) => {
            // The errno goes in its own field. Packing it beside the operation
            // in one byte made the two overlap: `EBUSY` is 16, the same bit the
            // operation nibble used, so a busy scheduler read as an erase.
            let raw: i32 = mpsl.to_retval().into();
            errno.store(
                raw.unsigned_abs().min(u32::from(u16::MAX)) as u16,
                Ordering::Relaxed,
            );
            3
        }
    }
}

/// Returns the profile currently selected by the report path.
fn active_profile() -> BridgeProfile {
    let flags = ACTIVE_PROFILE_FLAGS.load(Ordering::Acquire);
    StoredProfile {
        us_to_jis: flags & (1 << 0) != 0,
        caps_to_ctrl: flags & (1 << 1) != 0,
        swap_alt_gui: flags & (1 << 2) != 0,
    }
    .into()
}

fn profile_from_flags(flags: u8) -> BridgeProfile {
    StoredProfile {
        us_to_jis: flags & (1 << 0) != 0,
        caps_to_ctrl: flags & (1 << 1) != 0,
        swap_alt_gui: flags & (1 << 2) != 0,
    }
    .into()
}

fn source_profile_flags(profile: BridgeProfile) -> u8 {
    u8::from(profile.us_to_jis)
        | (u8::from(profile.caps_to_ctrl) << 1)
        | (u8::from(profile.swap_alt_gui) << 2)
}

fn source_profile(slot: usize) -> BridgeProfile {
    profile_from_flags(SOURCE_PROFILE_FLAGS[slot].load(Ordering::Acquire))
}

fn first_free_bond_slot() -> Option<u8> {
    BOND_SLOTS.lock(|cell| {
        cell.borrow()
            .iter()
            .position(Option::is_none)
            .map(|index| index as u8)
    })
}

fn source_is_active(slot: u8) -> bool {
    usize::from(slot) < BOND_SLOT_COUNT
        && ACTIVE_BLE_SLOTS.load(Ordering::Acquire) & (1u8 << slot) != 0
}

fn set_source_active(slot: u8, active: bool) {
    if usize::from(slot) >= BOND_SLOT_COUNT {
        return;
    }
    let mask = 1u8 << slot;
    if active {
        ACTIVE_BLE_SLOTS.fetch_or(mask, Ordering::AcqRel);
    } else {
        ACTIVE_BLE_SLOTS.fetch_and(!mask, Ordering::AcqRel);
    }
}

fn bonded_slot_for_address(addr_kind: u8, addr: [u8; 6]) -> Option<u8> {
    let advertised = Address::new(AddrKind::new(addr_kind), BdAddr::new(addr));
    BOND_SLOTS.lock(|cell| {
        cell.borrow().iter().enumerate().find_map(|(index, slot)| {
            let slot = slot.as_ref()?;
            if !bond_slot_was_loaded(index) {
                return None;
            }
            // The controller may report a resolved identity as HCI address
            // kind 0x02/0x03 while the bond stores the corresponding 0x00/0x01
            // public/random identity kind. It may also leave the RPA in the
            // advertisement. Use trouble-host's identity matcher for both
            // cases; raw address equality would make a valid multi-bond peer
            // look unregistered whenever its private address rotates.
            let identity = Identity {
                addr: Address::new(
                    AddrKind::new(slot.bond.addr_kind),
                    BdAddr::new(slot.bond.addr),
                ),
                irk: slot.bond.irk.and_then(IdentityResolvingKey::new),
            };
            identity.match_address(&advertised).then_some(index as u8)
        })
    })
}

fn registered_bond(slot: usize) -> Option<StoredBondSlot> {
    BOND_SLOTS.lock(|cell| cell.borrow()[slot])
}

fn pipeline_registrations() -> [Option<BleSourceRegistration>; BOND_SLOT_COUNT] {
    core::array::from_fn(|index| {
        registered_bond(index).map(|slot| BleSourceRegistration {
            identity: SourceIdentity::from_address(slot.bond.addr, slot.bond.irk.is_some()),
            irk: slot.bond.irk.map(|key| key.to_le_bytes()),
            name: SourceName::from_raw(slot.name, slot.name_len),
            profile: source_profile(index),
        })
    })
}

fn source_snapshot(slot: u8) -> [u8; diagnostics_report::DIAGNOSTICS_REPORT_LEN] {
    let index = usize::from(slot);
    let (transport, identity, name) = match index {
        0..=3 => match registered_bond(index) {
            Some(slot) => (
                Some(InputTransport::Ble),
                SourceIdentity {
                    address: Some(slot.bond.addr),
                    irk_present: slot.bond.irk.is_some(),
                },
                SourceName::from_raw(slot.name, slot.name_len),
            ),
            None => (None, SourceIdentity::NONE, SourceName::EMPTY),
        },
        4 => (
            Some(InputTransport::Virtual),
            SourceIdentity::NONE,
            VIRTUAL_SOURCE_NAME,
        ),
        _ => (None, SourceIdentity::NONE, SourceName::EMPTY),
    };
    let state = SourceState::try_from(SOURCE_STATES[index].load(Ordering::Acquire))
        .unwrap_or(SourceState::Failed);
    diagnostics_report::encode_source_block(&SourceReport {
        slot,
        transport,
        state,
        identity,
        profile: source_profile(index),
        name,
    })
}

/// Reads all bond slots before scanning starts. The 0.5 record, if present,
/// is returned as a migration request and is not erased here.
fn load_bond_slots(
    flash: &mut nrf_sdc::mpsl::Flash<'_>,
) -> ([Option<StoredBondSlot>; BOND_SLOT_COUNT], bool) {
    BOND_STACK_LOADED_SLOTS.store(0, Ordering::Release);
    let mut page = [0; BOND_PAGE_DATA_LEN];
    let (slots, migrate) = match flash.read(BOND_STORAGE_OFFSET, &mut page) {
        Ok(()) => decode_bond_page(&page),
        Err(error) => {
            let code = flash_error_code(&error, &BOND_FLASH_ERRNO);
            BOND_FLASH_ERROR.store(0x30 | code, Ordering::Relaxed);
            LAST_ERROR.store(PHASE_PROFILE_FLASH | code, Ordering::Relaxed);
            ([None; BOND_SLOT_COUNT], false)
        }
    };
    BOND_SLOTS.lock(|cell| *cell.borrow_mut() = slots);
    for (index, slot) in slots.iter().enumerate() {
        set_source_state(
            index,
            if slot.is_some() {
                SourceState::Disconnected
            } else {
                SourceState::Unregistered
            },
        );
        SOURCE_PROFILE_FLAGS[index].store(
            if slot.is_some() {
                source_profile_flags(active_profile())
            } else {
                0
            },
            Ordering::Release,
        );
    }
    let loaded = slots.iter().any(Option::is_some);
    BOND_LOADED.store(loaded, Ordering::Release);
    if let Some(slot) = slots[0] {
        remember_bonded_peer(&slot.bond.addr);
        HAS_BONDED_IRK.store(slot.bond.irk.is_some(), Ordering::Release);
    }
    if loaded {
        PAIRING_MODE.store(false, Ordering::Release);
    }
    (slots, migrate)
}

fn mark_bond_slot_dirty(slot: usize) {
    if slot < BOND_SLOT_COUNT {
        BOND_DIRTY_SLOTS.fetch_or(1u8 << slot, Ordering::AcqRel);
        BOND_PAGE_DIRTY.store(true, Ordering::Release);
    }
}

fn bond_slot_was_loaded(slot: usize) -> bool {
    slot < BOND_SLOT_COUNT && BOND_STACK_LOADED_SLOTS.load(Ordering::Acquire) & (1u8 << slot) != 0
}

/// Appends changed logical slots only after the radio has reached a quiet
/// boundary. No caller may invoke this while a link is carrying traffic.
///
/// The bond page is a small append-only journal. A failed or power-interrupted
/// record leaves earlier valid records intact; the decoder replays the page and
/// takes the last valid record for each logical slot. Once all 32 records are
/// occupied, this function fails closed instead of erasing the only copy. A
/// future compaction scheme can be added with an explicit migration format.
async fn persist_bond_page(
    flash: &mut nrf_sdc::mpsl::Flash<'_>,
    slots: &[Option<StoredBondSlot>; BOND_SLOT_COUNT],
    dirty_slots: u8,
) -> bool {
    if dirty_slots == 0 {
        BOND_PAGE_DIRTY.store(false, Ordering::Release);
        return true;
    }

    let mut page = [0; BOND_PAGE_DATA_LEN];
    if let Err(error) = flash.read(BOND_STORAGE_OFFSET, &mut page) {
        BOND_FLASH_ERROR.store(
            0x30 | flash_error_code(&error, &BOND_FLASH_ERRNO),
            Ordering::Relaxed,
        );
        BOND_WRITE_FAILED.store(true, Ordering::Relaxed);
        return false;
    }

    let Some(mut record_index) = page
        .chunks_exact(BOND_SLOT_RECORD_LEN)
        .position(|record| record.iter().all(|byte| *byte == 0xff))
    else {
        // Never erase here. The page may still contain the only recoverable
        // copy of one or more bonds.
        BOND_FLASH_ERROR.store(0x40, Ordering::Relaxed);
        BOND_WRITE_FAILED.store(true, Ordering::Relaxed);
        return false;
    };

    #[repr(align(4))]
    struct Aligned([u8; BOND_SLOT_RECORD_LEN]);

    for (slot, stored_slot) in slots.iter().enumerate() {
        let bit = 1u8 << slot;
        if dirty_slots & bit == 0 {
            continue;
        }
        if record_index >= BOND_PAGE_DATA_LEN / BOND_SLOT_RECORD_LEN {
            BOND_FLASH_ERROR.store(0x40, Ordering::Relaxed);
            BOND_WRITE_FAILED.store(true, Ordering::Relaxed);
            return false;
        }
        let record = match *stored_slot {
            Some(slot) => encode_bond_slot(&slot),
            None => encode_bond_tombstone(slot as u8),
        };
        let record = Aligned(record);
        let address = BOND_STORAGE_OFFSET + (record_index * BOND_SLOT_RECORD_LEN) as u32;
        match flash.write(address, &record.0).await {
            Ok(()) => {
                BOND_DIRTY_SLOTS.fetch_and(!bit, Ordering::AcqRel);
                BOND_SAVED.store(true, Ordering::Release);
                record_index += 1;
            }
            Err(error) => {
                BOND_FLASH_ERROR.store(
                    0x20 | flash_error_code(&error, &BOND_FLASH_ERRNO),
                    Ordering::Relaxed,
                );
                BOND_WRITE_FAILED.store(true, Ordering::Relaxed);
                return false;
            }
        }
    }

    if BOND_DIRTY_SLOTS.load(Ordering::Acquire) == 0 {
        BOND_FLASH_ERROR.store(0, Ordering::Relaxed);
        BOND_WRITE_FAILED.store(false, Ordering::Relaxed);
        BOND_PAGE_DIRTY.store(false, Ordering::Release);
        true
    } else {
        false
    }
}

fn snapshot_bond_slots() -> [Option<StoredBondSlot>; BOND_SLOT_COUNT] {
    BOND_SLOTS.lock(|cell| *cell.borrow())
}

/// Stages a bond-page rewrite without touching flash from a live connection.
async fn flush_bond_page_if_dirty(flash: &mut nrf_sdc::mpsl::Flash<'_>) {
    if !BOND_PAGE_DIRTY.load(Ordering::Acquire) {
        return;
    }
    let slots = snapshot_bond_slots();
    let dirty_slots = BOND_DIRTY_SLOTS.load(Ordering::Acquire);
    let _ = persist_bond_page(flash, &slots, dirty_slots).await;
}

fn set_source_state(slot: usize, state: SourceState) {
    SOURCE_STATES[slot].store(state as u8, Ordering::Release);
}

/// Publishes a profile after it is safe for both input sources to use it.
fn publish_profile(profile: StoredProfile, stored: bool) {
    let flags = source_profile_flags(profile.into());
    ACTIVE_PROFILE_FLAGS.store(flags, Ordering::Release);
    SOURCE_PROFILE_FLAGS[0].store(flags, Ordering::Release);
    SOURCE_PROFILE_FLAGS[4].store(flags, Ordering::Release);
    PROFILE_STORED.store(stored, Ordering::Release);
}

/// Publishes the keymap that new report pipelines should start with.
fn publish_keymap(keymap: Keymap, stored: bool) {
    ACTIVE_KEYMAP.lock(|cell| *cell.borrow_mut() = keymap);
    KEYMAP_STORED.store(stored, Ordering::Release);
}

/// Returns the keymap last loaded from or applied to the bridge.
fn active_keymap() -> Keymap {
    ACTIVE_KEYMAP.lock(|cell| *cell.borrow())
}

/// Reads the dedicated keymap page before the radio starts scanning.
fn load_keymap(flash: &mut nrf_sdc::mpsl::Flash<'_>) {
    let mut record = [0; KEYMAP_RECORD_LEN];
    match flash.read(KEYMAP_STORAGE_OFFSET, &mut record) {
        Ok(()) => match decode_keymap_record(&record) {
            Ok(keymap) => publish_keymap(keymap, true),
            Err(KeymapRecordError::Empty) => publish_keymap(Keymap::US_JIS, false),
            Err(error) => {
                let code = keymap_record_error_code(&error);
                KEYMAP_FLASH_ERROR.store(0x40 | code, Ordering::Relaxed);
                LAST_ERROR.store(PHASE_KEYMAP | code, Ordering::Relaxed);
                publish_keymap(Keymap::US_JIS, false);
            }
        },
        Err(error) => {
            let code = flash_error_code(&error, &KEYMAP_FLASH_ERRNO);
            KEYMAP_FLASH_ERROR.store(0x30 | code, Ordering::Relaxed);
            LAST_ERROR.store(PHASE_KEYMAP | code, Ordering::Relaxed);
            publish_keymap(Keymap::US_JIS, false);
        }
    }
}

/// Queues a validated keymap for the radio owner to apply and persist.
fn queue_keymap_write(keymap: Keymap) -> OutResponse {
    match RADIO_CONTROL_REQUESTS.try_send(RadioControl::Keymap(keymap)) {
        Ok(()) => OutResponse::Accepted,
        Err(_) => {
            KEYMAP_FLASH_ERROR.store(0x50 | 1, Ordering::Relaxed);
            LAST_ERROR.store(PHASE_KEYMAP | 8, Ordering::Relaxed);
            OutResponse::Rejected
        }
    }
}

/// Reads the profile page before the radio starts scanning.
fn load_profile(flash: &mut nrf_sdc::mpsl::Flash<'_>) {
    let mut record = [0; PROFILE_RECORD_LEN];
    match flash.read(PROFILE_STORAGE_OFFSET, &mut record) {
        Ok(()) => match decode_profile(&record) {
            Ok(profile) => publish_profile(profile, true),
            Err(_) => publish_profile(StoredProfile::from(BridgeProfile::US_JIS), false),
        },
        Err(error) => {
            let code = flash_error_code(&error, &PROFILE_FLASH_ERRNO);
            PROFILE_FLASH_ERROR.store(0x30 | code, Ordering::Relaxed);
            LAST_ERROR.store(PHASE_PROFILE_FLASH | code, Ordering::Relaxed);
            publish_profile(StoredProfile::from(BridgeProfile::US_JIS), false);
        }
    }
    PROFILE_READY.signal(());
}

/// Names why the host runner stopped, so a reset is not silent.
fn runner_stop_code<E>(result: &Result<(), BleHostError<E>>) -> u8 {
    match result {
        Ok(()) => 0x0a,
        Err(error) => ble_error_code(error),
    }
}

/// Tells the stack how the next pairing should authenticate.
///
/// `DisplayOnly` against a keyboard resolves to passkey entry: this side shows
/// a number and the keyboard types it. `NoInputNoOutput` resolves to Just
/// Works, which pairs with whoever answers.
fn apply_pairing_method<C: trouble_host::Controller, P: trouble_host::PacketPool>(
    stack: &trouble_host::Stack<'_, C, P>,
) {
    let capabilities = if REQUIRE_PASSKEY.load(Ordering::Acquire) {
        IoCapabilities::DisplayOnly
    } else {
        IoCapabilities::NoInputNoOutput
    };
    stack.set_io_capabilities(capabilities);
}

/// Tells the USB half the link is gone, waiting for room if it has to.
///
/// This used to be a `try_send` on a channel that holds two events. A full
/// channel dropped the notice, and the USB half then went on believing the
/// keyboard was still connected — holding down whatever key it last saw. A
/// stuck modifier is not a cosmetic loss: it broke Japanese input on the
/// machine's own keyboard, which this bridge is supposed to leave alone.
///
/// Waiting here is safe. The consumer only ever drains this channel.
async fn announce_disconnect(slot: u8) {
    RADIO_EVENTS.send(RadioEvent::Disconnected { slot }).await;
}

/// Packs what is known about the stored pairing into one byte.
fn bond_flags() -> u8 {
    use diagnostics_report::{BOND_FLAG_LOADED, BOND_FLAG_PEER_BONDED, BOND_FLAG_SAVED};
    let mut flags = 0;
    if BOND_LOADED.load(Ordering::Relaxed) {
        flags |= BOND_FLAG_LOADED;
    }
    if BOND_SAVED.load(Ordering::Relaxed) {
        flags |= BOND_FLAG_SAVED;
    }
    if PEER_WAS_BONDED.load(Ordering::Relaxed) {
        flags |= BOND_FLAG_PEER_BONDED;
    }
    if BOND_PAIRED_WITHOUT_KEYS.load(Ordering::Relaxed) {
        flags |= diagnostics_report::BOND_FLAG_PAIRED_WITHOUT_KEYS;
    }
    if BOND_WRITE_FAILED.load(Ordering::Relaxed) {
        flags |= diagnostics_report::BOND_FLAG_WRITE_FAILED;
    }
    flags
}

/// Writes one pairing to flash, replacing whatever was there.
///
/// Erase and write both need a radio timeslot, so this can only run while MPSL
/// is up — which it is, since the pairing that produced these keys just
/// finished. A failure here is not fatal: the link is already encrypted and
/// usable, and the only cost is having to pair again after the next reset.
/// The pairing waiting to be written, if one is.
///
/// Erasing a flash page asks the multiprotocol scheduler for a long timeslot,
/// and while a connection is up it does not get one — the erase came back
/// refused every time. Holding the keys until the link ends costs nothing: the
/// keyboard has to reconnect before they matter, and the radio is idle by then.
static PENDING_BOND: BlockingMutex<CriticalSectionRawMutex, RefCell<bool>> =
    BlockingMutex::new(RefCell::new(false));

/// A profile whose RAM mapping is live but whose flash record is not current.
///
/// The radio task owns the Flash object, but the profile request can arrive
/// while either scanning or connected. Keeping the committed profile here
/// lets the report task apply it immediately while the radio task waits for a
/// quiet point before touching MPSL flash.
static PENDING_PROFILE: BlockingMutex<CriticalSectionRawMutex, RefCell<PendingProfile>> =
    BlockingMutex::new(RefCell::new(None));
/// The keymap loaded at boot or most recently applied by the report task.
static ACTIVE_KEYMAP: BlockingMutex<CriticalSectionRawMutex, RefCell<Keymap>> =
    BlockingMutex::new(RefCell::new(Keymap::US_JIS));
/// A keymap applied in RAM but waiting for a radio-quiet flash boundary.
static PENDING_KEYMAP: BlockingMutex<CriticalSectionRawMutex, RefCell<Option<Keymap>>> =
    BlockingMutex::new(RefCell::new(None));
/// Latest configuration values waiting for the report task to apply.
///
/// This is deliberately separate from [`RADIO_EVENTS`]. Disconnects and HID
/// notifications are loss-intolerant events; a profile or keymap is a
/// latest-value update. Mixing those two contracts made a full event channel
/// reject a valid configuration commit.
static PENDING_CONFIGURATION: BlockingMutex<
    CriticalSectionRawMutex,
    RefCell<ConfigurationUpdates>,
> = BlockingMutex::new(RefCell::new(ConfigurationUpdates::new()));
/// Wakes the report task after a configuration value has been staged.
static CONFIG_APPLY_READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// One bond-list operation waiting for the link to become idle.
static PENDING_BOND_MANAGEMENT: BlockingMutex<
    CriticalSectionRawMutex,
    RefCell<Option<BondManagementRequest>>,
> = BlockingMutex::new(RefCell::new(None));

/// Stores a pairing in RAM and queues one page rewrite for a quiet boundary.
async fn store_bond(bond: &BondInformation) {
    let stored = to_stored_bond(bond);
    let existing_slot = bonded_slot_for_address(stored.addr_kind, stored.addr);
    let slot = existing_slot.or_else(first_free_bond_slot);
    let Some(slot) = slot else {
        BOND_WRITE_FAILED.store(true, Ordering::Release);
        return;
    };
    let record = if let Some(index) = existing_slot {
        if let Some(existing) = registered_bond(usize::from(index)) {
            StoredBondSlot::new(
                slot,
                stored,
                &existing.name[..usize::from(existing.name_len)],
            )
        } else {
            StoredBondSlot::new(slot, stored, b"BLE Keyboard")
        }
    } else {
        StoredBondSlot::new(slot, stored, b"BLE Keyboard")
    };
    let Ok(record) = record else {
        BOND_WRITE_FAILED.store(true, Ordering::Release);
        return;
    };
    BOND_SLOTS.lock(|cell| cell.borrow_mut()[usize::from(slot)] = Some(record));
    set_source_state(usize::from(slot), SourceState::Disconnected);
    SOURCE_PROFILE_FLAGS[usize::from(slot)]
        .store(source_profile_flags(active_profile()), Ordering::Release);
    PENDING_BOND.lock(|cell| *cell.borrow_mut() = true);
    mark_bond_slot_dirty(usize::from(slot));
    BOND_STACK_LOADED_SLOTS.fetch_or(1u8 << slot, Ordering::Release);
    remember_bonded_peer(&stored.addr);
    HAS_BONDED_IRK.store(stored.irk.is_some(), Ordering::Relaxed);
    BOND_LOADED.store(true, Ordering::Release);
    PAIRING_MODE.store(false, Ordering::Relaxed);
}

/// Writes the stashed pairing, if there is one, now that the link is down.
async fn flush_pending_bond(flash: &mut nrf_sdc::mpsl::Flash<'_>) {
    let pending = PENDING_BOND.lock(|cell| core::mem::replace(&mut *cell.borrow_mut(), false));
    if !pending && !BOND_PAGE_DIRTY.load(Ordering::Acquire) {
        return;
    }
    flush_bond_page_if_dirty(flash).await;
}

fn stage_bond_management(request: BondManagementRequest) -> bool {
    let accepted = PENDING_BOND_MANAGEMENT.lock(|cell| {
        let mut pending = cell.borrow_mut();
        if pending.is_some() {
            false
        } else {
            *pending = Some(request);
            true
        }
    });
    if !accepted {
        // A second request must not replace the first one. The USB transfer
        // has already been acknowledged at this point, so the stable
        // diagnostic is the only way to tell the host that this boundary was
        // busy and the request was not applied.
        LAST_ERROR.store(PHASE_BOND_MANAGEMENT | 2, Ordering::Relaxed);
    }
    accepted
}

#[derive(Clone, Copy)]
enum BondManagementOutcome {
    Applied,
    Retry,
    Rejected,
}

/// Applies one queued rename/delete after the active central link is gone.
fn apply_bond_management<C, P>(
    stack: &trouble_host::Stack<'_, C, P>,
    request: BondManagementRequest,
) -> BondManagementOutcome
where
    C: trouble_host::Controller,
    P: trouble_host::PacketPool,
{
    let slot = match request {
        BondManagementRequest::Rename { slot, .. } | BondManagementRequest::Delete { slot } => {
            usize::from(slot)
        }
    };
    let Some(current) = registered_bond(slot) else {
        return BondManagementOutcome::Rejected;
    };
    if source_is_active(slot as u8) {
        return BondManagementOutcome::Retry;
    }
    match request {
        BondManagementRequest::Rename { slot, name } => {
            let Ok(updated) = StoredBondSlot::new(slot, current.bond, name.as_bytes()) else {
                return BondManagementOutcome::Rejected;
            };
            BOND_SLOTS.lock(|cell| cell.borrow_mut()[usize::from(slot)] = Some(updated));
        }
        BondManagementRequest::Delete { .. } => {
            let identity = from_stored_bond(&current.bond).identity;
            if bond_slot_was_loaded(slot) {
                if stack.remove_bond_information(identity).is_err() {
                    return BondManagementOutcome::Rejected;
                }
                BOND_STACK_LOADED_SLOTS.fetch_and(!(1u8 << slot), Ordering::AcqRel);
            }
            BOND_SLOTS.lock(|cell| cell.borrow_mut()[slot] = None);
            set_source_state(slot, SourceState::Unregistered);
            SOURCE_PROFILE_FLAGS[slot].store(0, Ordering::Release);
            if !snapshot_bond_slots().iter().any(Option::is_some) {
                BOND_LOADED.store(false, Ordering::Release);
                PAIRING_MODE.store(true, Ordering::Release);
            }
        }
    }
    PENDING_BOND.lock(|cell| *cell.borrow_mut() = true);
    mark_bond_slot_dirty(slot);
    BondManagementOutcome::Applied
}

/// Applies a queued list operation, preserving it if the stack still reports
/// that the active link owns the requested slot.
async fn process_pending_bond_management<C, P>(
    stack: &trouble_host::Stack<'_, C, P>,
    flash: &mut nrf_sdc::mpsl::Flash<'_>,
) where
    C: trouble_host::Controller,
    P: trouble_host::PacketPool,
{
    let Some(request) = PENDING_BOND_MANAGEMENT.lock(|cell| *cell.borrow()) else {
        return;
    };
    match apply_bond_management(stack, request) {
        BondManagementOutcome::Applied => {
            PENDING_BOND_MANAGEMENT.lock(|cell| *cell.borrow_mut() = None);
            // The supervisor performs the write only after every central link
            // has gone quiet. Applying a rename/delete to an idle slot must
            // not make a flash write race a different live keyboard.
            if ACTIVE_BLE_SLOTS.load(Ordering::Acquire) == 0 {
                flush_pending_bond(flash).await;
            }
        }
        BondManagementOutcome::Retry => {}
        BondManagementOutcome::Rejected => {
            PENDING_BOND_MANAGEMENT.lock(|cell| *cell.borrow_mut() = None);
            LAST_ERROR.store(PHASE_BOND_MANAGEMENT | 1, Ordering::Relaxed);
        }
    }
}

/// Applies a committed profile to the live report path and holds its flash
/// record until the radio is quiet.
async fn stage_pending_profile(profile: StoredProfile) {
    PENDING_PROFILE.lock(|cell| {
        cell.borrow_mut().replace(profile);
    });
    PENDING_CONFIGURATION.lock(|cell| {
        cell.borrow_mut().stage_profile(profile);
    });
    PROFILE_STORED.store(false, Ordering::Release);
    CONFIG_APPLY_READY.signal(());
}

/// Applies a validated keymap in RAM and holds its flash record until quiet.
async fn stage_pending_keymap(keymap: Keymap) {
    PENDING_KEYMAP.lock(|cell| {
        cell.borrow_mut().replace(keymap);
    });
    PENDING_CONFIGURATION.lock(|cell| {
        cell.borrow_mut().stage_keymap(keymap);
    });
    KEYMAP_STORED.store(false, Ordering::Release);
    CONFIG_APPLY_READY.signal(());
}

/// Applies the latest configuration values without consuming radio events.
///
/// The values are coalesced before this function runs. That is safe because a
/// newer profile or keymap supersedes an older one, while source slots remain
/// independent. The flash-owned pending values are kept separately so this
/// task never takes ownership of a write that has not reached a quiet point.
fn apply_pending_configuration(pipeline: &mut ReportPipeline, pending: &mut Option<[u8; 8]>) {
    if let Some(profile) = PENDING_CONFIGURATION.lock(|cell| cell.borrow_mut().take_profile()) {
        let mut sink = PendingUsbReport(pending);
        match pipeline.set_profile(profile.into(), &mut sink) {
            Ok(()) => {
                PROFILE_APPLY_ERROR.store(0, Ordering::Relaxed);
                publish_profile(profile, PROFILE_STORED.load(Ordering::Acquire));
            }
            Err(error) => {
                let code = pipeline_refusal_code(&error);
                PROFILE_APPLY_ERROR.store(code, Ordering::Relaxed);
                LAST_ERROR.store(PHASE_PROFILE_APPLY | code, Ordering::Relaxed);
            }
        }
    }

    if let Some(keymap) = PENDING_CONFIGURATION.lock(|cell| cell.borrow_mut().take_keymap()) {
        let mut sink = PendingUsbReport(pending);
        match pipeline.set_keymap(keymap, &mut sink) {
            Ok(()) => {
                publish_keymap(keymap, false);
                KEYMAP_APPLY_ERROR.store(0, Ordering::Relaxed);
            }
            Err(error) => {
                let code = pipeline_refusal_code(&error);
                KEYMAP_APPLY_ERROR.store(code, Ordering::Relaxed);
                LAST_ERROR.store(PHASE_KEYMAP | code, Ordering::Relaxed);
            }
        }
    }

    for slot in 0..SOURCE_SLOT_COUNT {
        let slot = slot as u8;
        let Some(profile) =
            PENDING_CONFIGURATION.lock(|cell| cell.borrow_mut().take_source_profile(slot))
        else {
            continue;
        };
        let mut sink = PendingUsbReport(pending);
        match pipeline.set_source_profile(SourceId(slot), profile, &mut sink) {
            Ok(()) => {
                SOURCE_PROFILE_FLAGS[usize::from(slot)]
                    .store(source_profile_flags(profile), Ordering::Release);
            }
            Err(error) => {
                let code = pipeline_refusal_code(&error);
                PROFILE_APPLY_ERROR.store(code, Ordering::Relaxed);
                LAST_ERROR.store(PHASE_PROFILE_APPLY | code, Ordering::Relaxed);
            }
        }
    }
}

/// Queues one source-specific profile without consuming the radio event queue.
fn queue_source_profile_apply(slot: u8, profile: BridgeProfile) -> OutResponse {
    let accepted =
        PENDING_CONFIGURATION.lock(|cell| cell.borrow_mut().stage_source_profile(slot, profile));
    if accepted {
        CONFIG_APPLY_READY.signal(());
        OutResponse::Accepted
    } else {
        PROFILE_APPLY_ERROR.store(5, Ordering::Relaxed);
        LAST_ERROR.store(PHASE_PROFILE_APPLY | 5, Ordering::Relaxed);
        OutResponse::Rejected
    }
}

/// Writes the stashed profile, if there is one, at a radio-quiet boundary.
///
/// A failed write keeps the profile queued. The next scan boundary or link
/// teardown can retry it without losing the user's committed choice.
async fn flush_pending_profile(flash: &mut nrf_sdc::mpsl::Flash<'_>) {
    let Some(profile) = PENDING_PROFILE.lock(|cell| cell.borrow_mut().take()) else {
        return;
    };
    if persist_stored_profile(flash, &profile).await {
        PROFILE_STORED.store(true, Ordering::Release);
    } else {
        PENDING_PROFILE.lock(|cell| {
            *cell.borrow_mut() = Some(profile);
        });
    }
}

/// Writes the stashed keymap, if there is one, at a radio-quiet boundary.
async fn flush_pending_keymap(flash: &mut nrf_sdc::mpsl::Flash<'_>) {
    let Some(keymap) = PENDING_KEYMAP.lock(|cell| cell.borrow_mut().take()) else {
        return;
    };
    if persist_keymap(flash, &keymap).await {
        KEYMAP_STORED.store(true, Ordering::Release);
    } else {
        PENDING_KEYMAP.lock(|cell| {
            *cell.borrow_mut() = Some(keymap);
        });
    }
}

/// Writes one compatibility profile to its own page.
///
/// The profile page is deliberately not the bond page. Erase and write use the
/// same MPSL timeslot-aware path as bond storage. A failed operation leaves the
/// profile in `PENDING_PROFILE` while exposing the operation and errno through
/// diagnostics state.
async fn persist_stored_profile(
    flash: &mut nrf_sdc::mpsl::Flash<'_>,
    profile: &StoredProfile,
) -> bool {
    #[repr(align(4))]
    struct Aligned([u8; PROFILE_RECORD_LEN]);

    let record = Aligned(encode_profile(profile));
    for attempt in 0..BOND_WRITE_ATTEMPTS {
        if attempt > 0 {
            Timer::after(BOND_WRITE_RETRY_DELAY).await;
        }
        match flash
            .erase(
                PROFILE_STORAGE_OFFSET,
                PROFILE_STORAGE_OFFSET + FLASH_PAGE_LEN,
            )
            .await
        {
            Ok(()) => {}
            Err(error) => {
                PROFILE_FLASH_ERROR.store(
                    0x10 | flash_error_code(&error, &PROFILE_FLASH_ERRNO),
                    Ordering::Relaxed,
                );
                continue;
            }
        }
        match flash.write(PROFILE_STORAGE_OFFSET, &record.0).await {
            Ok(()) => {
                PROFILE_FLASH_ERROR.store(0, Ordering::Relaxed);
                PROFILE_WRITE_FAILED.store(false, Ordering::Relaxed);
                return true;
            }
            Err(error) => {
                PROFILE_FLASH_ERROR.store(
                    0x20 | flash_error_code(&error, &PROFILE_FLASH_ERRNO),
                    Ordering::Relaxed,
                );
            }
        }
    }

    PROFILE_WRITE_FAILED.store(true, Ordering::Relaxed);
    LAST_ERROR.store(
        PHASE_PROFILE_FLASH | (PROFILE_FLASH_ERROR.load(Ordering::Relaxed) & 0x0f),
        Ordering::Relaxed,
    );
    false
}

/// Writes one keymap record to its dedicated page through the MPSL flash path.
async fn persist_keymap(flash: &mut nrf_sdc::mpsl::Flash<'_>, keymap: &Keymap) -> bool {
    #[repr(align(4))]
    struct Aligned([u8; KEYMAP_RECORD_LEN]);

    let record = Aligned(encode_keymap_record(keymap));
    for attempt in 0..BOND_WRITE_ATTEMPTS {
        if attempt > 0 {
            Timer::after(BOND_WRITE_RETRY_DELAY).await;
        }
        match flash
            .erase(
                KEYMAP_STORAGE_OFFSET,
                KEYMAP_STORAGE_OFFSET + FLASH_PAGE_LEN,
            )
            .await
        {
            Ok(()) => {}
            Err(error) => {
                KEYMAP_FLASH_ERROR.store(
                    0x10 | flash_error_code(&error, &KEYMAP_FLASH_ERRNO),
                    Ordering::Relaxed,
                );
                continue;
            }
        }
        match flash.write(KEYMAP_STORAGE_OFFSET, &record.0).await {
            Ok(()) => {
                KEYMAP_FLASH_ERROR.store(0, Ordering::Relaxed);
                return true;
            }
            Err(error) => {
                KEYMAP_FLASH_ERROR.store(
                    0x20 | flash_error_code(&error, &KEYMAP_FLASH_ERRNO),
                    Ordering::Relaxed,
                );
            }
        }
    }

    LAST_ERROR.store(
        PHASE_KEYMAP | (KEYMAP_FLASH_ERROR.load(Ordering::Relaxed) & 0x0f),
        Ordering::Relaxed,
    );
    false
}

/// Transfers profile commits from the USB handler to the radio task.
///
/// This only stages and publishes the profile. Flash persistence is performed
/// separately by `flush_pending_profile` at a known quiet point.
async fn process_radio_control_requests() {
    while let Ok(request) = RADIO_CONTROL_REQUESTS.try_receive() {
        match request {
            RadioControl::Profile(profile) => stage_pending_profile(profile).await,
            RadioControl::Keymap(keymap) => stage_pending_keymap(keymap).await,
            RadioControl::Bond(request) => {
                let _ = stage_bond_management(request);
            }
        }
    }
}

/// Whether a profile has been applied in RAM but still awaits flash.
fn profile_pending() -> bool {
    PENDING_PROFILE.lock(|cell| cell.borrow().is_some())
}

/// Records which keyboard the bridge is loyal to.
fn remember_bonded_peer(addr: &[u8; 6]) {
    for (slot, byte) in BONDED_ADDRESS.iter().zip(addr) {
        slot.store(*byte, Ordering::Relaxed);
    }
    HAS_BONDED_PEER.store(true, Ordering::Relaxed);
}

/// Converts a pairing into the form written to flash.
fn to_stored_bond(bond: &BondInformation) -> StoredBond {
    StoredBond {
        ltk: bond.ltk.0,
        addr_kind: bond.identity.addr.kind.into_inner(),
        addr: bond.identity.addr.addr.into_inner(),
        irk: bond.identity.irk.map(|key| key.0.get()),
        is_bonded: bond.is_bonded,
        security_level: security_level_code(bond.security_level),
        ediv: bond.ediv,
        rand: bond.rand,
        encryption_key_len: bond.encryption_key_len,
    }
}

/// Rebuilds a pairing from a flash record.
fn from_stored_bond(stored: &StoredBond) -> BondInformation {
    BondInformation {
        ltk: LongTermKey::new(stored.ltk),
        identity: Identity {
            addr: Address {
                kind: AddrKind::new(stored.addr_kind),
                addr: BdAddr::new(stored.addr),
            },
            irk: stored.irk.and_then(IdentityResolvingKey::new),
        },
        is_bonded: stored.is_bonded,
        security_level: security_level_from_code(stored.security_level),
        ediv: stored.ediv,
        rand: stored.rand,
        encryption_key_len: stored.encryption_key_len,
    }
}

/// Numbers the security level for storage. The enum is not `repr(u8)`, and
/// pinning the mapping here keeps a reordering upstream from silently
/// reinterpreting every record already on the device.
fn security_level_code(level: SecurityLevel) -> u8 {
    match level {
        SecurityLevel::NoEncryption => 0,
        SecurityLevel::Encrypted => 1,
        SecurityLevel::EncryptedAuthenticated => 2,
    }
}

fn security_level_from_code(code: u8) -> SecurityLevel {
    match code {
        1 => SecurityLevel::Encrypted,
        2 => SecurityLevel::EncryptedAuthenticated,
        _ => SecurityLevel::NoEncryption,
    }
}

fn diagnostics_snapshot() -> [u8; diagnostics_report::DIAGNOSTICS_REPORT_LEN] {
    let (panic_line, panic_count) = last_panic();
    let mut address = [0; 6];
    for (byte, atomic) in address.iter_mut().zip(LAST_ADDRESS.iter()) {
        *byte = atomic.load(Ordering::Relaxed);
    }
    diagnostics_report::encode(&DiagnosticsReport {
        state: BridgeState::try_from(BRIDGE_STATE.load(Ordering::Acquire))
            .unwrap_or(BridgeState::Failed),
        last_error: LAST_ERROR.load(Ordering::Relaxed),
        advertisements_seen: ADVERTISEMENTS_SEEN.load(Ordering::Relaxed),
        hid_advertisements: HID_ADVERTISEMENTS.load(Ordering::Relaxed),
        last_address: address,
        last_rssi: LAST_RSSI.load(Ordering::Relaxed) as i8,
        connection_count: CONNECTION_COUNT.load(Ordering::Relaxed),
        input_reports_received: INPUT_REPORTS_RECEIVED.load(Ordering::Relaxed),
        reports_forwarded: REPORTS_FORWARDED.load(Ordering::Relaxed),
        panic_line,
        panic_count,
        discovery_step: DISCOVERY_STEP.load(Ordering::Relaxed),
    })
}

fn transfer_snapshot() -> [u8; diagnostics_report::DIAGNOSTICS_REPORT_LEN] {
    CONFIG_TRANSFER.lock(|cell| {
        let transfer = cell.borrow();
        diagnostics_report::encode_transfer(&TransferReport {
            state: transfer.state(),
            last_error: transfer.last_error(),
            target: transfer.target(),
            expected_len: transfer.expected_len(),
            received_len: transfer.received_len(),
            next_index: transfer.next_index(),
            declared_crc: transfer.declared_crc(),
            computed_crc: transfer.computed_crc(),
        })
    })
}

struct PendingUsbReport<'a>(&'a mut Option<[u8; 8]>);

impl UsbReportSink for PendingUsbReport<'_> {
    type Error = core::convert::Infallible;

    fn write(&mut self, report: [u8; 8]) -> Result<(), Self::Error> {
        *self.0 = Some(report);
        Ok(())
    }
}

/// Controller/host resources for the two simultaneous central links.
const CENTRAL_CONNECTIONS: usize = MAX_ACTIVE_BLE_LINKS;
const CENTRAL_CHANNELS: usize = 8;
const MAX_SERVICES: usize = 8;

/// Characteristics this bridge will read out of one HID service.
///
/// Eight was not enough and the overflow surfaced as `InsufficientSpace`, which
/// read as the board running out of memory rather than as a list that was too
/// short. A HOGP keyboard carries a protocol mode, a report map, HID
/// information, a control point, boot input and output reports, and one report
/// characteristic per report ID — past eight before any of the reports are
/// counted. The keyboard decides how many there are, so this is sized with room
/// to spare rather than to a measurement of one device.
const MAX_HID_CHARACTERISTICS: usize = 32;

/// Asks the keyboard whether it is still there, while a key is held.
///
/// Reads the same characteristic the bridge is subscribed to. The value is not
/// used; what is being tested is whether anything comes back at all. A refusal
/// counts as an answer — a peer that declines the read still had to receive it
/// — so the only verdict of death is silence.
///
/// Returns an error when the link is gone. The caller must then stop using this
/// client: a probe that timed out was abandoned mid-transaction, and the GATT
/// client holds exactly one response slot, so a late reply would be handed to
/// whatever request came next.
async fn probe_link<C: trouble_host::Controller>(
    client: &GattClient<'_, C, DefaultPacketPool, MAX_SERVICES>,
    characteristic: &Characteristic<[u8]>,
    connection: &Connection<'_, DefaultPacketPool>,
    value: &mut [u8],
    slot: u8,
) -> Result<(), BleHostError<C::Error>> {
    match with_timeout(
        LIVENESS_PROBE_TIMEOUT,
        client.read_characteristic(characteristic, value),
    )
    .await
    {
        Ok(Ok(_)) => {
            // Deliberately not clearing the recorded reason. A probe that
            // failed and a link that recovered are both worth knowing about,
            // and the failure counter beside it says whether the reason is
            // current. Clearing here would erase the only account of a release
            // the moment the keyboard came back.
            RADIO_EVENTS.send(RadioEvent::LinkAlive { slot }).await;
            Ok(())
        }
        Ok(Err(error)) => {
            // Matched on the variant rather than on the reduced diagnostics
            // code, because the two questions are different: the code says what
            // to show a person, this says whether the keys are still real.
            let link_is_gone = matches!(
                error,
                BleHostError::Controller(_)
                    | BleHostError::BleHost(
                        trouble_host::Error::Disconnected | trouble_host::Error::Timeout
                    )
            );
            LIVENESS_PROBE_ERROR
                .store(PHASE_SUBSCRIBED | ble_error_code(&error), Ordering::Relaxed);
            if link_is_gone {
                count_one(&LIVENESS_PROBE_FAILURES);
                return Err(error);
            }
            RADIO_EVENTS.send(RadioEvent::LinkAlive { slot }).await;
            Ok(())
        }
        Err(TimeoutError) => {
            count_one(&LIVENESS_PROBE_FAILURES);
            LIVENESS_PROBE_ERROR.store(PHASE_SUBSCRIBED | REASON_TIMEOUT, Ordering::Relaxed);
            connection.disconnect();
            Err(BleHostError::BleHost(trouble_host::Error::Timeout))
        }
    }
}

/// Runs one assigned link from connection through notifications.
///
/// The supervisor never owns a `Connection` or `GattClient`: keeping those
/// values in this future is what lets the other worker continue receiving
/// reports while this worker scans, connects, or discovers its own peer.
async fn run_link_assignment<'host, 'stack, C>(
    stack: &'host trouble_host::Stack<'stack, C, DefaultPacketPool>,
    worker: LinkWorkerId,
    assignment: LinkAssignment,
) -> bool
where
    C: trouble_host::Controller
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeClearFilterAcceptList>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeAddDeviceToFilterAcceptList>
        + bt_hci::controller::ControllerCmdAsync<bt_hci::cmd::le::LeCreateConn>,
{
    let slot = assignment.slot;
    let slot_index = usize::from(slot);
    let mut policy = CentralPolicy::new();
    let mut hogp = HogpCentral::new();
    if hogp.start_scan().is_err() {
        retry(&mut policy, PHASE_SCAN | REASON_HOST, slot).await;
        return false;
    }
    policy.apply(CentralEvent::HidAdvertisement {
        address: assignment.address.addr.into_inner(),
        rssi: assignment.rssi,
    });
    BRIDGE_STATE.store(BridgeState::Connecting as u8, Ordering::Release);
    set_source_state(slot_index, SourceState::Connecting);

    let addresses = [assignment.address];
    let connect_config = ConnectConfig {
        scan_config: ScanConfig {
            filter_accept_list: &addresses,
            active: false,
            interval: Duration::from_millis(100),
            window: Duration::from_millis(30),
            ..Default::default()
        },
        connect_params: RequestedConnParams {
            min_connection_interval: Duration::from_millis(15),
            max_connection_interval: Duration::from_millis(30),
            max_latency: 0,
            supervision_timeout: Duration::from_secs(16),
            ..Default::default()
        },
    };
    LAST_PHASE.store(PHASE_CONNECT, Ordering::Relaxed);
    let mut central = stack.central();
    let attempt = with_timeout(CONNECT_TIMEOUT, central.connect(&connect_config)).await;
    LINK_CONNECT_BOUNDARIES.send((worker, slot)).await;
    LAST_PHASE.store(PHASE_CONNECT | 0x0f, Ordering::Relaxed);
    let connection = match attempt {
        Err(TimeoutError) => {
            retry(&mut policy, PHASE_CONNECT | REASON_TIMEOUT, slot).await;
            return false;
        }
        Ok(Ok(connection)) => connection,
        Ok(Err(error)) => {
            retry(&mut policy, PHASE_CONNECT | ble_error_code(&error), slot).await;
            return false;
        }
    };
    CONNECTION_COUNT.fetch_add(1, Ordering::Relaxed);
    if hogp.connected().is_err() {
        retry(&mut policy, PHASE_CONNECT | REASON_HOST, slot).await;
        return false;
    }
    policy.apply(CentralEvent::Connected);
    BRIDGE_STATE.store(BridgeState::Securing as u8, Ordering::Release);
    set_source_state(slot_index, SourceState::Securing);
    LAST_PHASE.store(PHASE_SECURE, Ordering::Relaxed);

    let bonded = connection.is_bonded_peer();
    PEER_WAS_BONDED.store(bonded, Ordering::Relaxed);
    if let Err(error) = connection.set_bondable(true) {
        PAIRING_FAILURE.store(host_error_code(&error), Ordering::Relaxed);
    }
    if REQUIRE_SECURITY
        && !bonded
        && let Err(error) = connection.request_security()
    {
        retry(&mut policy, PHASE_SECURE | host_error_code(&error), slot).await;
        connection.disconnect();
        return false;
    }
    LAST_PHASE.store(PHASE_SECURE | 0x0f, Ordering::Relaxed);
    let secured = if !REQUIRE_SECURITY || bonded {
        true
    } else {
        let mut encrypted = false;
        loop {
            let event = if encrypted {
                match with_timeout(PAIRING_KEYS_GRACE, connection.next()).await {
                    Ok(event) => event,
                    Err(TimeoutError) => break true,
                }
            } else {
                connection.next().await
            };
            match event {
                ConnectionEvent::Encrypted { bond, .. } => {
                    if let Some(bond) = bond {
                        store_bond(&bond).await;
                    }
                    encrypted = true;
                }
                ConnectionEvent::PairingComplete { bond, .. } => {
                    match bond {
                        Some(bond) => store_bond(&bond).await,
                        None => BOND_PAIRED_WITHOUT_KEYS.store(true, Ordering::Relaxed),
                    }
                    if encrypted {
                        break true;
                    }
                }
                ConnectionEvent::Disconnected { reason } => {
                    DISCONNECT_REASON.store(reason.into_inner(), Ordering::Relaxed);
                    break false;
                }
                ConnectionEvent::PairingFailed(error) => {
                    PAIRING_FAILURE.store(host_error_code(&error), Ordering::Relaxed);
                }
                ConnectionEvent::PassKeyDisplay(passkey) => {
                    PASSKEY.store(passkey.value(), Ordering::Release);
                    PASSKEY_SERIAL.fetch_add(1, Ordering::Release);
                }
                ConnectionEvent::PassKeyConfirm(_) => {
                    let _ = connection.pass_key_confirm();
                }
                ConnectionEvent::PassKeyInput => {
                    PASSKEY.store(PAIRING_PASSKEY, Ordering::Release);
                    PASSKEY_SERIAL.fetch_add(1, Ordering::Release);
                    if let Err(error) = connection.pass_key_input(PAIRING_PASSKEY) {
                        PAIRING_FAILURE.store(host_error_code(&error), Ordering::Relaxed);
                    }
                }
                ConnectionEvent::OobRequest => {}
                _ => {}
            }
        }
    };
    if !secured {
        retry(&mut policy, PHASE_SECURE | REASON_REFUSED, slot).await;
        return false;
    }
    policy.apply(CentralEvent::Secured);
    clear_backoff();
    LAST_PHASE.store(PHASE_DISCOVER, Ordering::Relaxed);
    BRIDGE_STATE.store(BridgeState::Discovering as u8, Ordering::Release);
    set_source_state(slot_index, SourceState::Discovering);

    let client =
        match GattClient::<_, DefaultPacketPool, MAX_SERVICES>::new(stack, &connection).await {
            Ok(client) => client,
            Err(error) => {
                retry(&mut policy, PHASE_DISCOVER | ble_error_code(&error), slot).await;
                connection.disconnect();
                return false;
            }
        };
    let gatt_task = client.task();
    let gatt_work = async {
        let hid_uuid = Uuid::new_short(ukf_nrf52840_ble_usb::hogp::HID_SERVICE_UUID);
        let report_uuid = Uuid::new_short(ukf_nrf52840_ble_usb::hogp::REPORT_CHARACTERISTIC_UUID);
        let report_map_uuid =
            Uuid::new_short(ukf_nrf52840_ble_usb::hogp::REPORT_MAP_CHARACTERISTIC_UUID);
        let reference_uuid =
            Uuid::new_short(ukf_nrf52840_ble_usb::hogp::REPORT_REFERENCE_DESCRIPTOR_UUID);
        DISCOVERY_STEP.store(1, Ordering::Relaxed);
        let services = client.services_by_uuid(&hid_uuid).await?;
        DISCOVERY_STEP.store(2, Ordering::Relaxed);
        let service = services
            .first()
            .ok_or(trouble_host::BleHostError::<C::Error>::BleHost(
                trouble_host::Error::NotFound,
            ))?;
        DISCOVERY_STEP.store(3, Ordering::Relaxed);
        let report_map_char = client
            .characteristic_by_uuid::<[u8]>(service, &report_map_uuid)
            .await?;
        let mut report_map = [0; 512];
        DISCOVERY_STEP.store(4, Ordering::Relaxed);
        let report_map_len = client
            .read_characteristic(&report_map_char, &mut report_map)
            .await?;
        let mut reports = [ReportCharacteristic {
            value_handle: 0,
            notify: false,
            report_id: 0,
            report_type: 0,
        }; 8];
        DISCOVERY_STEP.store(5, Ordering::Relaxed);
        let characteristics = client
            .characteristics::<MAX_HID_CHARACTERISTICS>(service)
            .await?;
        let mut report_count = 0;
        for report_char in characteristics
            .iter()
            .filter(|characteristic| characteristic.uuid == report_uuid)
        {
            if report_count == reports.len() {
                break;
            }
            DISCOVERY_STEP.store(6, Ordering::Relaxed);
            let reference = client
                .descriptor_by_uuid::<[u8], [u8]>(report_char, &reference_uuid)
                .await?;
            let mut reference_bytes = [0; 2];
            if client
                .read_descriptor(&reference, &mut reference_bytes)
                .await?
                != 2
            {
                continue;
            }
            reports[report_count] = ReportCharacteristic {
                value_handle: report_char.handle,
                notify: report_char.cccd_handle.is_some(),
                report_id: reference_bytes[0],
                report_type: reference_bytes[1],
            };
            report_count += 1;
        }
        DISCOVERY_STEP.store(7, Ordering::Relaxed);
        let subscription = hogp
            .finish_discovery(&report_map[..report_map_len], &reports[..report_count])
            .map_err(|error| {
                HOGP_REFUSAL.store(hogp_refusal_code(&error), Ordering::Relaxed);
                trouble_host::Error::NotFound
            })?;
        DISCOVERY_STEP.store(8, Ordering::Relaxed);
        let report_char = characteristics
            .iter()
            .find(|characteristic| characteristic.handle == subscription.value_handle)
            .ok_or(trouble_host::Error::NotFound)?;
        DISCOVERY_STEP.store(9, Ordering::Relaxed);
        let mut listener = client.subscribe(report_char, false).await?;
        REPORT_MAPS.lock(|cell| cell.borrow_mut()[slot_index].copy_from_slice(&report_map));
        set_source_state(slot_index, SourceState::Connected);
        RADIO_EVENTS.send(RadioEvent::Connected { slot }).await;
        RADIO_EVENTS
            .send(RadioEvent::Discovered {
                slot,
                report_map_len: report_map_len as u16,
                reports,
                report_count: report_count as u8,
            })
            .await;
        let mut probe_value = [0; 8];
        loop {
            let notification = match select3(
                listener.next(),
                Timer::after(LIVENESS_PROBE_IDLE),
                RADIO_CONTROL_REQUESTS.receive(),
            )
            .await
            {
                Either3::First(notification) => notification,
                Either3::Second(()) => {
                    if !KEYS_HELD.load(Ordering::Relaxed) {
                        continue;
                    }
                    probe_link(&client, report_char, &connection, &mut probe_value, slot).await?;
                    continue;
                }
                Either3::Third(control) => {
                    match control {
                        RadioControl::Profile(profile) => stage_pending_profile(profile).await,
                        RadioControl::Keymap(keymap) => stage_pending_keymap(keymap).await,
                        RadioControl::Bond(request) => {
                            let _ = stage_bond_management(request);
                        }
                    }
                    continue;
                }
            };
            if notification.as_ref().len() != 8 {
                continue;
            }
            let mut payload = [0; 8];
            payload.copy_from_slice(notification.as_ref());
            INPUT_REPORTS_RECEIVED.fetch_add(1, Ordering::Relaxed);
            RADIO_EVENTS
                .send(RadioEvent::Notification {
                    slot,
                    handle: notification.handle(),
                    payload,
                })
                .await;
        }
        #[allow(unreachable_code)]
        Ok::<(), BleHostError<C::Error>>(())
    };
    match select(gatt_task, gatt_work).await {
        Either::First(_) => retry(&mut policy, PHASE_DISCOVER | 0x09, slot).await,
        Either::Second(Ok(())) => {}
        Either::Second(Err(error)) => {
            retry(&mut policy, PHASE_DISCOVER | ble_error_code(&error), slot).await
        }
    }
    true
}

/// Keeps one worker's connection and GATT state alive while the supervisor
/// scans and assigns the other worker.
async fn link_worker<'host, 'stack, C>(
    stack: &'host trouble_host::Stack<'stack, C, DefaultPacketPool>,
    worker: LinkWorkerId,
) -> !
where
    C: trouble_host::Controller
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeClearFilterAcceptList>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeAddDeviceToFilterAcceptList>
        + bt_hci::controller::ControllerCmdAsync<bt_hci::cmd::le::LeCreateConn>,
{
    loop {
        let assignment = LINK_ASSIGNMENTS[worker.0].receive().await;
        let slot = assignment.slot;
        let _ = run_link_assignment(stack, worker, assignment).await;
        set_source_active(slot, false);
        announce_disconnect(slot).await;
        LINK_RELEASES.send((worker, slot)).await;
    }
}

fn reclaim_link(links: &mut MultiLinkSet, worker: LinkWorkerId, slot: u8) {
    if links.source_for(worker) == Some(SourceId(slot)) {
        let _ = links.release(worker);
    }
}

/// Waits until the worker has finished the controller command that establishes
/// its link. Scanning may resume after that boundary, while security and GATT
/// discovery continue on the worker's independent connection.
async fn wait_for_link_connect_boundary(worker: LinkWorkerId, slot: u8) {
    loop {
        match select(
            LINK_CONNECT_BOUNDARIES.receive(),
            RADIO_CONTROL_REQUESTS.receive(),
        )
        .await
        {
            Either::First(boundary) if boundary == (worker, slot) => return,
            Either::First(_) => {}
            Either::Second(control) => match control {
                RadioControl::Profile(profile) => stage_pending_profile(profile).await,
                RadioControl::Keymap(keymap) => stage_pending_keymap(keymap).await,
                RadioControl::Bond(request) => {
                    let _ = stage_bond_management(request);
                }
            },
        }
    }
}

/// Scans and assigns peers while the link workers independently service GATT.
async fn supervise_links<'host, 'stack, C>(
    stack: &'host trouble_host::Stack<'stack, C, DefaultPacketPool>,
    mut flash: nrf_sdc::mpsl::Flash<'_>,
) -> !
where
    C: trouble_host::Controller
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeSetScanParams>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeSetScanEnable>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeClearFilterAcceptList>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeAddDeviceToFilterAcceptList>
        + bt_hci::controller::ControllerCmdAsync<bt_hci::cmd::le::LeCreateConn>,
{
    let mut links = MultiLinkSet::new();
    let scan_config = ScanConfig {
        active: true,
        interval: Duration::from_millis(60),
        window: Duration::from_millis(60),
        ..Default::default()
    };
    loop {
        while let Ok((worker, slot)) = LINK_RELEASES.try_receive() {
            reclaim_link(&mut links, worker, slot);
        }
        process_radio_control_requests().await;
        process_pending_bond_management(stack, &mut flash).await;
        if links.active_count() == 0 {
            flush_pending_bond(&mut flash).await;
            flush_pending_profile(&mut flash).await;
            flush_pending_keymap(&mut flash).await;
        }
        if links.active_count() == MAX_ACTIVE_BLE_LINKS {
            match select(LINK_RELEASES.receive(), RADIO_CONTROL_REQUESTS.receive()).await {
                Either::First((worker, slot)) => reclaim_link(&mut links, worker, slot),
                Either::Second(control) => match control {
                    RadioControl::Profile(profile) => stage_pending_profile(profile).await,
                    RadioControl::Keymap(keymap) => stage_pending_keymap(keymap).await,
                    RadioControl::Bond(request) => {
                        let _ = stage_bond_management(request);
                    }
                },
            }
            continue;
        }

        if PAIRING_METHOD_CHANGED.signaled() {
            PAIRING_METHOD_CHANGED.reset();
            apply_pairing_method(stack);
        }
        LAST_PHASE.store(PHASE_SCAN, Ordering::Relaxed);
        BRIDGE_STATE.store(BridgeState::Scanning as u8, Ordering::Release);
        let found = {
            let mut scanner = Scanner::new(stack.central());
            match scanner.scan(&scan_config).await {
                Ok(session) => {
                    match select3(
                        HID_CANDIDATES.wait(),
                        LINK_RELEASES.receive(),
                        RADIO_CONTROL_REQUESTS.receive(),
                    )
                    .await
                    {
                        Either3::First(found) => {
                            drop(session);
                            Some(found)
                        }
                        Either3::Second((worker, slot)) => {
                            drop(session);
                            reclaim_link(&mut links, worker, slot);
                            None
                        }
                        Either3::Third(control) => {
                            drop(session);
                            match control {
                                RadioControl::Profile(profile) => {
                                    stage_pending_profile(profile).await
                                }
                                RadioControl::Keymap(keymap) => stage_pending_keymap(keymap).await,
                                RadioControl::Bond(request) => {
                                    let _ = stage_bond_management(request);
                                }
                            }
                            None
                        }
                    }
                }
                Err(error) => {
                    let mut policy = CentralPolicy::new();
                    retry(&mut policy, PHASE_SCAN | ble_error_code(&error), u8::MAX).await;
                    None
                }
            }
        };
        Timer::after(SCAN_STOP_SETTLE).await;
        let Some((address, rssi, slot)) = found else {
            continue;
        };
        if source_is_active(slot) {
            continue;
        }
        let worker = match links.reserve(SourceId(slot)) {
            Ok(worker) => worker,
            Err(_) => continue,
        };
        set_source_active(slot, true);
        LINK_ASSIGNMENTS[worker.0]
            .send(LinkAssignment {
                address,
                rssi,
                slot,
            })
            .await;
        wait_for_link_connect_boundary(worker, slot).await;
    }
}

/// The controller commands scanning and connecting actually issue.
///
/// `trouble_host::Controller` alone is not enough: `Scanner::scan` and
/// `Central::connect` each carry their own `where` clause naming the HCI
/// commands they send, and a generic function has to repeat them or it will not
/// compile against any concrete controller.
async fn radio_task<C>(
    controller: C,
    resources: &mut HostResources<C, DefaultPacketPool, CENTRAL_CONNECTIONS, CENTRAL_CHANNELS>,
    mut flash: nrf_sdc::mpsl::Flash<'_>,
) -> !
where
    C: trouble_host::Controller
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeSetScanParams>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeSetScanEnable>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeClearFilterAcceptList>
        + bt_hci::controller::ControllerCmdSync<bt_hci::cmd::le::LeAddDeviceToFilterAcceptList>
        + bt_hci::controller::ControllerCmdAsync<bt_hci::cmd::le::LeCreateConn>,
{
    load_keymap(&mut flash);
    load_profile(&mut flash);
    process_radio_control_requests().await;
    flush_pending_profile(&mut flash).await;
    flush_pending_keymap(&mut flash).await;
    Timer::after(RADIO_START_DELAY).await;
    let stack = trouble_host::new(controller, resources)
        .set_random_address(Address::random(static_random_address()))
        .build();
    // No passkey. This keyboard accepts Just Works, which is what the Zephyr
    // bridge in this project's own history used to pair with it: that firmware
    // registers only a `cancel` authentication callback — leaving Zephyr's IO
    // capability at no-input-no-output — and asks for `BT_SECURITY_L2`,
    // encryption without authentication.
    //
    // This was the original setting here and it was abandoned for the wrong
    // reason. The failures it produced were at -89 dBm with a four second
    // supervision timeout, so the link died before pairing could finish, and
    // that was read as the keyboard refusing Just Works. It was not refusing
    // anything; it never got the chance to answer.
    apply_pairing_method(&stack);
    // Hand back every pairing from the previous boot before scanning starts,
    // so any registered keyboard is recognised on its first advertisement.
    let (bond_slots, migrate_legacy_bond) = load_bond_slots(&mut flash);
    for (index, slot) in bond_slots.iter().enumerate() {
        let Some(slot) = slot else { continue };
        if stack
            .add_bond_information(from_stored_bond(&slot.bond))
            .is_ok()
        {
            BOND_STACK_LOADED_SLOTS.fetch_or(1u8 << index, Ordering::Release);
        } else {
            // Keep the flash record visible for diagnostics and deletion, but
            // do not let scanning select a source the host stack did not
            // accept. The stable phase code is the evidence; it is not safe to
            // pretend this slot was restored and let a fresh pairing overwrite
            // another slot.
            set_source_state(index, SourceState::Failed);
            LAST_ERROR.store(PHASE_BOND_MANAGEMENT | 3, Ordering::Relaxed);
        }
    }
    if migrate_legacy_bond {
        BOND_DIRTY_SLOTS.store(1, Ordering::Release);
        BOND_PAGE_DIRTY.store(true, Ordering::Release);
        flush_bond_page_if_dirty(&mut flash).await;
    }
    BONDS_READY.signal(());
    let mut runner = stack.runner();
    let runner_task = runner.run_with_handler(&AdvertisementHandler);
    let work_task = join3(
        supervise_links(&stack, flash),
        link_worker(&stack, LinkWorkerId(0)),
        link_worker(&stack, LinkWorkerId(1)),
    );
    // Neither of these is meant to return. If one does, the board resets into
    // the bootloader and simply disappears from the host — so which one gave
    // up, and why, has to survive the reset. Discarding it here would repeat
    // the afternoon spent asking why the board kept vanishing.
    match embassy_futures::select::select(runner_task, work_task).await {
        Either::First(result) => {
            LAST_ERROR.store(PHASE_SCAN | runner_stop_code(&result), Ordering::Relaxed);
        }
        Either::Second(_) => {
            LAST_ERROR.store(PHASE_SCAN | REASON_HOST, Ordering::Relaxed);
        }
    }
    release_then_reset_into_uf2().await
}

/// Which phase a failure came from. The high nibble of `last_error`.
const PHASE_SCAN: u8 = 0x10;
/// Connection establishment failed.
const PHASE_CONNECT: u8 = 0x20;
/// Encryption or pairing failed.
const PHASE_SECURE: u8 = 0x30;
/// GATT discovery or subscription failed.
const PHASE_DISCOVER: u8 = 0x40;
/// The link was already subscribed and carrying keystrokes.
///
/// Its own phase rather than borrowing [`PHASE_DISCOVER`]: a liveness probe
/// that times out has nothing to do with discovery, and labelling it as such
/// would send the next reader looking at the wrong nine steps.
const PHASE_SUBSCRIBED: u8 = 0x50;
/// Profile flash persistence failed.
const PHASE_PROFILE_FLASH: u8 = 0x60;
/// The profile was stored but could not be applied to the live report path.
const PHASE_PROFILE_APPLY: u8 = 0x70;
/// A bond rename/delete request was rejected after transfer validation.
const PHASE_BOND_MANAGEMENT: u8 = 0x80;
/// A keymap transfer or live application was rejected.
const PHASE_KEYMAP: u8 = 0x90;

/// The peer closed or refused the link rather than returning a status.
const REASON_REFUSED: u8 = 0x0e;
/// The controller itself reported a transport-level failure.
const REASON_CONTROLLER: u8 = 0x0d;
/// A host-side error with no more specific mapping.
const REASON_HOST: u8 = 0x0f;
/// The attempt exceeded its own bound rather than returning a status.
const REASON_TIMEOUT: u8 = 0x08;

/// Reduces a host-side error to the low nibble of `last_error`.
///
/// `Connection::request_security` reports `trouble_host::Error` directly rather
/// than wrapping it, so it needs its own arm into the same code space.
fn host_error_code(error: &trouble_host::Error) -> u8 {
    match error {
        trouble_host::Error::Hci(status) => {
            let raw: u8 = (*status).into();
            if raw == 0 || raw > 0x0f {
                REASON_HOST
            } else {
                raw
            }
        }
        trouble_host::Error::Timeout => 0x08,
        trouble_host::Error::Disconnected => 0x09,
        // Discovery has several distinct ways to fail and they all landed on
        // `REASON_HOST`, which says only that the host layer objected. Knowing
        // whether the keyboard is missing a characteristic, refusing an
        // attribute, or running us out of room decides what to change next.
        trouble_host::Error::NotFound => 0x03,
        trouble_host::Error::Att(_) => 0x05,
        trouble_host::Error::InsufficientSpace | trouble_host::Error::OutOfMemory => 0x07,
        trouble_host::Error::NotSupported => 0x0a,
        trouble_host::Error::InvalidState => 0x0b,
        trouble_host::Error::UnexpectedGattResponse
        | trouble_host::Error::InvalidValue
        | trouble_host::Error::UnexpectedDataLength { .. } => 0x0c,
        _ => REASON_HOST,
    }
}

/// Reduces a failure to the low nibble of the diagnostics `last_error` byte.
///
/// HCI status codes are the useful ones and they fit in the low nibble for the
/// values that occur here, so they are passed through and can be looked up in
/// the Bluetooth core specification. Recording a magic number instead cost two
/// wrong guesses during bring-up before the real status, `0x0C Command
/// Disallowed`, made the cause obvious.
fn ble_error_code<E>(error: &BleHostError<E>) -> u8 {
    match error {
        BleHostError::Controller(_) => REASON_CONTROLLER,
        BleHostError::BleHost(trouble_host::Error::Hci(status)) => {
            let raw: u8 = (*status).into();
            if raw == 0 || raw > 0x0f {
                REASON_HOST
            } else {
                raw
            }
        }
        BleHostError::BleHost(error) => host_error_code(error),
    }
}

/// Consecutive failures, used to space out reconnection attempts.
static BACKOFF_STEPS: AtomicU8 = AtomicU8::new(0);
/// Longest wait between attempts. Long enough to be typed against, short
/// enough that a keyboard coming back into range is picked up promptly.
const BACKOFF_CEILING: Duration = Duration::from_secs(8);

async fn retry(policy: &mut CentralPolicy, error: u8, slot: u8) {
    let _ = policy.apply(CentralEvent::RetryableFailure(error));
    let state = if ACTIVE_BLE_SLOTS.load(Ordering::Acquire) == 0 {
        policy.state()
    } else {
        BridgeState::Subscribed
    };
    BRIDGE_STATE.store(state as u8, Ordering::Release);
    if source_is_active(slot) {
        set_source_state(usize::from(slot), SourceState::Failed);
    }
    LAST_ERROR.store(error, Ordering::Relaxed);

    // Retrying after 20 ms produced a hundred and forty connections in five
    // minutes, and each one generated a fresh passkey. Nobody can type a code
    // that is replaced several times a second, so the delay is not politeness
    // here — without it, passkey pairing cannot succeed at all.
    let steps = BACKOFF_STEPS.fetch_add(1, Ordering::Relaxed).min(3);
    let wait = Duration::from_secs(1 << steps);
    Timer::after(wait.min(BACKOFF_CEILING)).await;
}

/// Clears the backoff once a stage completes, so a healthy link reconnects fast.
fn clear_backoff() {
    BACKOFF_STEPS.store(0, Ordering::Relaxed);
}

impl RequestHandler for ConfigRequestHandler {
    fn get_report(&mut self, id: ReportId, buf: &mut [u8]) -> Option<usize> {
        let ReportId::Feature(report_id) = id else {
            return None;
        };
        if report_id != ukf_nrf52840_ble_usb::uf2_reset::CONFIG_REPORT_ID
            || buf.len() < diagnostics_report::DIAGNOSTICS_REPORT_LEN
        {
            return None;
        }
        let selected_keymap = KEYMAP_CHUNK.swap(0xff, Ordering::AcqRel);
        let report = if selected_keymap != 0xff {
            let payload = encode_keymap_payload(&active_keymap());
            diagnostics_report::encode_keymap_chunk(payload.as_slice(), selected_keymap)
                .unwrap_or_else(diagnostics_snapshot)
        } else {
            match PANIC_CHUNK.swap(0xff, Ordering::AcqRel) {
                0xff => diagnostics_snapshot(),
                diagnostics_report::SELECT_IDENTITY => {
                    diagnostics_report::encode_identity(&diagnostics_report::firmware_identity())
                }
                diagnostics_report::SELECT_TRANSFER => transfer_snapshot(),
                diagnostics_report::SELECT_PROFILE => {
                    diagnostics_report::encode_profile_block(&diagnostics_report::ProfileReport {
                        profile: active_profile().into(),
                        stored: PROFILE_STORED.load(Ordering::Acquire),
                        pending: profile_pending(),
                    })
                }
                SELECT_SOURCE => source_snapshot(SOURCE_SELECTOR.load(Ordering::Acquire)),
                SELECT_PASSKEY => {
                    diagnostics_report::encode_passkey(&diagnostics_report::SecurityReport {
                        passkey: PASSKEY.load(Ordering::Acquire),
                        passkey_serial: PASSKEY_SERIAL.load(Ordering::Acquire),
                        disconnect_reason: DISCONNECT_REASON.load(Ordering::Relaxed),
                        pairing_failure: PAIRING_FAILURE.load(Ordering::Relaxed),
                        bond_flags: bond_flags(),
                        hogp_refusal: HOGP_REFUSAL.load(Ordering::Relaxed),
                        notify_refusal: NOTIFY_REFUSAL.load(Ordering::Relaxed),
                        notify_handle: NOTIFY_HANDLE.load(Ordering::Relaxed),
                        notify_expected: NOTIFY_EXPECTED.load(Ordering::Relaxed),
                        bond_flash_error: BOND_FLASH_ERROR.load(Ordering::Relaxed),
                        bond_flash_errno: BOND_FLASH_ERRNO.load(Ordering::Relaxed),
                        usb_hogp_refusal: USB_HOGP_REFUSAL.load(Ordering::Relaxed),
                        link_setup_failed: LINK_SETUP_FAILED.load(Ordering::Relaxed),
                        stuck_key_releases: STUCK_KEY_RELEASES.load(Ordering::Relaxed),
                        liveness_probe_failures: LIVENESS_PROBE_FAILURES.load(Ordering::Relaxed),
                        liveness_probe_error: LIVENESS_PROBE_ERROR.load(Ordering::Relaxed),
                        inject_refusal: INJECT_REFUSAL.load(Ordering::Relaxed),
                    })
                }
                chunk => {
                    let mut text = [0; diagnostics_report::PANIC_TEXT_CHUNK_LEN];
                    let len = panic_message_chunk(chunk, &mut text);
                    diagnostics_report::encode_panic_text(chunk, &text[..len])
                }
            }
        };
        buf[..report.len()].copy_from_slice(&report);
        Some(report.len())
    }

    fn set_report(&mut self, id: ReportId, data: &[u8]) -> OutResponse {
        let ReportId::Feature(report_id) = id else {
            return OutResponse::Rejected;
        };

        match classify_feature_report(report_id, data) {
            ConfigRequest::ResetIntoBootloader => {
                RESET_REQUESTED.store(true, Ordering::Release);
                OutResponse::Accepted
            }
            ConfigRequest::InjectSourceReport(report) => INJECTED_REPORTS
                .try_send(report)
                .map_or(OutResponse::Rejected, |_| OutResponse::Accepted),
            ConfigRequest::SetPairingMethod { passkey } => {
                REQUIRE_PASSKEY.store(passkey, Ordering::Release);
                PAIRING_METHOD_CHANGED.signal(());
                OutResponse::Accepted
            }
            ConfigRequest::SetPairingMode(open) => {
                PAIRING_MODE.store(open, Ordering::Release);
                if open {
                    PAIRING_OPENED.signal(());
                }
                OutResponse::Accepted
            }
            ConfigRequest::SelectPanicChunk(chunk) => {
                KEYMAP_CHUNK.store(0xff, Ordering::Release);
                PANIC_CHUNK.store(chunk, Ordering::Release);
                OutResponse::Accepted
            }
            ConfigRequest::SelectSource(slot) => {
                KEYMAP_CHUNK.store(0xff, Ordering::Release);
                SOURCE_SELECTOR.store(slot, Ordering::Release);
                PANIC_CHUNK.store(SELECT_SOURCE, Ordering::Release);
                OutResponse::Accepted
            }
            ConfigRequest::SelectKeymapChunk(chunk) => {
                if chunk >= diagnostics_report::KEYMAP_CHUNK_COUNT_MAX {
                    OutResponse::Rejected
                } else {
                    PANIC_CHUNK.store(0xff, Ordering::Release);
                    KEYMAP_CHUNK.store(chunk, Ordering::Release);
                    OutResponse::Accepted
                }
            }
            ConfigRequest::WriteBegin {
                target,
                total_len,
                payload_crc,
            } => transfer_response(
                CONFIG_TRANSFER
                    .lock(|cell| cell.borrow_mut().begin(target, total_len, payload_crc)),
            ),
            ConfigRequest::WriteChunk { index, data, count } => {
                let result = if usize::from(count) <= data.len() {
                    CONFIG_TRANSFER
                        .lock(|cell| cell.borrow_mut().chunk(index, &data[..usize::from(count)]))
                } else {
                    let invalid = [0; 24];
                    CONFIG_TRANSFER.lock(|cell| cell.borrow_mut().chunk(index, &invalid))
                };
                transfer_response(result)
            }
            ConfigRequest::WriteCommit {
                target,
                total_len,
                payload_crc,
            } if target == TARGET_PROFILE => {
                let result = CONFIG_TRANSFER.lock(|cell| {
                    cell.borrow_mut()
                        .commit(target, total_len, payload_crc)
                        .map(decode_profile_payload)
                });
                match result {
                    Ok(Ok(profile)) => queue_profile_write(profile),
                    Ok(Err(error)) => {
                        let code = profile_payload_error_code(&error);
                        PROFILE_FLASH_ERROR.store(0x40 | code, Ordering::Relaxed);
                        PROFILE_WRITE_FAILED.store(true, Ordering::Relaxed);
                        LAST_ERROR.store(PHASE_PROFILE_FLASH | code, Ordering::Relaxed);
                        OutResponse::Rejected
                    }
                    Err(error) => transfer_response(Err(error)),
                }
            }
            ConfigRequest::WriteCommit {
                target,
                total_len,
                payload_crc,
            } if target == TARGET_SOURCE_PROFILE => {
                let result = CONFIG_TRANSFER.lock(|cell| {
                    cell.borrow_mut()
                        .commit(target, total_len, payload_crc)
                        .map(decode_source_profile_payload)
                });
                match result {
                    Ok(Ok((slot, profile)))
                        if is_runtime_source_slot(slot)
                            && (slot == ukf_nrf52840_ble_usb::VIRTUAL_SOURCE.0
                                || registered_bond(usize::from(slot)).is_some()) =>
                    {
                        queue_source_profile_apply(slot, profile.into())
                    }
                    Ok(Ok((_slot, _profile))) => {
                        // The codec knows the fixed slot range, but an empty
                        // bond slot has no report path to configure.
                        PROFILE_APPLY_ERROR.store(5, Ordering::Relaxed);
                        LAST_ERROR.store(PHASE_PROFILE_APPLY | 5, Ordering::Relaxed);
                        OutResponse::Rejected
                    }
                    Ok(Err(error)) => {
                        let code = profile_payload_error_code(&error);
                        PROFILE_FLASH_ERROR.store(0x40 | code, Ordering::Relaxed);
                        PROFILE_WRITE_FAILED.store(true, Ordering::Relaxed);
                        LAST_ERROR.store(PHASE_PROFILE_FLASH | code, Ordering::Relaxed);
                        OutResponse::Rejected
                    }
                    Err(error) => transfer_response(Err(error)),
                }
            }
            ConfigRequest::WriteCommit {
                target,
                total_len,
                payload_crc,
            } if target == TARGET_KEYMAP => {
                let result = CONFIG_TRANSFER.lock(|cell| {
                    cell.borrow_mut()
                        .commit(target, total_len, payload_crc)
                        .map(decode_keymap_payload)
                });
                match result {
                    Ok(Ok(keymap)) => queue_keymap_write(keymap),
                    Ok(Err(error)) => {
                        let code = keymap_payload_error_code(&error);
                        KEYMAP_APPLY_ERROR.store(code, Ordering::Relaxed);
                        LAST_ERROR.store(PHASE_KEYMAP | code, Ordering::Relaxed);
                        OutResponse::Rejected
                    }
                    Err(error) => transfer_response(Err(error)),
                }
            }
            ConfigRequest::WriteCommit {
                target,
                total_len,
                payload_crc,
            } if target == TARGET_BOND_MANAGEMENT => {
                let result = CONFIG_TRANSFER.lock(|cell| {
                    cell.borrow_mut()
                        .commit(target, total_len, payload_crc)
                        .map(decode_bond_management)
                });
                match result {
                    Ok(Ok(request)) => RADIO_CONTROL_REQUESTS
                        .try_send(RadioControl::Bond(request))
                        .map_or(OutResponse::Rejected, |_| OutResponse::Accepted),
                    Ok(Err(_)) => OutResponse::Rejected,
                    Err(error) => transfer_response(Err(error)),
                }
            }
            ConfigRequest::WriteCommit {
                target,
                total_len,
                payload_crc,
            } => transfer_response(CONFIG_TRANSFER.lock(|cell| {
                cell.borrow_mut()
                    .commit(target, total_len, payload_crc)
                    .map(|_| ())
            })),
            ConfigRequest::Rejected(_) => OutResponse::Rejected,
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // The BLE build deliberately does not force `HfclkSource::ExternalXtal`
    // here: MPSL owns CLOCK once it starts and manages the HFXO requests the
    // radio and USB both need. Overriding it before MPSL initialises is the
    // kind of double ownership that produced the earlier failures.
    //
    // The interrupt priorities, however, must move. `nrf-mpsl` puts RADIO,
    // TIMER0, and RTC0 at P0 because the controller has to service the radio at
    // exact anchor points. Everything else on this chip starts at P0 too: the
    // NVIC resets to zero and the USB driver enables its interrupt without ever
    // setting a priority, while embassy-time defaults RTC1 to P0. Equal
    // priority means no preemption, so a USB transfer in progress holds the
    // radio off.
    //
    // Scanning tolerates that — a missed scan window costs nothing. Connection
    // establishment does not, and the controller asserted inside
    // `LE Create Connection` on every attempt.
    let mut config = embassy_nrf::config::Config::default();
    // The radio runs off the 32 MHz crystal. embassy defaults to the internal
    // oscillator so that crystal-less boards still boot, and its accuracy is
    // nowhere near the tolerance a BLE connection has to hold. Scanning does
    // not care — a drifting scan window still catches advertisements — but a
    // connection is a schedule of anchor points, and the controller asserted
    // inside `LE Create Connection` every time. The `nrf-sdc` example sets this
    // before MPSL starts, and MPSL taking CLOCK afterwards is what that example
    // does too.
    config.hfclk_source = embassy_nrf::config::HfclkSource::ExternalXtal;
    config.time_interrupt_priority = Priority::P2;
    config.gpiote_interrupt_priority = Priority::P2;
    let peripherals = embassy_nrf::init(config);
    // P2 also keeps RNG above the P4 low-priority context that draws from it,
    // which would otherwise wait on an interrupt that cannot preempt it.
    interrupt::USBD.set_priority(Priority::P2);
    interrupt::RNG.set_priority(Priority::P2);

    // Both XIAO Sense user LEDs are active low, so `Level::High` is off.
    let mut red_led = Output::new(peripherals.P0_26, Level::High, OutputDrive::Standard);
    let mut blue_led = Output::new(peripherals.P0_06, Level::High, OutputDrive::Standard);
    red_led.set_low();

    let mpsl_peripherals = mpsl::Peripherals::new(
        peripherals.RTC0,
        peripherals.TIMER0,
        peripherals.TEMP,
        peripherals.PPI_CH19,
        peripherals.PPI_CH30,
        peripherals.PPI_CH31,
    );
    // The XIAO Sense has no low-frequency crystal wired for this use, so the
    // internal RC oscillator is the source. These are Nordic's recommended
    // calibration intervals for it.
    let lfclk_config = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_RC as u8,
        rc_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_CTIV as u8,
        rc_temp_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_TEMP_CTIV as u8,
        accuracy_ppm: mpsl::raw::MPSL_DEFAULT_CLOCK_ACCURACY_PPM as u16,
        skip_wait_lfclk_started: mpsl::raw::MPSL_DEFAULT_SKIP_WAIT_LFCLK_STARTED != 0,
    };

    // Timeslots have to be declared up front. `new` asks for none, so the
    // first `mpsl_timeslot_session_open` — which is how the flash driver gets
    // to run at all — came back ENOMEM, and storing the pairing failed every
    // time. That read as "the radio is too busy while connected", which it was
    // not: there was simply no session to open.
    static SESSION_MEM: StaticCell<mpsl::SessionMem<1>> = StaticCell::new();
    let session_mem = SESSION_MEM.init(mpsl::SessionMem::new());
    static MPSL: StaticCell<MultiprotocolServiceLayer> = StaticCell::new();
    let mpsl = MPSL.init(
        mpsl::MultiprotocolServiceLayer::with_timeslots(
            mpsl_peripherals,
            Irqs,
            lfclk_config,
            session_mem,
        )
        .expect("MPSL rejected its peripherals, clock configuration, or timeslot memory"),
    );
    spawner.spawn(mpsl_task(mpsl).expect("the MPSL task is spawned exactly once"));

    let sdc_peripherals = sdc::Peripherals::new(
        peripherals.PPI_CH17,
        peripherals.PPI_CH18,
        peripherals.PPI_CH20,
        peripherals.PPI_CH21,
        peripherals.PPI_CH22,
        peripherals.PPI_CH23,
        peripherals.PPI_CH24,
        peripherals.PPI_CH25,
        peripherals.PPI_CH26,
        peripherals.PPI_CH27,
        peripherals.PPI_CH28,
        peripherals.PPI_CH29,
    );
    let mut controller_rng = rng::Rng::new(peripherals.RNG, Irqs);
    let mut controller_memory = sdc::Mem::<SDC_MEMORY_BYTES>::new();
    let controller = sdc::Builder::new()
        .expect("the controller builder starts from a valid default")
        .support_scan()
        .support_central()
        .central_count(MAX_ACTIVE_BLE_LINKS as u8)
        .expect("two central links are within the controller's limits")
        // Tells the controller how large the host's packets are and how many
        // to hold per link. Leaving it out was the difference between this
        // build and the `trouble-host` nrf-sdc central example: without it the
        // controller keeps its own defaults while the host sizes its pool to
        // `DefaultPacketPool::MTU`, and the two no longer describe the same
        // link. Every other call here already matched that example.
        .buffer_cfg(
            DefaultPacketPool::MTU as u16,
            DefaultPacketPool::MTU as u16,
            L2CAP_TX_QUEUE,
            L2CAP_RX_QUEUE,
        )
        .expect("the controller accepts the host's packet sizes and queue depths")
        .build(
            sdc_peripherals,
            &mut controller_rng,
            mpsl,
            &mut controller_memory,
        )
        .expect("the controller accepts its peripherals and memory");
    let controller = TracingController(controller);
    // Flash writes have to share the radio with the link, so this goes through
    // MPSL's timeslot-aware driver rather than touching NVMC directly.
    let bond_flash = nrf_sdc::mpsl::Flash::take(mpsl, peripherals.NVMC);

    let mut host_resources: HostResources<
        _,
        DefaultPacketPool,
        CENTRAL_CONNECTIONS,
        CENTRAL_CHANNELS,
    > = HostResources::new();

    let driver = Driver::new(peripherals.USBD, Irqs, HardwareVbusDetect::new(Irqs));

    let mut config = Config::new(USB_VENDOR_ID, USB_PRODUCT_ID);
    config.manufacturer = Some("Unified Keyboard Firmware");
    config.product = Some("XIAO BLE coexistence probe");
    config.serial_number = Some("BLE-COEXIST-PROBE");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    let mut config_descriptor = [0; 256];
    let mut bos_descriptor = [0; 256];
    let mut msos_descriptor = [0; 256];
    let mut control_buffer = [0; 128];
    let mut keyboard_state = State::new();
    let mut config_state = State::new();
    let mut config_handler = ConfigRequestHandler;

    let mut builder = Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut msos_descriptor,
        &mut control_buffer,
    );

    let mut keyboard = HidWriter::<_, 8>::new(
        &mut builder,
        &mut keyboard_state,
        HidConfig {
            report_descriptor: KeyboardReport::desc(),
            request_handler: None,
            poll_ms: 8,
            max_packet_size: 8,
            hid_subclass: HidSubclass::Boot,
            hid_boot_protocol: HidBootProtocol::Keyboard,
        },
    );
    let _config_writer = HidWriter::<_, CONFIG_CONTROL_BUFFER_LEN>::new(
        &mut builder,
        &mut config_state,
        HidConfig {
            report_descriptor: CONFIG_REPORT_DESCRIPTOR,
            request_handler: Some(&mut config_handler),
            poll_ms: 255,
            max_packet_size: CONFIG_REPORT_LEN as u16,
            hid_subclass: HidSubclass::No,
            hid_boot_protocol: HidBootProtocol::None,
        },
    );

    let mut usb = builder.build();

    let usb_task = usb.run();

    let report_task = async {
        keyboard.ready().await;
        PROFILE_READY.wait().await;
        BONDS_READY.wait().await;
        // Release everything before sending anything. A reset while a key was
        // held — a reflash, a panic, a lost cable — leaves the host holding
        // that key forever, because the report that would have released it was
        // never sent. It cost a stuck meta key that broke Japanese input on the
        // machine's own keyboard, which this bridge is supposed to leave alone.
        // One empty report at start-up ends any such key from a previous life.
        let _ = keyboard.write(&[0; 8]).await;
        red_led.set_high();
        blue_led.set_low();

        // Attaching source zero to a fresh engine cannot fail. Draining the
        // channel on an error instead would enumerate, light the ready LED, and
        // then silently swallow every keystroke — the exact failure shape that
        // took eight candidates to diagnose. Fail loudly into UF2 instead.
        // Layout conversion and nothing else. The preset also swaps Alt
        // for the meta key and turns Caps Lock into Control, which the
        // keyboard already decides for itself — and swapping Alt broke
        // `Alt` + `` ` ``, the Windows IME toggle, by sending Meta instead.
        let mut pipeline =
            ReportPipeline::new_with_ble_sources(pipeline_registrations(), active_profile())
                .expect("the bridge engine accepts its first source");
        let mut hogp = [const { HogpCentral::new() }; BOND_SLOT_COUNT];
        let mut pending = None;
        pipeline
            .set_keymap(active_keymap(), &mut PendingUsbReport(&mut pending))
            .expect("the loaded keymap can be applied before any report is held");
        // The last line of defence. Everything else here depends on the radio
        // half noticing that the link is gone; this fires on its own if it
        // does not, and it is the only thing standing between a lost link and
        // a key held down on the host indefinitely.
        let mut watchdog = StuckKeyWatchdog::new(STUCK_KEY_LIMIT.as_millis());
        loop {
            let mut releasing_for_reset = false;
            match select5(
                INJECTED_REPORTS.receive(),
                RADIO_EVENTS.receive(),
                CONFIG_APPLY_READY.wait(),
                stuck_key_deadline(watchdog.deadline()),
                RELEASE_FOR_RESET.wait(),
            )
            .await
            {
                Either5::Fifth(()) => {
                    // A reset is about to take the board away. Whatever the
                    // host is holding has to come up first, because a board in
                    // the bootloader never sends the release and the start-up
                    // report that would have cleared it is minutes away.
                    releasing_for_reset = true;
                    let mut sink = PendingUsbReport(&mut pending);
                    match pipeline.release_all(&mut sink) {
                        Ok(_) => {}
                        // Nothing more can be done for the host here, and
                        // holding the reset would strand the board.
                        Err(error) => {
                            INJECT_REFUSAL.store(pipeline_refusal_code(&error), Ordering::Relaxed)
                        }
                    }
                }
                Either5::First(report) => {
                    let mut sink = PendingUsbReport(&mut pending);
                    // Discarding this cost an afternoon. With the keyboard
                    // switched off every injected report was refused for a
                    // detached source and nothing recorded it, so the injection
                    // tool reported success while the host received nothing.
                    match pipeline.accept_virtual_report(report, &mut sink) {
                        Ok(_) => INJECT_REFUSAL.store(0, Ordering::Relaxed),
                        Err(error) => {
                            INJECT_REFUSAL.store(pipeline_refusal_code(&error), Ordering::Relaxed)
                        }
                    }
                }
                Either5::Fourth(()) => {
                    let mut sink = PendingUsbReport(&mut pending);
                    match pipeline.release_all(&mut sink) {
                        Ok(true) => count_one(&STUCK_KEY_RELEASES),
                        // Nothing was owed, or the release could not be handed
                        // over. Either way the deadline is now in the past, and
                        // leaving it there would spin this task on an instant
                        // that has already arrived. Push it out and try again.
                        Ok(false) | Err(_) => watchdog.observe_liveness(now_ms()),
                    }
                }
                Either5::Third(()) => {
                    apply_pending_configuration(&mut pipeline, &mut pending);
                }
                Either5::Second(event) => match event {
                    // Only meaningful while something is held, and that is
                    // exactly when the radio half sends it.
                    RadioEvent::LinkAlive { slot } if source_is_active(slot) => {
                        watchdog.observe_liveness(now_ms())
                    }
                    RadioEvent::LinkAlive { .. } => {}
                    RadioEvent::Connected { slot } => {
                        // These three failing is exactly how today's two long
                        // hunts started: a missed `connected()` surfaced as
                        // "characteristic not found", and a missing
                        // `pipeline.connect` surfaced as reports with no
                        // source. Both wore a different face than their cause.
                        let index = usize::from(slot);
                        let mut setup = index < BOND_SLOT_COUNT;
                        if setup {
                            setup &= hogp[index].start_scan().is_ok();
                            setup &= hogp[index].connected().is_ok();
                        }
                        let active_source = SourceId(slot);
                        if let Some(record) = registered_bond(usize::from(slot)) {
                            let identity = SourceIdentity::from_address(
                                record.bond.addr,
                                record.bond.irk.is_some(),
                            );
                            let irk = record.bond.irk.map(|key| key.to_le_bytes());
                            setup &= pipeline
                                .register_ble_source(
                                    active_source,
                                    identity,
                                    SourceName::from_raw(record.name, record.name_len),
                                    irk,
                                    source_profile(usize::from(slot)),
                                )
                                .is_ok();
                        } else {
                            setup = false;
                        }
                        setup &= pipeline.connect_source(active_source).is_ok();
                        if !setup {
                            set_source_state(usize::from(slot), SourceState::Failed);
                            LINK_SETUP_FAILED.store(true, Ordering::Relaxed);
                        } else {
                            set_source_state(usize::from(slot), SourceState::Connected);
                        }
                    }
                    RadioEvent::Discovered {
                        slot,
                        report_map_len,
                        reports,
                        report_count,
                    } => {
                        // A separate `HogpCentral` from the radio half's, so it
                        // can refuse for its own reasons. Recording only the
                        // radio half's refusal would leave this one invisible.
                        let index = usize::from(slot);
                        let discovered = if index >= BOND_SLOT_COUNT {
                            false
                        } else {
                            REPORT_MAPS.lock(|cell| {
                                hogp[index]
                                    .finish_discovery(
                                        &cell.borrow()[index][..usize::from(report_map_len)],
                                        &reports[..usize::from(report_count)],
                                    )
                                    .map_err(|error| {
                                        USB_HOGP_REFUSAL
                                            .store(hogp_refusal_code(&error), Ordering::Relaxed);
                                    })
                                    .is_ok()
                            })
                        };
                        if discovered {
                            USB_HOGP_REFUSAL.store(0, Ordering::Relaxed);
                            BRIDGE_STATE.store(BridgeState::Subscribed as u8, Ordering::Release);
                        }
                    }
                    RadioEvent::Notification {
                        slot,
                        handle,
                        payload,
                    } => {
                        let mut sink = PendingUsbReport(&mut pending);
                        // Six reports arrived from the keyboard and none
                        // reached USB. Which of the three refusals fired, and
                        // the handle it saw against the one it expected, is the
                        // whole difference between a wiring mistake and a
                        // transformation that dropped the report.
                        NOTIFY_HANDLE.store(handle, Ordering::Relaxed);
                        let index = usize::from(slot);
                        let expected = if index < BOND_SLOT_COUNT {
                            hogp[index].subscribed_handle()
                        } else {
                            0
                        };
                        NOTIFY_EXPECTED.store(expected, Ordering::Relaxed);
                        let result = if index < BOND_SLOT_COUNT {
                            Some(hogp[index].accept_notification_from(
                                SourceId(slot),
                                handle,
                                &payload,
                                &mut pipeline,
                                &mut sink,
                            ))
                        } else {
                            None
                        };
                        match result {
                            Some(Ok(_)) => {
                                REPORTS_FORWARDED.fetch_add(1, Ordering::Relaxed);
                                NOTIFY_REFUSAL.store(0, Ordering::Relaxed);
                            }
                            Some(Err(error)) => {
                                NOTIFY_REFUSAL
                                    .store(notify_refusal_code(&error), Ordering::Relaxed);
                            }
                            None => NOTIFY_REFUSAL.store(1, Ordering::Relaxed),
                        }
                    }
                    RadioEvent::Disconnected { slot } => {
                        let mut sink = PendingUsbReport(&mut pending);
                        let _ = pipeline.disconnect_source(SourceId(slot), &mut sink);
                        if let Some(central) = hogp.get_mut(usize::from(slot)) {
                            central.disconnected();
                        }
                        let state = if registered_bond(usize::from(slot)).is_some() {
                            SourceState::Disconnected
                        } else {
                            SourceState::Unregistered
                        };
                        set_source_state(usize::from(slot), state);
                        let bridge_state = if ACTIVE_BLE_SLOTS.load(Ordering::Acquire) == 0 {
                            BridgeState::Scanning
                        } else {
                            BridgeState::Subscribed
                        };
                        BRIDGE_STATE.store(bridge_state as u8, Ordering::Release);
                    }
                },
            }
            let emitted = pending.take();
            if let Some(report) = emitted {
                // A write only fails once the host has taken the endpoint away,
                // at which point the report has no destination anyway.
                let _ = keyboard.write(&report).await;
            }
            // Driven by what actually went to the host rather than by what the
            // keyboard sent, because the stuck key is the one the host thinks
            // is down. The radio half reads this to decide whether a silent
            // link is worth spending a probe on.
            let holds_keys = pipeline.holds_keys();
            KEYS_HELD.store(holds_keys, Ordering::Relaxed);
            if !holds_keys {
                watchdog.observe_output(BootKeyboardReport::EMPTY, now_ms());
            } else if let Some(report) = emitted {
                watchdog.observe_output(BootKeyboardReport::from_bytes(report), now_ms());
            }
            if releasing_for_reset {
                // Answered only after the write, because the reset is waiting
                // on this to mean "the host has it", not "we decided to send
                // it". Signalling earlier would race the reset against the
                // report it exists to deliver.
                RELEASED_FOR_RESET.signal(());
            }
        }
    };

    // Closes the pairing window if nothing came of it. Pairing succeeding
    // closes it too, so this only fires when no keyboard turned up.
    let pairing_window_task = async {
        loop {
            PAIRING_OPENED.wait().await;
            match with_timeout(PAIRING_WINDOW, PAIRING_OPENED.wait()).await {
                // Reopened while open: start the window again rather than
                // closing it out from under a request that just arrived.
                Ok(()) => continue,
                Err(TimeoutError) => PAIRING_MODE.store(false, Ordering::Release),
            }
        }
    };

    let reset_task = async {
        loop {
            if RESET_REQUESTED.load(Ordering::Acquire) {
                // The host asked for this reset, which in practice means a
                // reflash is about to start. Whatever is held has to come up
                // before the board disappears into the bootloader.
                release_then_reset_into_uf2().await;
            }
            Timer::after(RESET_POLL_INTERVAL).await;
        }
    };

    let radio = radio_task(controller, &mut host_resources, bond_flash);
    join3(
        usb_task,
        report_task,
        select(select(reset_task, pairing_window_task), radio),
    )
    .await;
}
