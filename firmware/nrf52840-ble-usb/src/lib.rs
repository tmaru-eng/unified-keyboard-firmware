#![no_std]
#![forbid(unsafe_code)]

//! Board-independent report pipeline used by the nRF52840 firmware adapter.
//!
//! The actual radio and USB drivers are intentionally kept at the binary
//! boundary. This module makes their shared keyboard semantics testable on a
//! host before hardware is involved.

use ukf_core::{
    BootKeyboardReport, BridgeEngine, BridgeError, BridgeOutput, BridgeProfile, IRK_LEN,
    InputTransport, Keymap, SOURCE_SLOT_COUNT, SourceId, SourceIdentity, SourceName,
    SourceSnapshot, VIRTUAL_SOURCE_ID,
};

pub mod bond_management;
pub mod bond_store;
pub mod central_policy;
pub mod config_hid;
pub mod config_transfer;
pub mod configuration_updates;
pub mod detected_order_probe;
pub mod diagnostics;
pub mod diagnostics_report;
pub mod hogp;
pub mod keymap_store;
pub mod multi_link;
pub mod panic_recovery;
pub mod power_probe;
pub mod profile_store;
pub mod s140;
pub mod startup;
pub mod uf2_reset;
pub mod usb_enable_boundary;
pub mod usb_errata_preflight;
pub mod usb_power;
pub mod usb_preflight;

/// Registered BLE source slot. Source identity is carried by the slot rather
/// than derived from the peer address.
pub const BLE_SOURCE: SourceId = SourceId(0);
/// Configuration-injection source slot.
pub const VIRTUAL_SOURCE: SourceId = VIRTUAL_SOURCE_ID;

/// Returns whether the current bridge binary has a live source path for a slot.
///
/// Slots zero through three are registered BLE paths; slot four is the
/// configuration-injection source.
pub const fn is_runtime_source_slot(slot: u8) -> bool {
    (slot as usize) < ukf_core::REGISTERED_SOURCE_SLOTS || slot == VIRTUAL_SOURCE.0
}

const BLE_SOURCE_NAME: SourceName = SourceName::from_raw(*b"BLE Keyboard\0", 12);
const VIRTUAL_SOURCE_NAME: SourceName = SourceName::from_raw(*b"Virtual Input", 13);

/// Static registration metadata for one BLE source slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BleSourceRegistration {
    /// Public identity facts for diagnostics and address matching.
    pub identity: SourceIdentity,
    /// Optional private resolving key kept only inside the bridge engine.
    pub irk: Option<[u8; IRK_LEN]>,
    /// User-visible name retained for this slot.
    pub name: SourceName,
    /// Profile used whenever this slot is attached.
    pub profile: BridgeProfile,
}

/// Minimal synchronous boundary implemented by test fakes and USB task queues.
pub trait UsbReportSink {
    /// Adapter-specific write error.
    type Error;

    /// Enqueues one eight-byte USB boot-keyboard input report.
    fn write(&mut self, report: [u8; 8]) -> Result<(), Self::Error>;
}

/// Failure while validating or forwarding an input report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipelineError<E> {
    /// The BLE HID characteristic did not contain an eight-byte boot report.
    InvalidReportLength(usize),
    /// The bridge source was not in the expected lifecycle state.
    Bridge(BridgeError),
    /// The USB output adapter rejected the report.
    Usb(E),
}

/// Owns the bridge state for BLE and configuration-interface input sources.
#[derive(Clone, Copy, Debug)]
pub struct ReportPipeline {
    bridge: BridgeEngine,
    profile: BridgeProfile,
    /// Whether the last report written to USB left the host holding something.
    ///
    /// The host's belief, not the bridge's: what matters when deciding whether
    /// a release is owed is what was actually put on the wire.
    held: bool,
}

impl ReportPipeline {
    /// Creates a pipeline with BLE and virtual sources attached to the profile.
    pub fn new(profile: BridgeProfile) -> Result<Self, BridgeError> {
        let mut registrations = [None; ukf_core::REGISTERED_SOURCE_SLOTS];
        registrations[usize::from(BLE_SOURCE.0)] = Some(BleSourceRegistration {
            identity: SourceIdentity::NONE,
            irk: None,
            name: BLE_SOURCE_NAME,
            profile,
        });
        let mut pipeline = Self::new_with_ble_sources(registrations, profile)?;
        pipeline.connect(profile)?;
        Ok(pipeline)
    }

