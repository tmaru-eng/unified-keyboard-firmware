# /// script
# requires-python = ">=3.11"
# dependencies = ["hidapi"]
# ///
"""Push one source keyboard report through the bridge and out over USB.

This exercises the conversion on real hardware without a BLE keyboard. The
firmware treats the payload as if a source keyboard had sent it, applies that
source's profile, and emits the converted report from its USB HID keyboard
interface.

**The converted keystroke goes to whatever window currently has focus.** Point
the focus somewhere harmless before running this.

Run it with uv so the hidapi dependency is declared rather than assumed::

    uv run inject_report.py --shift 0x1f      # Shift+2 on an ANSI US keyboard
    uv run inject_report.py --release         # all keys up

The packet builder is dependency-free so its wire contract can be tested on the
host without a board or a HID driver.
"""

from __future__ import annotations

import argparse
import struct
import zlib

from reset_xiao import CONFIG_REPORT_ID, CONFIG_REPORT_LEN, CONFIG_PROTOCOL_VERSION, find_bridge

# Extensions start at 100 because the factory firmware allocates 0..=50.
UKF_INJECT_SOURCE_REPORT = 100
INJECTED_REPORT_LEN = 8

MODIFIERS = {
    "ctrl": 0x01,
    "shift": 0x02,
    "alt": 0x04,
    "gui": 0x08,
}


def build_source_report(modifiers: int, usages: list[int]) -> bytes:
    """Return the eight-byte boot keyboard report a source would have sent."""

    if len(usages) > 6:
        raise ValueError("a boot keyboard report carries at most six keys")
    report = bytearray(INJECTED_REPORT_LEN)
    report[0] = modifiers & 0xFF
    # Byte 1 is reserved and always zero.
    for index, usage in enumerate(usages):
        report[2 + index] = usage & 0xFF
    return bytes(report)


def build_injection_packet(source_report: bytes) -> bytes:
    """Return hidapi's report-ID-prefixed 33-byte injection packet."""

    if len(source_report) != INJECTED_REPORT_LEN:
        raise ValueError(f"source report must be {INJECTED_REPORT_LEN} bytes")
    payload = bytearray(CONFIG_REPORT_LEN)
    payload[0] = CONFIG_PROTOCOL_VERSION
    payload[1] = UKF_INJECT_SOURCE_REPORT
    payload[2 : 2 + INJECTED_REPORT_LEN] = source_report
    crc = zlib.crc32(payload[:28]) & 0xFFFFFFFF
    payload[28:] = struct.pack("<I", crc)
    return bytes([CONFIG_REPORT_ID]) + bytes(payload)


def send(packet: bytes) -> None:
    """Enumerate, open the configuration interface, and send one packet."""

    try:
        import hid  # type: ignore[import-not-found]
    except ImportError as error:
        raise RuntimeError("run this with: uv run inject_report.py") from error

    device_info = find_bridge(hid.enumerate())
    device_type = getattr(hid, "Device", None)
    if device_type is not None:
        device = device_type(path=device_info["path"])
    else:
        device = hid.device()
        device.open_path(device_info["path"])
    try:
        result = device.send_feature_report(packet)
    finally:
        device.close()

    expected = CONFIG_REPORT_LEN + 1
    if result != expected:
        raise RuntimeError(
            f"hidapi send_feature_report returned {result}; expected {expected}"
        )


def parse_usage(text: str) -> int:
    """Parse a HID usage given as decimal or 0x-prefixed hex."""

    return int(text, 0)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "usages",
        nargs="*",
        type=parse_usage,
        help="HID usages to hold, e.g. 0x1f for the digit 2 on ANSI US",
    )
    for name in MODIFIERS:
        parser.add_argument(f"--{name}", action="store_true", help=f"hold {name}")
    parser.add_argument(
        "--release",
        action="store_true",
        help="send an all-zero report, releasing everything",
    )
    parser.add_argument(
        "--print-packet",
        action="store_true",
        help="print the 33-byte packet without opening a device",
    )
    args = parser.parse_args()

    if args.release:
        source_report = build_source_report(0, [])
    else:
        modifiers = sum(bit for name, bit in MODIFIERS.items() if getattr(args, name))
        source_report = build_source_report(modifiers, args.usages)

    packet = build_injection_packet(source_report)
    if args.print_packet:
        print(packet.hex(" "))
        return

    send(packet)
    print(f"injected source report {source_report.hex(' ')}")


if __name__ == "__main__":
    main()
