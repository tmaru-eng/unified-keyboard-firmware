"""Send the factory-compatible software reset command to one XIAO bridge.

The selector accepts either the factory legacy identity ``0xCAFE:0xBAF2`` or
the new bridge identity ``0x1209:0x0001``. In both cases it requires the exact
vendor configuration HID usage ``0xFF00/0x20`` and rejects ambiguity.

Install the optional hidapi binding before using the hardware path::

    py -m pip install hidapi
    py reset_xiao.py

The packet builder is dependency-free so its wire contract can be tested on
the host without a board or a HID driver.
"""

from __future__ import annotations

import argparse
import struct
import zlib

CONFIG_REPORT_ID = 100
CONFIG_REPORT_LEN = 32
CONFIG_PROTOCOL_VERSION = 18
RESET_INTO_BOOTSEL = 1
LEGACY_VENDOR_ID = 0xCAFE
LEGACY_PRODUCT_ID = 0xBAF2
NEW_VENDOR_ID = 0x1209
NEW_PRODUCT_ID = 0x0001
# Keep the old names as source-compatible aliases for existing host scripts.
VENDOR_ID = LEGACY_VENDOR_ID
PRODUCT_ID = LEGACY_PRODUCT_ID
SUPPORTED_IDENTITIES = frozenset(
    {
        (LEGACY_VENDOR_ID, LEGACY_PRODUCT_ID),
        (NEW_VENDOR_ID, NEW_PRODUCT_ID),
    }
)
CONFIG_USAGE_PAGE = 0xFF00
CONFIG_USAGE = 0x20


def build_reset_report() -> bytes:
    """Return hidapi's report-ID-prefixed 33-byte reset packet."""

    payload = bytearray(CONFIG_REPORT_LEN)
    payload[0] = CONFIG_PROTOCOL_VERSION
    payload[1] = RESET_INTO_BOOTSEL
    crc = zlib.crc32(payload[:28]) & 0xFFFFFFFF
    payload[28:] = struct.pack("<I", crc)
    return bytes([CONFIG_REPORT_ID]) + bytes(payload)


def find_bridge(devices: list[dict]) -> dict:
    """Select exactly one vendor config HID interface from hidapi enumeration.

    Both the factory-compatible legacy identity and the new-repository
    identity are accepted, but the usage page/usage contract is always exact.
    A mixed or duplicate enumeration remains ambiguous and is rejected.
    """

    matches = [
        device
        for device in devices
        if (device.get("vendor_id"), device.get("product_id"))
        in SUPPORTED_IDENTITIES
        and device.get("usage_page") == CONFIG_USAGE_PAGE
        and device.get("usage") == CONFIG_USAGE
    ]
    if len(matches) != 1:
        raise RuntimeError(
            f"expected one XIAO config HID interface, found {len(matches)}"
        )
    return matches[0]


def ensure_report_sent(result: int) -> None:
    """Fail closed when hidapi did not accept the complete reset report."""

    expected = CONFIG_REPORT_LEN + 1
    if result != expected:
        raise RuntimeError(
            f"hidapi send_feature_report returned {result}; expected {expected}"
        )


def send_reset() -> None:
    """Enumerate, open, and send the reset packet through hidapi."""

    try:
        import hid  # type: ignore[import-not-found]
    except ImportError as error:
        raise RuntimeError("install hidapi first: py -m pip install hidapi") from error

    device_info = find_bridge(hid.enumerate())
    # hidapi exposes the constructor as either ``Device(path=...)`` or the
    # lower-case ``device().open_path(...)`` depending on the wheel version.
    device_type = getattr(hid, "Device", None)
    if device_type is not None:
        device = device_type(path=device_info["path"])
    else:
        device = hid.device()
        device.open_path(device_info["path"])
    try:
        ensure_report_sent(device.send_feature_report(build_reset_report()))
    finally:
        device.close()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--print-packet",
        action="store_true",
        help="print the 33-byte hidapi packet without opening a device",
    )
    args = parser.parse_args()
    if args.print_packet:
        print(build_reset_report().hex(" "))
        return
    send_reset()
    print("Software reset requested; wait for the XIAO UF2 volume to appear.")


if __name__ == "__main__":
    main()
