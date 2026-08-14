//! What the bridge must not change on its way to the host.

use ukf_core::{
    BootKeyboardReport, BridgeEngine, BridgeProfile, InputTransport, SourceId, VIRTUAL_SOURCE_ID,
};

#[test]
fn alt_grave_reaches_the_host_as_alt_grave() {
    // Windows toggles the IME with Alt and the grave key, and on a JIS layout
    // that usage is the 半角/全角 key. Both halves have to survive: converting
    // the usage would send a bracket, and swapping Alt for the meta key would
    // send Meta and grave, which does nothing. The bridge preset did the
    // second, so the combination silently stopped working on hardware.
    let mut bridge = BridgeEngine::new();
    bridge
        .attach(SourceId(0), InputTransport::Ble, BridgeProfile::US_JIS)
        .expect("a fresh engine accepts its first source");

    let output = bridge
        .submit_boot_report(
            SourceId(0),
            BootKeyboardReport {
                modifiers: 0b0000_0100,
                keys: [0x35, 0, 0, 0, 0, 0],
            },
        )
        .expect("an attached source accepts a report");

    assert_eq!(output.report.modifiers, 0b0000_0100);
    assert_eq!(output.report.keys[0], 0x35);
}

#[test]
fn the_plain_conversion_leaves_caps_lock_alone() {
    // Caps Lock as Control is the keyboard's business, not the bridge's, and
    // doing it here as well cancels the switch on the keyboard.
    let mut bridge = BridgeEngine::new();
    bridge
        .attach(SourceId(0), InputTransport::Ble, BridgeProfile::US_JIS)
        .expect("a fresh engine accepts its first source");

    let output = bridge
        .submit_boot_report(
            SourceId(0),
            BootKeyboardReport {
                modifiers: 0,
                keys: [0x39, 0, 0, 0, 0, 0],
            },
        )
        .expect("an attached source accepts a report");

    assert_eq!(output.report.modifiers, 0);
    assert_eq!(output.report.keys[0], 0x39);
}

#[test]
fn the_plain_conversion_still_converts_text() {
    let mut bridge = BridgeEngine::new();
    bridge
        .attach(SourceId(0), InputTransport::Ble, BridgeProfile::US_JIS)
        .expect("a fresh engine accepts its first source");

    let output = bridge
        .submit_boot_report(
            SourceId(0),
            BootKeyboardReport {
                modifiers: 0b0000_0010,
                keys: [0x1f, 0, 0, 0, 0, 0],
            },
        )
        .expect("an attached source accepts a report");

    assert_eq!(output.report.modifiers, 0);
    assert_eq!(output.report.keys[0], 0x2f);
}

#[test]
fn the_wire_form_round_trips() {
    // The watchdog decides whether the host is holding anything from the bytes
    // that were actually put on the endpoint, so reading them back has to give
    // the report they were built from.
    let report = BootKeyboardReport {
        modifiers: 0b1010_0101,
        keys: [0x04, 0x05, 0x06, 0x07, 0x08, 0x09],
    };

    assert_eq!(BootKeyboardReport::from_bytes(report.to_bytes()), report);
    assert_eq!(
        BootKeyboardReport::from_bytes(BootKeyboardReport::EMPTY.to_bytes()),
        BootKeyboardReport::EMPTY
    );
}

#[test]
fn virtual_input_is_merged_by_the_same_engine_as_other_sources() {
    let mut bridge = BridgeEngine::new();
    bridge
        .attach(SourceId(0), InputTransport::Ble, BridgeProfile::NONE)
        .expect("BLE source attaches");
    bridge
        .attach(
            VIRTUAL_SOURCE_ID,
            InputTransport::Virtual,
            BridgeProfile::NONE,
        )
        .expect("virtual source attaches");

    bridge
        .submit_boot_report(
            SourceId(0),
            BootKeyboardReport {
                modifiers: 0,
                keys: [0x04, 0, 0, 0, 0, 0],
            },
        )
        .expect("BLE source accepts a report");
    let output = bridge
        .submit_boot_report(
            VIRTUAL_SOURCE_ID,
            BootKeyboardReport {
                modifiers: 0,
                keys: [0x05, 0, 0, 0, 0, 0],
            },
        )
        .expect("virtual source accepts a report");

    assert_eq!(output.report.keys, [0x04, 0x05, 0, 0, 0, 0]);
}