    /// Creates a pipeline with fixed BLE registrations and a connected virtual
    /// source. Only the caller-selected BLE source is attached later.
    pub fn new_with_ble_sources(
        registrations: [Option<BleSourceRegistration>; ukf_core::REGISTERED_SOURCE_SLOTS],
        virtual_profile: BridgeProfile,
    ) -> Result<Self, BridgeError> {
        let mut bridge = BridgeEngine::new();
        for (index, registration) in registrations.into_iter().enumerate() {
            let Some(registration) = registration else {
                continue;
            };
            bridge.register_source(
                SourceId(index as u8),
                InputTransport::Ble,
                registration.identity,
                registration.name,
                registration.irk,
                registration.profile,
            )?;
        }
        bridge.register_source(
            VIRTUAL_SOURCE,
            InputTransport::Virtual,
            SourceIdentity::NONE,
            VIRTUAL_SOURCE_NAME,
            None,
            virtual_profile,
        )?;
        bridge.attach(VIRTUAL_SOURCE, InputTransport::Virtual, virtual_profile)?;
        Ok(Self {
            bridge,
            profile: virtual_profile,
            held: false,
        })
    }

    /// Whether the host is currently holding anything this pipeline sent.
    pub const fn holds_keys(&self) -> bool {
        self.held
    }

    /// Releases every key the host is holding, if it is holding any.
    ///
    /// This is the stuck-key watchdog's hand on the output, so it clears
    /// *every* source: what it is protecting against is the host holding a key
    /// nobody is pressing, and which source put it there does not change that.
    ///
    /// Unlike [`Self::disconnect`] it leaves the sources attached, because it
    /// fires when a keyboard has gone *quiet*, not when it is known to be gone:
    /// one that answers again has to keep working without reconnecting.
    ///
    /// Returns whether a release was written. Writing on every call would put a
    /// report the host already believes on the wire at the watchdog's polling
    /// rate.
    pub fn release_all<S: UsbReportSink>(
        &mut self,
        usb: &mut S,
    ) -> Result<bool, PipelineError<S::Error>> {
        if !self.held {
            return Ok(false);
        }
        let mut report = BootKeyboardReport::EMPTY;
        for index in 0..SOURCE_SLOT_COUNT {
            let source = SourceId(index as u8);
            match self
                .bridge
                .submit_boot_report(source, BootKeyboardReport::EMPTY)
            {
                Ok(output) => report = output.report,
                Err(BridgeError::DetachedSource | BridgeError::UnregisteredSource) => {}
                Err(error) => return Err(PipelineError::Bridge(error)),
            }
        }
        // Recorded only after the write succeeds. A refused write leaves the
        // key down, and calling that a release would disarm the watchdog on the
        // strength of a failure.
        usb.write(report.to_bytes()).map_err(PipelineError::Usb)?;
        self.held = report != BootKeyboardReport::EMPTY;
        Ok(true)
    }

    /// Attaches the BLE source, replacing any attachment left from before.
    ///
    /// [`Self::disconnect`] detaches the source and nothing put it back, so a
    /// pipeline survived exactly one connection: after the first disconnect
    /// every report was refused because its source no longer existed. The
    /// keyboard reconnects routinely — it sleeps, it goes out of range — so
    /// this has to be called each time a link comes up.
    pub fn connect(&mut self, profile: BridgeProfile) -> Result<(), BridgeError> {
        // Detaching first makes this safe to call on a link that never dropped,
        // which is the case the caller cannot easily tell apart.
        match self.bridge.detach(BLE_SOURCE) {
            Ok(_) | Err(BridgeError::DetachedSource) => {}
            Err(error) => return Err(error),
        }
        self.profile = profile;
        self.bridge
            .attach(BLE_SOURCE, InputTransport::Ble, self.profile)?;
        Ok(())
    }

    /// Attaches one registered BLE slot using the profile stored on that slot.
    pub fn connect_source(&mut self, source: SourceId) -> Result<(), BridgeError> {
        let profile = self.bridge.profile(source)?;
        match self.bridge.detach(source) {
            Ok(_) | Err(BridgeError::DetachedSource) => {}
            Err(error) => return Err(error),
        }
        self.bridge.attach(source, InputTransport::Ble, profile)
    }

    /// Registers a newly paired BLE slot before its first report path is
    /// attached. Existing registrations may be refreshed when a slot is
    /// reused after deletion.
    pub fn register_ble_source(
        &mut self,
        source: SourceId,
        identity: SourceIdentity,
        name: SourceName,
        irk: Option<[u8; IRK_LEN]>,
        profile: BridgeProfile,
    ) -> Result<(), BridgeError> {
        self.bridge
            .register_source(source, InputTransport::Ble, identity, name, irk, profile)
    }

    /// Releases the host report, then applies one profile to both sources.
    ///
    /// A profile change can alter both key usages and modifier bits. The empty
    /// report must reach the host before either source is transformed with the
    /// new mapping, otherwise a key pressed under the old mapping could remain
    /// held under a mapping that no longer has a corresponding release.
    pub fn set_profile<S: UsbReportSink>(
        &mut self,
        profile: BridgeProfile,
        usb: &mut S,
    ) -> Result<(), PipelineError<S::Error>> {
        self.release_all(usb)?;
        match self.bridge.set_profile(BLE_SOURCE, profile) {
            Ok(()) | Err(BridgeError::DetachedSource) => {}
            Err(error) => return Err(PipelineError::Bridge(error)),
        }
        self.bridge
            .set_profile(VIRTUAL_SOURCE, profile)
            .map_err(PipelineError::Bridge)?;
        self.profile = profile;
        Ok(())
    }

