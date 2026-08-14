//! Compile-only adapter boundary for Nordic S140 via `nrf-softdevice`.
//!
//! Advertisement parsing stays target-independent so malformed AD structures
//! are host-tested. The concrete GATT client is only built for Cortex-M
//! firmware with the `s140-central` feature.

use crate::hogp::HID_SERVICE_UUID;

const INCOMPLETE_16_BIT_SERVICE_UUIDS: u8 = 0x02;
const COMPLETE_16_BIT_SERVICE_UUIDS: u8 = 0x03;

/// Returns whether well-formed BLE advertising data declares the HID service.
pub fn advertisement_has_hid_service(mut payload: &[u8]) -> bool {
    while let Some((&length, rest)) = payload.split_first() {
        let length = usize::from(length);
        if length == 0 {
            return false;
        }
        if rest.len() < length {
            return false;
        }

        let (element, remaining) = rest.split_at(length);
        let (data_type, value) = match element.split_first() {
            Some(parts) => parts,
            None => return false,
        };
        if matches!(
            *data_type,
            INCOMPLETE_16_BIT_SERVICE_UUIDS | COMPLETE_16_BIT_SERVICE_UUIDS
        ) && value
            .chunks_exact(2)
            .any(|uuid| u16::from_le_bytes([uuid[0], uuid[1]]) == HID_SERVICE_UUID)
        {
            return true;
        }
        payload = remaining;
    }
    false
}

/// S140 types and discovery operations available to the ARM firmware build.
#[cfg(all(feature = "s140-central", target_arch = "arm", target_os = "none"))]
pub mod adapter {
    use core::convert::Infallible;

    use nrf_softdevice::ble::gatt_client::{
        self, Characteristic, Client, Descriptor, DiscoverError, HvxType, ReadError, WriteError,
    };
    use nrf_softdevice::ble::{Address, Connection, Uuid, central};
    use nrf_softdevice::{Config, raw};

    use crate::hogp::{
        HID_SERVICE_UUID, HogpCentral, HogpError, InputSubscription, REPORT_CHARACTERISTIC_UUID,
        REPORT_MAP_CHARACTERISTIC_UUID, REPORT_REFERENCE_DESCRIPTOR_UUID, ReportCharacteristic,
    };
    use crate::{ReportPipeline, UsbReportSink};
    use ukf_core::BridgeOutput;

    const CLIENT_CONFIGURATION_DESCRIPTOR_UUID: u16 = 0x2902;
    const MAX_REPORT_CHARACTERISTICS: usize = 8;
    const MAX_NOTIFICATION_LENGTH: usize = 9;

    #[derive(Clone, Copy, Debug)]
    struct DiscoveredReport {
        value_handle: u16,
        cccd_handle: u16,
        report_reference_handle: u16,
        notify: bool,
    }

