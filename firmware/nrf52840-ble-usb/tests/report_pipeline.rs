//! Host-side acceptance tests for the hardware report boundary.

use ukf_core::{BootKeyboardReport, BridgeProfile, SourceId, SourceState, VIRTUAL_SOURCE_ID};
use ukf_nrf52840_ble_usb::profile_store::PendingProfile;
use ukf_nrf52840_ble_usb::{BleSourceRegistration, PipelineError, ReportPipeline, UsbReportSink};

#[derive(Debug, Default)]
struct MemoryUsbSink {
    writes: Vec<[u8; 8]>,
}

impl UsbReportSink for MemoryUsbSink {
    type Error = ();

    fn write(&mut self, report: [u8; 8]) -> Result<(), Self::Error> {
        self.writes.push(report);
        Ok(())
    }
}

#[test]
fn ble_at_report_is_transformed_then_written_as_usb_hid() {
    let mut pipeline = ReportPipeline::new(BridgeProfile::US_JIS_PRESET).unwrap();
    let mut usb = MemoryUsbSink::default();

    pipeline
        .accept_ble_report(
            BootKeyboardReport {
                modifiers: 0b0000_0010,
                keys: [0x1f, 0, 0, 0, 0, 0],
            }
            .to_bytes(),
            &mut usb,
        )
        .unwrap();

    assert_eq!(usb.writes, [[0, 0, 0x2f, 0, 0, 0, 0, 0]]);
}

#[test]
fn malformed_ble_report_is_rejected_without_a_usb_write() {
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();

    let result = pipeline.accept_ble_payload(&[0, 0, 0x04], &mut usb);

    assert_eq!(result, Err(PipelineError::InvalidReportLength(3)));
    assert!(usb.writes.is_empty());
}

