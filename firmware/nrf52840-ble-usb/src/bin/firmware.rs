#![no_std]
#![no_main]
#![forbid(unsafe_code)]

//! Compile-only generic nRF52840 USB HID firmware shell.

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_nrf::usb::Driver;
use embassy_nrf::usb::vbus_detect::HardwareVbusDetect;
use embassy_nrf::{bind_interrupts, peripherals, usb};
use embassy_usb::class::hid::{
    Config as HidConfig, HidBootProtocol, HidSubclass, HidWriter, State,
};
use embassy_usb::{Builder, Config};
use panic_halt as _;
use usbd_hid::descriptor::{KeyboardReport, SerializedDescriptor};

bind_interrupts!(struct Irqs {
    USBD => usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => usb::vbus_detect::InterruptHandler;
});

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let peripherals = embassy_nrf::init(Default::default());
    let driver = Driver::new(peripherals.USBD, Irqs, HardwareVbusDetect::new(Irqs));

    let mut config = Config::new(0x1209, 0x0001);
    config.manufacturer = Some("Unified Keyboard Firmware");
    config.product = Some("nRF52840 bridge prototype");
    config.serial_number = Some("COMPILE-ONLY");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    let mut config_descriptor = [0; 256];
    let mut bos_descriptor = [0; 256];
    let mut msos_descriptor = [0; 256];
    let mut control_buffer = [0; 64];
    let mut hid_state = State::new();
    let mut builder = Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut msos_descriptor,
        &mut control_buffer,
    );
    let hid_config = HidConfig {
        report_descriptor: KeyboardReport::desc(),
        request_handler: None,
        poll_ms: 8,
        max_packet_size: 8,
        hid_subclass: HidSubclass::Boot,
        hid_boot_protocol: HidBootProtocol::Keyboard,
    };
    let mut writer = HidWriter::<_, 8>::new(&mut builder, &mut hid_state, hid_config);
    let mut usb = builder.build();

    let usb_device = usb.run();
    let report_output = async {
        writer.ready().await;
        let _ = writer.write(&[0; 8]).await;

        // BLE central integration boundary: a future Trouble + nrf-sdc task
        // will feed reports through `ReportPipeline`, then await this writer.
        core::future::pending::<()>().await;
    };
    join(usb_device, report_output).await;
}