    /// Returns the profile currently owned by one registered source slot.
    pub fn source_profile(&self, source: SourceId) -> Result<BridgeProfile, BridgeError> {
        self.bridge.profile(source)
    }

    /// Returns the public identity, lifecycle, transport, and profile snapshot
    /// for one slot. Private IRK bytes are intentionally not part of it.
    pub fn source_snapshot(&self, source: SourceId) -> Result<SourceSnapshot, BridgeError> {
        self.bridge.source_snapshot(source)
    }

    /// Returns all fixed-capacity source slots in slot order.
    pub fn source_snapshots(&self) -> [SourceSnapshot; SOURCE_SLOT_COUNT] {
        self.bridge.source_snapshots()
    }

    /// Changes exactly one source profile after releasing the aggregate USB
    /// report. A disconnected source may still be configured because its slot
    /// remains registered.
    pub fn set_source_profile<S: UsbReportSink>(
        &mut self,
        source: SourceId,
        profile: BridgeProfile,
        usb: &mut S,
    ) -> Result<(), PipelineError<S::Error>> {
        self.release_all(usb)?;
        self.bridge
            .set_profile(source, profile)
            .map_err(PipelineError::Bridge)
    }

    /// Returns the mapping table currently used by layout-enabled sources.
    pub const fn keymap(&self) -> Keymap {
        self.bridge.keymap()
    }

    /// Releases the aggregate report, then replaces the live keymap.
    pub fn set_keymap<S: UsbReportSink>(
        &mut self,
        keymap: Keymap,
        usb: &mut S,
    ) -> Result<(), PipelineError<S::Error>> {
        self.release_all(usb)?;
        self.bridge.set_keymap(keymap);
        Ok(())
    }

    /// Parses an eight-byte BLE boot-keyboard payload and forwards its output.
    pub fn accept_ble_payload<S: UsbReportSink>(
        &mut self,
        payload: &[u8],
        usb: &mut S,
    ) -> Result<BridgeOutput, PipelineError<S::Error>> {
        let bytes: [u8; 8] = payload
            .try_into()
            .map_err(|_| PipelineError::InvalidReportLength(payload.len()))?;
        self.accept_ble_report(bytes, usb)
    }

