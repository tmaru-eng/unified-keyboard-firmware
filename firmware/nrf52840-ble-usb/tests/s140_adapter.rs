//! S140 adapter acceptance tests at the target-independent boundary.

use ukf_nrf52840_ble_usb::hogp::{CentralState, HogpCentral};
use ukf_nrf52840_ble_usb::s140::advertisement_has_hid_service;

#[test]
fn s140_advertisement_filter_drives_hogp_scan_state() {
    let mut central = HogpCentral::new();
    central.start_scan().unwrap();

    // Flags followed by a complete 16-bit service list containing HID (0x1812).
    let advertisement = [2, 0x01, 0x06, 5, 0x03, 0x0f, 0x18, 0x12, 0x18];
    assert!(advertisement_has_hid_service(&advertisement));
    assert!(central.advertisement_is_hid(&[0x180f, 0x1812]));
    assert_eq!(central.state(), CentralState::Scanning);
}

#[test]
fn malformed_s140_advertisement_is_rejected() {
    // The element says it has five following bytes, but only three are present.
    assert!(!advertisement_has_hid_service(&[5, 0x03, 0x12, 0x18]));
}
