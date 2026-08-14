//! Host-tested Bluetooth LE HID-over-GATT (HOGP) central boundary.
//!
//! The radio/controller task owns scanning, connecting and ATT operations. It
//! feeds their results into [`HogpCentral`], which selects the keyboard input
//! Report characteristic and normalizes its notifications into USB boot
//! keyboard reports. Keeping this state machine independent of a concrete BLE
//! stack lets it be tested on the host and prevents malformed notifications
//! from reaching the keyboard pipeline.

use crate::{PipelineError, ReportPipeline, UsbReportSink};
use ukf_core::{BridgeOutput, SourceId};

/// Bluetooth SIG Human Interface Device service UUID.
pub const HID_SERVICE_UUID: u16 = 0x1812;
/// Bluetooth SIG Report characteristic UUID.
pub const REPORT_CHARACTERISTIC_UUID: u16 = 0x2a4d;
/// Bluetooth SIG Report Map characteristic UUID.
pub const REPORT_MAP_CHARACTERISTIC_UUID: u16 = 0x2a4b;
/// Bluetooth SIG Report Reference descriptor UUID.
pub const REPORT_REFERENCE_DESCRIPTOR_UUID: u16 = 0x2908;

const INPUT_REPORT_TYPE: u8 = 1;

/// Progress of the single-keyboard HOGP central.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CentralState {
    /// No scan or connection is active.
    #[default]
    Idle,
    /// Scanning for an advertisement containing the HID service.
    Scanning,
    /// Connected; HID discovery has not completed.
    Discovering,
    /// Subscribed to the selected keyboard input Report characteristic.
    Subscribed,
}

/// Information collected from a Report characteristic and its Report
/// Reference descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReportCharacteristic {
    /// ATT value handle used to identify notifications.
    pub value_handle: u16,
    /// Whether the characteristic supports notifications.
    pub notify: bool,
    /// Report ID from descriptor 0x2908. Zero denotes a boot report without ID.
    pub report_id: u8,
    /// Report type from descriptor 0x2908: 1 input, 2 output, 3 feature.
    pub report_type: u8,
}

/// A selected keyboard input report subscription.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputSubscription {
    /// ATT value handle to subscribe to.
    pub value_handle: u16,
    /// Optional leading report ID expected in notifications.
    pub report_id: u8,
}

/// Invalid HOGP lifecycle, discovery data, or notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HogpError<E> {
    /// An operation was attempted in the wrong lifecycle state.
    InvalidState,
    /// The peer did not expose a keyboard Application Collection.
    NotKeyboard,
    /// No notifiable input Report characteristic was found.
    MissingInputReport,
    /// Notification came from a characteristic other than the subscription.
    UnexpectedHandle(u16),
    /// The normalized notification was not an eight-byte boot report.
    InvalidReportLength(usize),
    /// Report transformation or USB forwarding failed.
    Pipeline(PipelineError<E>),
}

/// Deterministic HOGP central state used by the async BLE adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct HogpCentral {
    state: CentralState,
    subscription: Option<InputSubscription>,
}

impl HogpCentral {
    /// Creates an idle central.
    pub const fn new() -> Self {
        Self {
            state: CentralState::Idle,
            subscription: None,
        }
    }

    /// Returns the current lifecycle state.
    pub const fn state(&self) -> CentralState {
        self.state
    }

    /// Starts scanning. The controller should filter for [`HID_SERVICE_UUID`].
    pub fn start_scan(&mut self) -> Result<u16, HogpError<core::convert::Infallible>> {
        if self.state != CentralState::Idle {
            return Err(HogpError::InvalidState);
        }
        self.state = CentralState::Scanning;
        Ok(HID_SERVICE_UUID)
    }

    /// Tests service UUIDs decoded from an advertising packet.
    pub fn advertisement_is_hid(&self, advertised_services: &[u16]) -> bool {
        self.state == CentralState::Scanning && advertised_services.contains(&HID_SERVICE_UUID)
    }

    /// Records a successful controller connection and begins discovery.
    pub fn connected(&mut self) -> Result<(), HogpError<core::convert::Infallible>> {
        if self.state != CentralState::Scanning {
            return Err(HogpError::InvalidState);
        }
        self.state = CentralState::Discovering;
        Ok(())
    }

    /// Selects a notifiable input Report after service, characteristic, and
    /// Report Reference descriptor discovery.
    pub fn finish_discovery(
        &mut self,
        report_map: &[u8],
        reports: &[ReportCharacteristic],
    ) -> Result<InputSubscription, HogpError<core::convert::Infallible>> {
        if self.state != CentralState::Discovering {
            return Err(HogpError::InvalidState);
        }
        if !report_map_has_keyboard_application(report_map) {
            return Err(HogpError::NotKeyboard);
        }
        let report = reports
            .iter()
            .find(|report| {
                report.notify
                    && report.report_type == INPUT_REPORT_TYPE
                    && report_map_keyboard_report_id(report_map, report.report_id)
            })
            .ok_or(HogpError::MissingInputReport)?;
        let subscription = InputSubscription {
            value_handle: report.value_handle,
            report_id: report.report_id,
        };
        self.subscription = Some(subscription);
        self.state = CentralState::Subscribed;
        Ok(subscription)
    }