    /// Forwards an already validated BLE boot-keyboard report through the core.
    pub fn accept_ble_report<S: UsbReportSink>(
        &mut self,
        bytes: [u8; 8],
        usb: &mut S,
    ) -> Result<BridgeOutput, PipelineError<S::Error>> {
        let report = BootKeyboardReport {
            modifiers: bytes[0],
            keys: [bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]],
        };
        self.accept_report(BLE_SOURCE, report, usb)
    }

    /// Forwards a report from the registered BLE source that owns the link.
    pub fn accept_ble_report_from<S: UsbReportSink>(
        &mut self,
        source: SourceId,
        bytes: [u8; 8],
        usb: &mut S,
    ) -> Result<BridgeOutput, PipelineError<S::Error>> {
        let report = BootKeyboardReport {
            modifiers: bytes[0],
            keys: [bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]],
        };
        self.accept_report(source, report, usb)
    }

    /// Forwards a report from the configuration interface, as its own source.
    ///
    /// Injection used to be submitted as the BLE keyboard, which was wrong in
    /// both directions: it overwrote whatever the real keyboard was holding,
    /// and it stopped working the moment that keyboard disconnected, because
    /// the source it was borrowing had been detached. On hardware that refusal
    /// was discarded by the caller, so an injected key simply never arrived and
    /// nothing said why.
    ///
    /// The virtual source is attached for the life of the pipeline and is never
    /// detached, because there is no link to lose. That is what makes injection
    /// work with no keyboard present, and it is also the first source that is
    /// not a radio — the shape the USB host input will need.
    pub fn accept_virtual_report<S: UsbReportSink>(
        &mut self,
        bytes: [u8; 8],
        usb: &mut S,
    ) -> Result<BridgeOutput, PipelineError<S::Error>> {
        let report = BootKeyboardReport {
            modifiers: bytes[0],
            keys: [bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]],
        };
        self.accept_report(VIRTUAL_SOURCE, report, usb)
    }

    fn accept_report<S: UsbReportSink>(
        &mut self,
        source: SourceId,
        report: BootKeyboardReport,
        usb: &mut S,
    ) -> Result<BridgeOutput, PipelineError<S::Error>> {
        let output = self
            .bridge
            .submit_boot_report(source, report)
            .map_err(PipelineError::Bridge)?;
        usb.write(output.report.to_bytes())
            .map_err(PipelineError::Usb)?;
        self.held = output.report != BootKeyboardReport::EMPTY;
        Ok(output)
    }

    /// Detaches the BLE source and emits the resulting all-keys-released report.
    pub fn disconnect<S: UsbReportSink>(
        &mut self,
        usb: &mut S,
    ) -> Result<BridgeOutput, PipelineError<S::Error>> {
        let output = self
            .bridge
            .detach(BLE_SOURCE)
            .map_err(PipelineError::Bridge)?;
        usb.write(output.report.to_bytes())
            .map_err(PipelineError::Usb)?;
        self.held = output.report != BootKeyboardReport::EMPTY;
        Ok(output)
    }

    /// Detaches one registered BLE source and emits the merged report left by
    /// every other attached source.
    pub fn disconnect_source<S: UsbReportSink>(
        &mut self,
        source: SourceId,
        usb: &mut S,
    ) -> Result<BridgeOutput, PipelineError<S::Error>> {
        let output = self.bridge.detach(source).map_err(PipelineError::Bridge)?;
        usb.write(output.report.to_bytes())
            .map_err(PipelineError::Usb)?;
        self.held = output.report != BootKeyboardReport::EMPTY;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::vec::Vec;
    use ukf_core::KeymapRule;

    #[derive(Default)]
    struct Sink(Vec<[u8; 8]>);

    impl UsbReportSink for Sink {
        type Error = ();

        fn write(&mut self, report: [u8; 8]) -> Result<(), Self::Error> {
            self.0.push(report);
            Ok(())
        }
    }

    fn registrations() -> [Option<BleSourceRegistration>; ukf_core::REGISTERED_SOURCE_SLOTS] {
        [
            Some(BleSourceRegistration {
                identity: SourceIdentity::NONE,
                irk: None,
                name: SourceName::from_raw(*b"Keyboard 0\0\0\0", 10),
                profile: BridgeProfile::NONE,
            }),
            Some(BleSourceRegistration {
                identity: SourceIdentity::NONE,
                irk: None,
                name: SourceName::from_raw(*b"Keyboard 1\0\0\0", 10),
                profile: BridgeProfile::NONE,
            }),
            None,
            None,
        ]
    }

    #[test]
    fn two_ble_sources_and_virtual_input_are_aggregated_independently() {
        let mut pipeline =
            ReportPipeline::new_with_ble_sources(registrations(), BridgeProfile::NONE).unwrap();
        pipeline.connect_source(SourceId(0)).unwrap();
        pipeline.connect_source(SourceId(1)).unwrap();
        let mut sink = Sink::default();

        pipeline
            .accept_ble_report_from(SourceId(0), [0, 0, 4, 0, 0, 0, 0, 0], &mut sink)
            .unwrap();
        pipeline
            .accept_ble_report_from(SourceId(1), [0, 0, 5, 0, 0, 0, 0, 0], &mut sink)
            .unwrap();
        pipeline
            .accept_virtual_report([0, 0, 6, 0, 0, 0, 0, 0], &mut sink)
            .unwrap();

        assert_eq!(sink.0[2], [0, 0, 4, 5, 6, 0, 0, 0]);

        pipeline.disconnect_source(SourceId(0), &mut sink).unwrap();
        assert_eq!(sink.0[3], [0, 0, 5, 6, 0, 0, 0, 0]);
    }

    #[test]
    fn changing_the_keymap_releases_the_old_aggregate_before_new_input() {
        let mut pipeline =
            ReportPipeline::new_with_ble_sources(registrations(), BridgeProfile::NONE).unwrap();
        pipeline.connect_source(SourceId(0)).unwrap();
        let mut sink = Sink::default();
        pipeline
            .set_source_profile(SourceId(0), BridgeProfile::US_JIS, &mut sink)
            .unwrap();

        pipeline
            .accept_ble_report_from(SourceId(0), [0b10, 0, 0x1f, 0, 0, 0, 0, 0], &mut sink)
            .unwrap();
        let mut keymap = Keymap::new();
        keymap
            .set_rule(KeymapRule::new(0x1f, true, 0x04, false))
            .unwrap();
        pipeline.set_keymap(keymap, &mut sink).unwrap();
        pipeline
            .accept_ble_report_from(SourceId(0), [0b10, 0, 0x1f, 0, 0, 0, 0, 0], &mut sink)
            .unwrap();

        assert_eq!(sink.0[0], [0, 0, 0x2f, 0, 0, 0, 0, 0]);
        assert_eq!(sink.0[1], [0; 8]);
        assert_eq!(sink.0[2], [0, 0, 0x04, 0, 0, 0, 0, 0]);
    }
}