#[test]
fn disconnect_emits_a_release_report() {
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();
    pipeline
        .accept_ble_report([0, 0, 0x04, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();

    pipeline.disconnect(&mut usb).unwrap();

    assert_eq!(usb.writes.last(), Some(&[0; 8]));
}

#[test]
fn a_keyboard_that_reconnects_still_reaches_usb() {
    // The keyboard drops the link routinely: it sleeps, it goes out of range,
    // the host reboots it. On hardware this shape produced forty-two reports
    // received over BLE and none delivered to USB, because disconnecting
    // detached the source and nothing ever put it back.
    let mut pipeline = ReportPipeline::new(BridgeProfile::US_JIS_PRESET).unwrap();
    let mut usb = MemoryUsbSink::default();
    let at_sign = BootKeyboardReport {
        modifiers: 0b0000_0010,
        keys: [0x1f, 0, 0, 0, 0, 0],
    }
    .to_bytes();

    pipeline.accept_ble_report(at_sign, &mut usb).unwrap();
    pipeline.disconnect(&mut usb).unwrap();
    pipeline.connect(BridgeProfile::US_JIS_PRESET).unwrap();
    pipeline.accept_ble_report(at_sign, &mut usb).unwrap();

    let converted = [0, 0, 0x2f, 0, 0, 0, 0, 0];
    assert_eq!(usb.writes.first(), Some(&converted));
    assert_eq!(usb.writes.last(), Some(&converted));
}

#[test]
fn releasing_everything_emits_one_empty_report() {
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();
    pipeline
        .accept_ble_report([0, 0, 0x04, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();

    assert_eq!(pipeline.release_all(&mut usb), Ok(true));

    assert_eq!(usb.writes.last(), Some(&[0; 8]));
}

#[test]
fn releasing_a_host_that_holds_nothing_writes_nothing() {
    // The watchdog polls; a release on every poll would flood the endpoint with
    // reports that say what the host already believes.
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();

    assert_eq!(pipeline.release_all(&mut usb), Ok(false));
    assert!(usb.writes.is_empty());

    pipeline
        .accept_ble_report([0, 0, 0x04, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();
    pipeline.release_all(&mut usb).unwrap();

    assert_eq!(pipeline.release_all(&mut usb), Ok(false));
    assert_eq!(usb.writes.len(), 2);
}

#[test]
fn a_release_leaves_the_source_usable() {
    // The watchdog fires when the keyboard has gone quiet, not when it is gone.
    // A keyboard that answers again afterwards has to keep working without a
    // reconnection, so releasing must not detach the source.
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();
    pipeline
        .accept_ble_report([0, 0, 0x04, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();
    pipeline.release_all(&mut usb).unwrap();

    pipeline
        .accept_ble_report([0, 0, 0x05, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();

    assert_eq!(usb.writes.last(), Some(&[0, 0, 0x05, 0, 0, 0, 0, 0]));
}

#[test]
fn a_release_after_a_disconnect_does_not_write_again() {
    // Disconnecting already released the host. Firing the watchdog afterwards
    // must not fail on the detached source, and must not repeat the report.
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();
    pipeline
        .accept_ble_report([0, 0, 0x04, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();
    pipeline.disconnect(&mut usb).unwrap();

    assert_eq!(pipeline.release_all(&mut usb), Ok(false));
    assert_eq!(usb.writes.len(), 2);
}

#[test]
fn a_failed_release_is_not_recorded_as_released() {
    // A write that never reached the host leaves the key down. Reporting the
    // release anyway would disarm the watchdog on the strength of a failure.
    #[derive(Default)]
    struct RefusingSink;

    impl UsbReportSink for RefusingSink {
        type Error = ();

        fn write(&mut self, _report: [u8; 8]) -> Result<(), Self::Error> {
            Err(())
        }
    }

    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();
    pipeline
        .accept_ble_report([0, 0, 0x04, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();

    assert_eq!(
        pipeline.release_all(&mut RefusingSink),
        Err(PipelineError::Usb(()))
    );

    assert_eq!(pipeline.release_all(&mut usb), Ok(true));
    assert_eq!(usb.writes.last(), Some(&[0; 8]));
}

#[test]
fn connecting_a_link_that_never_dropped_is_harmless() {
    // The caller cannot always tell a fresh link from one that was never lost,
    // so attaching twice has to be safe rather than an error to be swallowed.
    let mut pipeline = ReportPipeline::new(BridgeProfile::US_JIS_PRESET).unwrap();
    let mut usb = MemoryUsbSink::default();

    pipeline.connect(BridgeProfile::US_JIS_PRESET).unwrap();
    pipeline
        .accept_ble_report(
            BootKeyboardReport {
                modifiers: 0,
                keys: [0x04, 0, 0, 0, 0, 0],
            }
            .to_bytes(),
            &mut usb,
        )
        .unwrap();

    assert_eq!(usb.writes, [[0, 0, 0x04, 0, 0, 0, 0, 0]]);
}

#[test]
fn a_notification_after_a_disconnect_is_still_refused() {
    // The lifecycle rule stays: a keyboard that is not connected cannot send
    // keystrokes, and a notification arriving from nowhere is a defect worth
    // surfacing rather than a report worth forwarding.
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();
    pipeline.disconnect(&mut usb).unwrap();
    let after_disconnect = usb.writes.len();

    let result = pipeline.accept_ble_report([0x02, 0, 0, 0, 0, 0, 0, 0], &mut usb);

    assert!(matches!(result, Err(PipelineError::Bridge(_))));
    assert_eq!(usb.writes.len(), after_disconnect);
}

#[test]
fn an_injected_report_reaches_the_host_with_no_keyboard_connected() {
    // Observed on hardware: with the keyboard switched off, an injected key
    // never arrived. Disconnecting detaches the source and the injection was
    // refused for a source that no longer existed — then discarded by the
    // caller, so nothing said so.
    //
    // This is also what made the stuck-key watchdog untestable. With a keyboard
    // connected the liveness probe holds the watchdog open, and without one the
    // injection never reached the host, so the two conditions never met.
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();
    pipeline.disconnect(&mut usb).unwrap();

    pipeline
        .accept_virtual_report([0x02, 0, 0, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();

    assert_eq!(usb.writes.last(), Some(&[0x02, 0, 0, 0, 0, 0, 0, 0]));
    assert!(pipeline.holds_keys());
}

#[test]
fn injecting_into_a_live_link_does_not_disturb_it() {
    // The attach only happens when the source is gone. Reattaching on every
    // injection would clear the keys a connected keyboard is holding.
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();
    pipeline
        .accept_ble_report([0, 0, 0x04, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();

    pipeline
        .accept_virtual_report([0, 0, 0x04, 0x05, 0, 0, 0, 0], &mut usb)
        .unwrap();

    assert_eq!(usb.writes.last(), Some(&[0, 0, 0x04, 0x05, 0, 0, 0, 0]));
}

#[test]
fn virtual_input_merges_with_a_pressed_ble_key() {
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();

    pipeline
        .accept_ble_report([0, 0, 0x04, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();
    pipeline
        .accept_virtual_report([0, 0, 0x05, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();

    assert_eq!(usb.writes.last(), Some(&[0, 0, 0x04, 0x05, 0, 0, 0, 0]));
}

#[test]
fn disconnecting_ble_does_not_release_virtual_keys() {
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();

    pipeline
        .accept_virtual_report([0, 0, 0x05, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();
    pipeline.disconnect(&mut usb).unwrap();

    assert_eq!(usb.writes.last(), Some(&[0, 0, 0x05, 0, 0, 0, 0, 0]));
}

#[test]
fn release_all_releases_both_sources() {
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();

    pipeline
        .accept_ble_report([0, 0, 0x04, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();
    pipeline
        .accept_virtual_report([0, 0, 0x05, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();
    pipeline.release_all(&mut usb).unwrap();

    assert_eq!(usb.writes.last(), Some(&[0; 8]));
}

#[test]
fn release_all_clears_a_nonzero_ble_slot_and_the_virtual_source() {
    let mut registrations = [None; 4];
    registrations[2] = Some(BleSourceRegistration {
        identity: ukf_core::SourceIdentity::NONE,
        name: ukf_core::SourceName::try_from_bytes(b"Keyboard 3").unwrap(),
        irk: None,
        profile: BridgeProfile::NONE,
    });
    let mut pipeline =
        ReportPipeline::new_with_ble_sources(registrations, BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();
    pipeline.connect_source(SourceId(2)).unwrap();
    pipeline
        .accept_ble_report_from(SourceId(2), [0, 0, 0x04, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();
    pipeline
        .accept_virtual_report([0, 0, 0x05, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();

    assert_eq!(pipeline.release_all(&mut usb), Ok(true));
    assert_eq!(usb.writes.last(), Some(&[0; 8]));
    pipeline
        .accept_virtual_report([0, 0, 0x06, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();
    assert_eq!(usb.writes.last(), Some(&[0, 0, 0x06, 0, 0, 0, 0, 0]));
}

#[test]
fn changing_profile_releases_keys_then_updates_ble_and_virtual_sources() {
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();
    let at_sign = [0x02, 0, 0x1f, 0, 0, 0, 0, 0];

    pipeline.accept_ble_report(at_sign, &mut usb).unwrap();
    pipeline.accept_virtual_report(at_sign, &mut usb).unwrap();
    let writes_before_change = usb.writes.len();

    pipeline
        .set_profile(BridgeProfile::US_JIS_PRESET, &mut usb)
        .unwrap();

    assert_eq!(usb.writes.len(), writes_before_change + 1);
    assert_eq!(usb.writes.last(), Some(&[0; 8]));

    pipeline.accept_ble_report(at_sign, &mut usb).unwrap();
    assert_eq!(usb.writes.last(), Some(&[0, 0, 0x2f, 0, 0, 0, 0, 0]));
    pipeline.release_all(&mut usb).unwrap();

    pipeline
        .accept_virtual_report([0, 0, 0x39, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();
    assert_eq!(usb.writes.last(), Some(&[0x01, 0, 0, 0, 0, 0, 0, 0]));
}

#[test]
fn restored_ble_registrations_accept_an_idle_global_profile_change() {
    let mut registrations = [None; 4];
    registrations[0] = Some(BleSourceRegistration {
        identity: ukf_core::SourceIdentity::from_address([1, 2, 3, 4, 5, 6], true),
        name: ukf_core::SourceName::try_from_bytes(b"Keyboard A").unwrap(),
        irk: None,
        profile: BridgeProfile::NONE,
    });
    let mut pipeline =
        ReportPipeline::new_with_ble_sources(registrations, BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();

    pipeline
        .set_profile(BridgeProfile::US_JIS, &mut usb)
        .unwrap();

    assert_eq!(
        pipeline.source_profile(SourceId(0)),
        Ok(BridgeProfile::US_JIS)
    );
    assert_eq!(
        pipeline.source_profile(VIRTUAL_SOURCE_ID),
        Ok(BridgeProfile::US_JIS)
    );
}

#[test]
fn source_profile_change_releases_all_sources_before_only_that_source_changes() {
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();
    let at_sign = [0x02, 0, 0x1f, 0, 0, 0, 0, 0];

    pipeline.accept_ble_report(at_sign, &mut usb).unwrap();
    pipeline
        .accept_virtual_report([0, 0, 0x04, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();
    pipeline
        .set_source_profile(SourceId(0), BridgeProfile::US_JIS, &mut usb)
        .unwrap();

    assert_eq!(usb.writes.last(), Some(&[0; 8]));
    assert_eq!(
        pipeline.source_profile(SourceId(0)),
        Ok(BridgeProfile::US_JIS)
    );
    assert_eq!(
        pipeline.source_profile(VIRTUAL_SOURCE_ID),
        Ok(BridgeProfile::NONE)
    );

    pipeline
        .accept_virtual_report([0, 0, 0x1f, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();
    assert_eq!(usb.writes.last(), Some(&[0, 0, 0x1f, 0, 0, 0, 0, 0]));
}

#[test]
fn runtime_source_allowlist_excludes_reserved_registration_slots() {
    assert!(ukf_nrf52840_ble_usb::is_runtime_source_slot(0));
    assert!(ukf_nrf52840_ble_usb::is_runtime_source_slot(1));
    assert!(ukf_nrf52840_ble_usb::is_runtime_source_slot(2));
    assert!(ukf_nrf52840_ble_usb::is_runtime_source_slot(3));
    assert!(ukf_nrf52840_ble_usb::is_runtime_source_slot(4));
    assert!(!ukf_nrf52840_ble_usb::is_runtime_source_slot(5));
}

#[test]
fn a_source_profile_survives_ble_disconnect_and_reconnect() {
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();

    pipeline
        .set_source_profile(SourceId(0), BridgeProfile::US_JIS, &mut usb)
        .unwrap();
    pipeline.disconnect(&mut usb).unwrap();
    let ble_profile = pipeline.source_profile(SourceId(0)).unwrap();
    pipeline.connect(ble_profile).unwrap();

    assert_eq!(
        pipeline.source_profile(SourceId(0)),
        Ok(BridgeProfile::US_JIS)
    );
}

#[test]
fn source_snapshots_report_ble_slot_zero_and_virtual_slot_four_independently() {
    let pipeline = ReportPipeline::new(BridgeProfile::US_JIS).unwrap();

    assert_eq!(
        pipeline.source_snapshot(SourceId(0)).unwrap().state,
        SourceState::Connected
    );
    assert_eq!(
        pipeline.source_snapshot(VIRTUAL_SOURCE_ID).unwrap().state,
        SourceState::Connected
    );
    assert_eq!(
        pipeline.source_snapshot(VIRTUAL_SOURCE_ID).unwrap().slot,
        VIRTUAL_SOURCE_ID
    );
}

#[test]
fn applying_a_profile_does_not_drain_its_persistence_queue() {
    let requested = BridgeProfile::US_JIS_PRESET;
    let mut pending: PendingProfile = None;
    let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();

    pending.replace(requested.into());
    pipeline.set_profile(requested, &mut usb).unwrap();
    pipeline
        .accept_ble_report([0x02, 0, 0x1f, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();

    assert_eq!(usb.writes.last(), Some(&[0, 0, 0x2f, 0, 0, 0, 0, 0]));
    assert!(pending.is_some());
    assert_eq!(pending.take(), Some(requested.into()));
}

#[test]
fn a_registered_ble_slot_can_be_connected_without_being_slot_zero() {
    let mut registrations = [None; 4];
    registrations[2] = Some(BleSourceRegistration {
        identity: ukf_core::SourceIdentity::from_address([1, 2, 3, 4, 5, 6], true),
        name: ukf_core::SourceName::try_from_bytes(b"Corne").unwrap(),
        irk: None,
        profile: BridgeProfile::US_JIS,
    });
    let mut pipeline =
        ReportPipeline::new_with_ble_sources(registrations, BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();

    assert_eq!(
        pipeline.source_snapshot(SourceId(2)).unwrap().state,
        SourceState::Disconnected
    );
    assert_eq!(
        pipeline.source_snapshot(SourceId(0)).unwrap().state,
        SourceState::Unregistered
    );

    pipeline.connect_source(SourceId(2)).unwrap();
    pipeline
        .accept_ble_report_from(SourceId(2), [0x02, 0, 0x1f, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();

    assert_eq!(usb.writes.last(), Some(&[0, 0, 0x2f, 0, 0, 0, 0, 0]));
    assert_eq!(
        pipeline.source_snapshot(SourceId(2)).unwrap().state,
        SourceState::Connected
    );
}

#[test]
fn disconnecting_one_registered_ble_slot_leaves_the_virtual_source_usable() {
    let mut registrations = [None; 4];
    registrations[1] = Some(BleSourceRegistration {
        identity: ukf_core::SourceIdentity::NONE,
        name: ukf_core::SourceName::try_from_bytes(b"Keyboard 2").unwrap(),
        irk: None,
        profile: BridgeProfile::NONE,
    });
    let mut pipeline =
        ReportPipeline::new_with_ble_sources(registrations, BridgeProfile::NONE).unwrap();
    let mut usb = MemoryUsbSink::default();
    pipeline.connect_source(SourceId(1)).unwrap();
    pipeline
        .accept_ble_report_from(SourceId(1), [0, 0, 0x04, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();
    pipeline.disconnect_source(SourceId(1), &mut usb).unwrap();
    pipeline
        .accept_virtual_report([0, 0, 0x05, 0, 0, 0, 0, 0], &mut usb)
        .unwrap();

    assert_eq!(usb.writes.last(), Some(&[0, 0, 0x05, 0, 0, 0, 0, 0]));
}