    /// Validates and normalizes one notification, then forwards it through the
    /// existing transform and USB output pipeline.
    /// Handle this central subscribed to, or zero before discovery finishes.
    ///
    /// Exposed so a caller can report the handle a rejected notification
    /// arrived on alongside the one that was expected; a mismatch and a
    /// transformation failure are otherwise indistinguishable from outside.
    pub fn subscribed_handle(&self) -> u16 {
        self.subscription.map_or(0, |s| s.value_handle)
    }

    /// Runs one HID input notification through the bridge and out to USB.
    ///
    /// Refuses anything that did not come from the subscribed characteristic,
    /// so a second notifiable characteristic on the same link cannot inject
    /// keystrokes that were never subscribed to.
    pub fn accept_notification<S: UsbReportSink>(
        &self,
        value_handle: u16,
        payload: &[u8],
        pipeline: &mut ReportPipeline,
        usb: &mut S,
    ) -> Result<BridgeOutput, HogpError<S::Error>> {
        self.accept_notification_from(SourceId(0), value_handle, payload, pipeline, usb)
    }

    /// Runs a notification through the BLE source slot that owns this link.
    pub fn accept_notification_from<S: UsbReportSink>(
        &self,
        source: SourceId,
        value_handle: u16,
        payload: &[u8],
        pipeline: &mut ReportPipeline,
        usb: &mut S,
    ) -> Result<BridgeOutput, HogpError<S::Error>> {
        if self.state != CentralState::Subscribed {
            return Err(HogpError::InvalidState);
        }
        let subscription = self.subscription.ok_or(HogpError::InvalidState)?;
        if value_handle != subscription.value_handle {
            return Err(HogpError::UnexpectedHandle(value_handle));
        }

        // HOGP identifies the report through the Report characteristic and its
        // Report Reference descriptor. The characteristic value itself does
        // not carry the USB Report ID prefix.
        if payload.len() != 8 {
            return Err(HogpError::InvalidReportLength(payload.len()));
        }
        pipeline
            .accept_ble_report_from(
                source,
                payload
                    .try_into()
                    .map_err(|_| HogpError::InvalidReportLength(payload.len()))?,
                usb,
            )
            .map_err(HogpError::Pipeline)
    }

    /// Resets discovery state after link loss. The caller should also invoke
    /// [`ReportPipeline::disconnect`] so the USB host receives a release.
    pub fn disconnected(&mut self) {
        self.subscription = None;
        self.state = CentralState::Idle;
    }
}

/// Recognizes the standard short-item sequence declaring a Generic Desktop
/// Keyboard Application Collection. It intentionally does not interpret the
/// rest of an untrusted report descriptor.
fn report_map_has_keyboard_application(map: &[u8]) -> bool {
    report_map_keyboard_report_id(map, 0)
        || (1..=u8::MAX).any(|report_id| report_map_keyboard_report_id(map, report_id))
}