    /// One notification copied out of the SoftDevice event buffer.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct Notification {
        value_handle: u16,
        len: u8,
        payload: [u8; MAX_NOTIFICATION_LENGTH],
    }

    impl Notification {
        /// ATT value handle that emitted the notification.
        pub const fn value_handle(&self) -> u16 {
            self.value_handle
        }

        /// Notification value, limited to a boot report and optional report ID.
        pub fn payload(&self) -> &[u8] {
            &self.payload[..usize::from(self.len)]
        }

        /// Feeds this copied SoftDevice event into the existing HOGP boundary.
        pub fn forward<S: UsbReportSink>(
            &self,
            central: &HogpCentral,
            pipeline: &mut ReportPipeline,
            usb: &mut S,
        ) -> Result<BridgeOutput, HogpError<S::Error>> {
            central.accept_notification(self.value_handle(), self.payload(), pipeline, usb)
        }
    }

    /// Error while converting S140 discovery into a HOGP subscription.
    #[derive(Debug)]
    pub enum AdapterError {
        /// `HogpCentral` rejected the lifecycle or discovered HID metadata.
        Hogp(HogpError<Infallible>),
        /// S140 GATT service discovery failed.
        Discover(DiscoverError),
        /// Reading the Report Map or a Report Reference failed.
        Read(ReadError),
        /// A Report Reference was not the required two-byte value.
        InvalidReportReferenceLength(usize),
        /// Enabling notifications through the CCCD failed.
        Write(WriteError),
        /// Discovery selected a report without a matching CCCD.
        MissingClientConfiguration,
    }

    /// Dynamic HID-service client used because HOGP may repeat Report fields.
    pub struct HidClient {
        connection: Connection,
        report_map_handle: Option<u16>,
        reports: [Option<DiscoveredReport>; MAX_REPORT_CHARACTERISTICS],
    }

    impl HidClient {
        fn report_map_handle(&self) -> Result<u16, DiscoverError> {
            self.report_map_handle
                .ok_or(DiscoverError::ServiceIncomplete)
        }

        fn push_report(&mut self, report: DiscoveredReport) {
            if let Some(slot) = self.reports.iter_mut().find(|slot| slot.is_none()) {
                *slot = Some(report);
            }
        }
    }

    impl Client for HidClient {
        type Event = Notification;

        fn on_hvx(
            &self,
            _connection: &Connection,
            kind: HvxType,
            handle: u16,
            data: &[u8],
        ) -> Option<Self::Event> {
            if kind != HvxType::Notification || data.len() > MAX_NOTIFICATION_LENGTH {
                return None;
            }
            let mut payload = [0; MAX_NOTIFICATION_LENGTH];
            payload[..data.len()].copy_from_slice(data);
            Some(Notification {
                value_handle: handle,
                len: data.len() as u8,
                payload,
            })
        }

        fn uuid() -> Uuid {
            Uuid::new_16(HID_SERVICE_UUID)
        }

        fn new_undiscovered(connection: Connection) -> Self {
            Self {
                connection,
                report_map_handle: None,
                reports: [None; MAX_REPORT_CHARACTERISTICS],
            }
        }

        fn discovered_characteristic(
            &mut self,
            characteristic: &Characteristic,
            descriptors: &[Descriptor],
        ) {
            if characteristic.uuid == Some(Uuid::new_16(REPORT_MAP_CHARACTERISTIC_UUID)) {
                self.report_map_handle = Some(characteristic.handle_value);
                return;
            }
            if characteristic.uuid != Some(Uuid::new_16(REPORT_CHARACTERISTIC_UUID)) {
                return;
            }

            let cccd_handle = descriptors
                .iter()
                .find(|descriptor| {
                    descriptor.uuid == Some(Uuid::new_16(CLIENT_CONFIGURATION_DESCRIPTOR_UUID))
                })
                .map(|descriptor| descriptor.handle);
            let report_reference_handle = descriptors
                .iter()
                .find(|descriptor| {
                    descriptor.uuid == Some(Uuid::new_16(REPORT_REFERENCE_DESCRIPTOR_UUID))
                })
                .map(|descriptor| descriptor.handle);
            if let (Some(cccd_handle), Some(report_reference_handle)) =
                (cccd_handle, report_reference_handle)
            {
                self.push_report(DiscoveredReport {
                    value_handle: characteristic.handle_value,
                    cccd_handle,
                    report_reference_handle,
                    notify: characteristic.props.notify() != 0,
                });
            }
        }

        fn discovery_complete(&mut self) -> Result<(), DiscoverError> {
            self.report_map_handle()?;
            if self.reports.iter().all(Option::is_none) {
                return Err(DiscoverError::ServiceIncomplete);
            }
            Ok(())
        }
    }

    /// Conservative S140 configuration for one central connection.
    pub fn softdevice_config() -> Config {
        Config {
            clock: Some(raw::nrf_clock_lf_cfg_t {
                source: raw::NRF_CLOCK_LF_SRC_RC as u8,
                rc_ctiv: 16,
                rc_temp_ctiv: 2,
                accuracy: raw::NRF_CLOCK_LF_ACCURACY_500_PPM as u8,
            }),
            conn_gap: Some(raw::ble_gap_conn_cfg_t {
                conn_count: 1,
                event_length: 6,
            }),
            conn_gatt: Some(raw::ble_gatt_conn_cfg_t { att_mtu: 64 }),
            gap_role_count: Some(raw::ble_gap_cfg_role_count_t {
                adv_set_count: 0,
                periph_role_count: 0,
                central_role_count: 1,
                central_sec_count: 0,
                _bitfield_1: raw::ble_gap_cfg_role_count_t::new_bitfield_1(0),
            }),
            ..Default::default()
        }
    }

    /// S140 active-scan defaults; advertisement bytes must be validated first.
    pub fn scan_config() -> central::ScanConfig<'static> {
        central::ScanConfig::default()
    }

    /// Builds S140 connection parameters for one address selected by scanning.
    pub fn connect_config<'a>(addresses: &'a [&'a Address]) -> central::ConnectConfig<'a> {
        let mut config = central::ConnectConfig::default();
        config.scan_config.whitelist = Some(addresses);
        config
    }

    /// Discovers HID metadata, selects the input report through `HogpCentral`,
    /// and writes the selected report's CCCD.
    ///
    /// `nrf-softdevice` 0.1 performs one ATT read at offset zero. Consequently,
    /// `report_map` receives only the first response even when the complete HID
    /// Report Map is longer than the negotiated MTU. The first bridge milestone
    /// supports keyboards whose Keyboard Application Collection is present in
    /// that prefix; full long-read support remains a separate compatibility
    /// milestone.
    pub async fn discover_and_subscribe(
        central: &mut HogpCentral,
        connection: &Connection,
        report_map: &mut [u8],
    ) -> Result<(HidClient, InputSubscription), AdapterError> {
        central.connected().map_err(AdapterError::Hogp)?;
        let client: HidClient = gatt_client::discover(connection)
            .await
            .map_err(AdapterError::Discover)?;
        let report_map_len = gatt_client::read(
            &client.connection,
            client.report_map_handle().map_err(AdapterError::Discover)?,
            report_map,
        )
        .await
        .map_err(AdapterError::Read)?;

        let mut hogp_reports = [ReportCharacteristic {
            value_handle: 0,
            notify: false,
            report_id: 0,
            report_type: 0,
        }; MAX_REPORT_CHARACTERISTICS];
        let mut count = 0;
        for report in client.reports.iter().flatten() {
            let mut reference = [0; 2];
            let len = gatt_client::read(
                &client.connection,
                report.report_reference_handle,
                &mut reference,
            )
            .await
            .map_err(AdapterError::Read)?;
            if len != reference.len() {
                return Err(AdapterError::InvalidReportReferenceLength(len));
            }
            hogp_reports[count] = ReportCharacteristic {
                value_handle: report.value_handle,
                notify: report.notify,
                report_id: reference[0],
                report_type: reference[1],
            };
            count += 1;
        }

        let subscription = central
            .finish_discovery(&report_map[..report_map_len], &hogp_reports[..count])
            .map_err(AdapterError::Hogp)?;
        let cccd_handle = client
            .reports
            .iter()
            .flatten()
            .find(|report| report.value_handle == subscription.value_handle)
            .map(|report| report.cccd_handle)
            .ok_or(AdapterError::MissingClientConfiguration)?;
        gatt_client::write(&client.connection, cccd_handle, &[1, 0])
            .await
            .map_err(AdapterError::Write)?;
        Ok((client, subscription))
    }
}
