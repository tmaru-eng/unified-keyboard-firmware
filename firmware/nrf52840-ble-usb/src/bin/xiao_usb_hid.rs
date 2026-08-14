#![no_std]
#![no_main]

//! Seeed XIAO nRF52840 Sense USB HID milestone.
//!
//! This binary deliberately leaves the factory S140 SoftDevice resident but
//! disabled, so the application owns POWER, CLOCK, and USBD directly and can
//! use the interrupt-driven `embassy-nrf` USB driver. The milestone it proves
//! is the host-visible boundary only: a boot keyboard interface plus the
//! factory-compatible configuration interface and its UF2 software reset.
//!
//! The BLE central is intentionally absent. Reintroducing it means swapping
//! [`HardwareVbusDetect`] for `SoftwareVbusDetect` fed by SoftDevice SoC power
//! events, not rewriting this USB boundary.

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

use cortex_m::peripheral::SCB;
use embassy_executor::Spawner;
use embassy_futures::join::join3;
use embassy_nrf::config::{Config as NrfConfig, HfclkSource};
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::{bind_interrupts, pac, peripherals, usb};
use embassy_time::{Duration, Timer};
use embassy_usb::class::hid::{
    Config as HidConfig, HidBootProtocol, HidSubclass, HidWriter, ReportId, RequestHandler, State,
};
use embassy_usb::control::OutResponse;
use embassy_usb::{Builder, Config};
use ukf_nrf52840_ble_usb::config_hid::{
    CONFIG_CONTROL_BUFFER_LEN, CONFIG_REPORT_DESCRIPTOR, ConfigRequest, classify_feature_report,
};
use ukf_nrf52840_ble_usb::uf2_reset::{CONFIG_REPORT_LEN, UF2_RESET_MAGIC};
use usbd_hid::descriptor::{KeyboardReport, SerializedDescriptor};

/// Approved application identity for the independent bridge prototype.
const USB_VENDOR_ID: u16 = 0x1209;
/// Approved application product identity for the independent bridge prototype.
const USB_PRODUCT_ID: u16 = 0x0001;

/// Grace period letting the reset request's status stage reach the host.
///
/// Resetting inside the control handler would drop the device off the bus
/// before the host sees the transfer complete, which the legacy sender reports
/// as a failure even though the board did reboot.
const RESET_GRACE: Duration = Duration::from_millis(50);
/// Interval at which the reset request flag is observed.
const RESET_POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Set by the configuration interface once a validated reset command arrives.
static RESET_REQUESTED: AtomicBool = AtomicBool::new(false);

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
});

/// Requests the UF2 bootloader on the next boot and resets the MCU.
///
/// The SoftDevice is not enabled in this binary, so GPREGRET is written
/// directly instead of through the S140 SVC ABI.
fn reset_into_uf2_bootloader() -> ! {
    pac::POWER
        .gpregret()
        .write(|w| w.set_gpregret(UF2_RESET_MAGIC));
    SCB::sys_reset()
}

/// Returns to the UF2 bootloader instead of halting on an unrecoverable fault.
///
/// A silent halt is indistinguishable from a board that never started, and it
/// requires a physical double-reset to recover. Returning to UF2 keeps every
/// failure reflashable over USB.
#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    reset_into_uf2_bootloader()
}

/// Validates configuration feature reports and records a reset request.
struct ConfigRequestHandler;

impl RequestHandler for ConfigRequestHandler {
    fn set_report(&mut self, id: ReportId, data: &[u8]) -> OutResponse {
        let ReportId::Feature(report_id) = id else {
            return OutResponse::Rejected;
        };

        match classify_feature_report(report_id, data) {
            ConfigRequest::ResetIntoBootloader => {
                RESET_REQUESTED.store(true, Ordering::Release);
                OutResponse::Accepted
            }
            ConfigRequest::InjectSourceReport(_) => OutResponse::Rejected,
            // This image has no radio, so everything about pairing and about
            // the BLE probe's recorded state is refused rather than silently
            // accepted. A host that gets an acknowledgement here would believe
            // it had opened pairing on a board that cannot pair.
            ConfigRequest::SelectPanicChunk(_)
            | ConfigRequest::SelectSource(_)
            | ConfigRequest::SelectKeymapChunk(_)
            | ConfigRequest::SetPairingMode(_)
            | ConfigRequest::SetPairingMethod { .. }
            | ConfigRequest::WriteBegin { .. }
            | ConfigRequest::WriteChunk { .. }
            | ConfigRequest::WriteCommit { .. } => OutResponse::Rejected,
            ConfigRequest::Rejected(_) => OutResponse::Rejected,
        }
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let mut nrf_config = NrfConfig::default();
    // USBD is only specified against the external 32 MHz crystal.
    nrf_config.hfclk_source = HfclkSource::ExternalXtal;
    let peripherals = embassy_nrf::init(nrf_config);

    // Both XIAO Sense user LEDs are active low, so `Level::High` is off.
    let mut red_led = Output::new(peripherals.P0_26, Level::High, OutputDrive::Standard);
    let mut blue_led = Output::new(peripherals.P0_06, Level::High, OutputDrive::Standard);
    red_led.set_low();

    let driver = Driver::new(peripherals.USBD, Irqs, HardwareVbusDetect::new(Irqs));

    let mut config = Config::new(USB_VENDOR_ID, USB_PRODUCT_ID);
    config.manufacturer = Some("Unified Keyboard Firmware");
    config.product = Some("XIAO USB HID milestone");
    config.serial_number = Some("USB-HID-MILESTONE");
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

    // The keyboard endpoint only becomes ready once the host has selected a
    // configuration, so this is the first point at which HID enumeration is
    // proven rather than assumed.
    let enumerated = async {
        keyboard.ready().await;
        red_led.set_high();
        blue_led.set_low();
        core::future::pending::<()>().await;
    };

    let reset_task = async {
        loop {
            if RESET_REQUESTED.load(Ordering::Acquire) {
                Timer::after(RESET_GRACE).await;
                reset_into_uf2_bootloader();
            }
            Timer::after(RESET_POLL_INTERVAL).await;
        }
    };

    join3(usb_task, enumerated, reset_task).await;
}
