//! Hardware-free walkthrough of the bridge core.

use std::io::{self, Write};

use ukf_core::{BootKeyboardReport, BridgeEngine, BridgeProfile, InputTransport, SourceId};

const USB: SourceId = SourceId(0);
const BLE: SourceId = SourceId(1);

fn report(modifiers: u8, key: u8) -> BootKeyboardReport {
    BootKeyboardReport {
        modifiers,
        keys: [key, 0, 0, 0, 0, 0],
    }
}

fn print_report(
    output: &mut impl Write,
    label: &str,
    report: BootKeyboardReport,
) -> io::Result<()> {
    writeln!(output, "{label}: {:02x?}", report.to_bytes())
}

/// Writes a deterministic bridge lifecycle to `output` without using hardware.
pub fn write_demo(mut output: impl Write) -> io::Result<()> {
    let mut bridge = BridgeEngine::new();
    bridge
        .attach(USB, InputTransport::Usb, BridgeProfile::NONE)
        .expect("source 0 is within bridge capacity");
    bridge
        .attach(
            BLE,
            InputTransport::Ble,
            BridgeProfile {
                us_to_jis: true,
                ..BridgeProfile::NONE
            },
        )
        .expect("source 1 is within bridge capacity");
    writeln!(output, "attached: usb=source-0 ble=source-1")?;

    // On an ANSI US keyboard, Shift+2 is @. The JIS output is usage 0x2f
    // without Shift, serialized as a standard eight-byte boot report.
    let converted = bridge
        .submit_boot_report(BLE, report(0b0000_0010, 0x1f))
        .expect("the BLE source is attached");
    print_report(&mut output, "ble-us-jis-at", converted.report)?;

    // Release @, then demonstrate one held key from each transport.
    bridge
        .submit_boot_report(BLE, BootKeyboardReport::EMPTY)
        .expect("the BLE source is attached");
    let usb_only = bridge
        .submit_boot_report(USB, report(0, 0x04))
        .expect("the USB source is attached");
    print_report(&mut output, "usb-a", usb_only.report)?;

    let merged = bridge
        .submit_boot_report(BLE, report(0, 0x05))
        .expect("the BLE source is attached");
    print_report(&mut output, "usb-a+ble-b", merged.report)?;

    let ble_only = bridge.detach(USB).expect("the USB source is attached");
    print_report(&mut output, "detach-usb-keeps-ble-b", ble_only.report)
}

#[allow(dead_code)] // This file is also included by the integration test.
fn main() -> io::Result<()> {
    write_demo(io::stdout().lock())
}