fn report_map_keyboard_report_id(mut map: &[u8], wanted_report_id: u8) -> bool {
    const MAIN: u8 = 0;
    const GLOBAL: u8 = 1;
    const LOCAL: u8 = 2;
    const INPUT_TAG: u8 = 8;
    const COLLECTION_TAG: u8 = 10;
    const END_COLLECTION_TAG: u8 = 12;
    const USAGE_PAGE_TAG: u8 = 0;
    const REPORT_ID_TAG: u8 = 8;
    const USAGE_TAG: u8 = 0;
    const APPLICATION_COLLECTION: u32 = 1;
    const GENERIC_DESKTOP_PAGE: u32 = 1;
    const KEYBOARD_USAGE: u32 = 6;

    let mut usage_page = 0;
    let mut local_usage = None;
    let mut report_id = 0;
    let mut depth = 0u8;
    let mut keyboard_depth = None;

    while let Some((&prefix, rest)) = map.split_first() {
        if prefix == 0xfe {
            if rest.len() < 2 {
                return false;
            }
            let length = usize::from(rest[0]);
            if rest.len() < length + 2 {
                return false;
            }
            map = &rest[length + 2..];
            continue;
        }

        let size = match prefix & 0x03 {
            3 => 4,
            size => usize::from(size),
        };
        if rest.len() < size {
            return false;
        }
        let data = rest[..size]
            .iter()
            .enumerate()
            .fold(0u32, |value, (index, byte)| {
                value | (u32::from(*byte) << (index * 8))
            });
        map = &rest[size..];

        let item_type = (prefix >> 2) & 0x03;
        let tag = prefix >> 4;
        match (item_type, tag) {
            (GLOBAL, USAGE_PAGE_TAG) => usage_page = data,
            (GLOBAL, REPORT_ID_TAG) => report_id = data as u8,
            (LOCAL, USAGE_TAG) => local_usage = Some(data),
            (MAIN, COLLECTION_TAG) => {
                depth = depth.saturating_add(1);
                if data == APPLICATION_COLLECTION
                    && usage_page == GENERIC_DESKTOP_PAGE
                    && local_usage == Some(KEYBOARD_USAGE)
                {
                    keyboard_depth = Some(depth);
                }
                local_usage = None;
            }
            (MAIN, INPUT_TAG) => {
                if keyboard_depth.is_some() && report_id == wanted_report_id {
                    return true;
                }
                local_usage = None;
            }
            (MAIN, END_COLLECTION_TAG) => {
                if keyboard_depth == Some(depth) {
                    keyboard_depth = None;
                }
                depth = depth.saturating_sub(1);
                local_usage = None;
            }
            (MAIN, _) => local_usage = None,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::vec::Vec;
    use ukf_core::BridgeProfile;

    const KEYBOARD_REPORT_MAP: &[u8] = &[
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x06, // Usage (Keyboard)
        0xa1, 0x01, // Collection (Application)
        0x85, 0x01, // Report ID 1
        0x81, 0x02, // Input (Data, Variable, Absolute)
        0xc0, // End Collection
    ];

    #[derive(Default)]
    struct Sink(Vec<[u8; 8]>);

    impl UsbReportSink for Sink {
        type Error = ();

        fn write(&mut self, report: [u8; 8]) -> Result<(), Self::Error> {
            self.0.push(report);
            Ok(())
        }
    }

    fn subscribed_central(report_id: u8) -> HogpCentral {
        let mut central = HogpCentral::new();
        central.start_scan().unwrap();
        central.connected().unwrap();
        central
            .finish_discovery(
                KEYBOARD_REPORT_MAP,
                &[
                    ReportCharacteristic {
                        value_handle: 10,
                        notify: true,
                        report_id: 2,
                        report_type: 2,
                    },
                    ReportCharacteristic {
                        value_handle: 20,
                        notify: true,
                        report_id,
                        report_type: 1,
                    },
                ],
            )
            .unwrap();
        central
    }

    #[test]
    fn scan_matches_only_advertised_hid_service() {
        let mut central = HogpCentral::new();
        assert_eq!(central.start_scan(), Ok(HID_SERVICE_UUID));
        assert!(!central.advertisement_is_hid(&[0x180f, 0x180a]));
        assert!(central.advertisement_is_hid(&[0x180f, HID_SERVICE_UUID]));
    }

    #[test]
    fn discovery_selects_notifiable_input_report() {
        let central = subscribed_central(1);
        assert_eq!(central.state(), CentralState::Subscribed);
        assert_eq!(
            central.subscription,
            Some(InputSubscription {
                value_handle: 20,
                report_id: 1
            })
        );
    }

    #[test]
    fn report_reference_id_is_not_prefixed_to_notification_value() {
        let central = subscribed_central(1);
        let mut pipeline = ReportPipeline::new(BridgeProfile::US_JIS_PRESET).unwrap();
        let mut sink = Sink::default();

        central
            .accept_notification(
                20,
                &[0x02, 0, 0x1f, 0, 0, 0, 0, 0],
                &mut pipeline,
                &mut sink,
            )
            .unwrap();

        assert_eq!(sink.0, [[0, 0, 0x2f, 0, 0, 0, 0, 0]]);
    }

    #[test]
    fn composite_hid_selects_keyboard_report_id() {
        const COMPOSITE_REPORT_MAP: &[u8] = &[
            0x05, 0x0c, // Usage Page (Consumer)
            0x09, 0x01, // Usage (Consumer Control)
            0xa1, 0x01, // Collection (Application)
            0x85, 0x01, // Report ID 1
            0x81, 0x02, // Input
            0xc0, // End Collection
            0x05, 0x01, // Usage Page (Generic Desktop)
            0x09, 0x06, // Usage (Keyboard)
            0xa1, 0x01, // Collection (Application)
            0x85, 0x02, // Report ID 2
            0x81, 0x02, // Input
            0xc0, // End Collection
        ];
        let mut central = HogpCentral::new();
        central.start_scan().unwrap();
        central.connected().unwrap();
        let subscription = central
            .finish_discovery(
                COMPOSITE_REPORT_MAP,
                &[
                    ReportCharacteristic {
                        value_handle: 10,
                        notify: true,
                        report_id: 1,
                        report_type: 1,
                    },
                    ReportCharacteristic {
                        value_handle: 20,
                        notify: true,
                        report_id: 2,
                        report_type: 1,
                    },
                ],
            )
            .unwrap();

        assert_eq!(subscription.value_handle, 20);
        assert_eq!(subscription.report_id, 2);
    }

    #[test]
    fn invalid_notification_length_never_reaches_usb() {
        let central = subscribed_central(1);
        let mut pipeline = ReportPipeline::new(BridgeProfile::NONE).unwrap();
        let mut sink = Sink::default();

        let result = central.accept_notification(20, &[0, 0, 0x04], &mut pipeline, &mut sink);

        assert_eq!(result, Err(HogpError::InvalidReportLength(3)));
        assert!(sink.0.is_empty());
    }
}
