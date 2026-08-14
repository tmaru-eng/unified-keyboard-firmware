//! System-level tests driving the bridge with the four modelled keyboards.
//!
//! These cover the claims that are easy to state and easy to break: transport
//! must not change meaning, a JIS keyboard must be passed through untouched,
//! and a disconnect must release only its own keys.

use ukf_core::mock::{MOD_LEFT_SHIFT, MockError, MockKeyboard, PhysicalLayout, jis_usage};
use ukf_core::{BridgeEngine, BridgeProfile, InputTransport, SourceId};

const US_DIGIT_2: u8 = 0x1F;
const JIS_AT: u8 = 0x2F;
const KEY_A: u8 = 0x04;
const KEY_B: u8 = 0x05;

fn attach(engine: &mut BridgeEngine, id: SourceId, keyboard: &MockKeyboard) {
    engine
        .attach(id, keyboard.transport(), keyboard.profile())
        .expect("the source identifier is within capacity");
}

#[test]
fn the_same_layout_produces_the_same_output_over_usb_and_ble() {
    // The transport is a route, not a meaning. If this ever diverges, one of
    // the two adapters has grown behavior that belongs in the core.
    let mut over_usb = MockKeyboard::ansi_us_usb();
    let mut over_ble = MockKeyboard::ansi_us_ble();

    let mut usb_engine = BridgeEngine::new();
    let mut ble_engine = BridgeEngine::new();
    attach(&mut usb_engine, SourceId(0), &over_usb);
    attach(&mut ble_engine, SourceId(0), &over_ble);

    let usb_chord = over_usb
        .chord(MOD_LEFT_SHIFT, US_DIGIT_2)
        .expect("Shift+2 exists on ANSI US");
    let ble_chord = over_ble
        .chord(MOD_LEFT_SHIFT, US_DIGIT_2)
        .expect("Shift+2 exists on ANSI US");
    assert_eq!(usb_chord, ble_chord);

    let usb_output = usb_engine
        .submit_boot_report(SourceId(0), usb_chord)
        .expect("the source is attached");
    let ble_output = ble_engine
        .submit_boot_report(SourceId(0), ble_chord)
        .expect("the source is attached");

    assert_eq!(usb_output, ble_output);
    // `Shift+2` on ANSI US is `@`, which a JIS host produces unshifted.
    assert_eq!(usb_output.report.modifiers, 0);
    assert!(usb_output.report.contains(JIS_AT));
}

#[test]
fn a_jis_keyboard_is_passed_through_untouched_over_both_transports() {
    for mut keyboard in [MockKeyboard::jis_usb(), MockKeyboard::jis_ble()] {
        assert_eq!(keyboard.profile(), BridgeProfile::NONE);

        let mut engine = BridgeEngine::new();
        attach(&mut engine, SourceId(0), &keyboard);

        let chord = keyboard
            .chord(MOD_LEFT_SHIFT, US_DIGIT_2)
            .expect("the digit row exists on JIS too");
        let output = engine
            .submit_boot_report(SourceId(0), chord)
            .expect("the source is attached");

        // A JIS keyboard already matches the host. Converting it would be a
        // defect, not a feature.
        assert_eq!(output.report, chord);
    }
}

#[test]
fn japanese_only_keys_exist_on_jis_and_survive_the_bridge() {
    let japanese_only = [
        jis_usage::RO,
        jis_usage::YEN,
        jis_usage::HENKAN,
        jis_usage::MUHENKAN,
        jis_usage::KANA,
        jis_usage::EISU,
    ];

    for usage in japanese_only {
        let mut keyboard = MockKeyboard::jis_usb();
        let mut engine = BridgeEngine::new();
        attach(&mut engine, SourceId(0), &keyboard);

        let pressed = keyboard.press(usage).expect("the key exists on JIS");
        let output = engine
            .submit_boot_report(SourceId(0), pressed)
            .expect("the source is attached");
        assert!(output.report.contains(usage), "usage {usage:#04x} was lost");
    }
}

#[test]
fn an_ansi_us_keyboard_cannot_produce_japanese_only_keys() {
    let mut keyboard = MockKeyboard::ansi_us_usb();

    assert_eq!(
        keyboard.press(jis_usage::KANA),
        Err(MockError::NotOnLayout {
            usage: jis_usage::KANA,
            layout: PhysicalLayout::AnsiUs,
        })
    );
}

#[test]
fn disconnecting_one_keyboard_releases_only_its_own_keys() {
    let mut ble = MockKeyboard::ansi_us_ble();
    let mut usb = MockKeyboard::jis_usb();

    let mut engine = BridgeEngine::new();
    attach(&mut engine, SourceId(0), &ble);
    attach(&mut engine, SourceId(1), &usb);

    engine
        .submit_boot_report(SourceId(0), ble.press(KEY_A).expect("A exists"))
        .expect("the BLE source is attached");
    let both = engine
        .submit_boot_report(SourceId(1), usb.press(KEY_B).expect("B exists"))
        .expect("the USB source is attached");
    assert!(both.report.contains(KEY_A) && both.report.contains(KEY_B));

    let after = engine
        .detach(SourceId(0))
        .expect("the BLE source is attached");
    assert!(
        !after.report.contains(KEY_A),
        "the disconnected keyboard's key stayed held"
    );
    assert!(
        after.report.contains(KEY_B),
        "the surviving keyboard's key was released by an unrelated disconnect"
    );
}

#[test]
fn ble_framing_carries_a_report_id_and_usb_framing_does_not() {
    let mut ble = MockKeyboard::ansi_us_ble();
    let mut usb = MockKeyboard::ansi_us_usb();
    ble.press(KEY_A).expect("A exists");
    usb.press(KEY_A).expect("A exists");

    let (ble_wire, ble_len) = ble.wire_report();
    let (usb_wire, usb_len) = usb.wire_report();

    assert_eq!(usb_len, 8, "a USB boot report is exactly eight bytes");
    assert_eq!(ble_len, 9, "a HOGP report may prefix its report ID");
    assert_eq!(ble_wire[0], 1);
    // Byte 1 of a boot report is reserved and must be zero on both.
    assert_eq!(usb_wire[1], 0);
    assert_eq!(ble_wire[2], 0);
    // Same keystroke, same payload once the prefix is removed.
    assert_eq!(&ble_wire[1..ble_len], &usb_wire[..usb_len]);
}

#[test]
fn transports_are_reported_as_attached() {
    assert_eq!(MockKeyboard::ansi_us_usb().transport(), InputTransport::Usb);
    assert_eq!(MockKeyboard::ansi_us_ble().transport(), InputTransport::Ble);
    assert_eq!(MockKeyboard::jis_usb().layout(), PhysicalLayout::Jis);
    assert_eq!(MockKeyboard::ansi_us_ble().layout(), PhysicalLayout::AnsiUs);
}
