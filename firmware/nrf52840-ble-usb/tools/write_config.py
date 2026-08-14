# /// script
# requires-python = ">=3.11"
# dependencies = ["hidapi"]
# ///
"""Send an arbitrary byte sequence through the fixed-capacity transfer path.

The payload is limited to the firmware's 512-byte scratch area. The tool sends
BEGIN, sequential 23-byte-or-smaller CHUNK packets, and COMMIT, then reads the
transfer diagnostic block so a rejected operation is visible to the operator.

Examples::

    uv run write_config.py profile.bin
    uv run write_config.py --hex "00 01 02 03"
"""

from __future__ import annotations

import argparse
import struct
import zlib
from pathlib import Path

from read_diagnostics import DiagnosticsError, decode_transfer
from reset_xiao import (
    CONFIG_PROTOCOL_VERSION,
    CONFIG_REPORT_ID,
    CONFIG_REPORT_LEN,
    find_bridge,
)

UKF_SELECT_PANIC_CHUNK = 101
UKF_WRITE_BEGIN = 104
UKF_WRITE_CHUNK = 105
UKF_WRITE_COMMIT = 106
TARGET_SCRATCH = 1
TARGET_PROFILE = 2
TARGET_SOURCE_PROFILE = 3
TARGET_KEYMAP = 5
TRANSFER_CAPACITY = 512
SELECT_TRANSFER = 0xFC


def build_packet(command: int, fields: bytes = b"") -> bytes:
    """Build a report-ID-prefixed configuration packet."""

    if len(fields) > CONFIG_REPORT_LEN - 6:
        raise ValueError("configuration command fields exceed the 26-byte data area")
    payload = bytearray(CONFIG_REPORT_LEN)
    payload[0] = CONFIG_PROTOCOL_VERSION
    payload[1] = command
    payload[2 : 2 + len(fields)] = fields
    payload[28:] = struct.pack("<I", zlib.crc32(payload[:28]) & 0xFFFFFFFF)
    return bytes([CONFIG_REPORT_ID]) + bytes(payload)


def build_begin_packet(target: int, total_len: int, payload_crc: int) -> bytes:
    """Build a BEGIN packet using the protocol's fixed offsets."""

    if not 0 <= total_len <= 0xFFFF:
        raise ValueError("total_len must fit in u16")
    fields = bytearray(26)
    fields[0] = target & 0xFF
    struct.pack_into("<H", fields, 2, total_len)
    struct.pack_into("<I", fields, 4, payload_crc & 0xFFFFFFFF)
    return build_packet(UKF_WRITE_BEGIN, fields)


def build_chunk_packet(index: int, data: bytes) -> bytes:
    """Build one CHUNK packet with one to twenty-three data bytes."""

    if not 0 <= index <= 0xFFFF:
        raise ValueError("chunk index must fit in u16")
    if not 1 <= len(data) <= 23:
        raise ValueError("chunk data must contain between 1 and 23 bytes")
    fields = bytearray(26)
    struct.pack_into("<H", fields, 0, index)
    fields[2] = len(data)
    fields[3 : 3 + len(data)] = data
    return build_packet(UKF_WRITE_CHUNK, fields)


def build_commit_packet(target: int, total_len: int, payload_crc: int) -> bytes:
    """Build a COMMIT packet using the protocol's fixed offsets."""

    if not 0 <= total_len <= 0xFFFF:
        raise ValueError("total_len must fit in u16")
    fields = bytearray(26)
    fields[0] = target & 0xFF
    struct.pack_into("<H", fields, 2, total_len)
    struct.pack_into("<I", fields, 4, payload_crc & 0xFFFFFFFF)
    return build_packet(UKF_WRITE_COMMIT, fields)


def build_select_transfer_packet() -> bytes:
    """Build the selector request for the transfer diagnostic block."""

    return build_packet(UKF_SELECT_PANIC_CHUNK, bytes([SELECT_TRANSFER]))


def ensure_report_sent(device, packet: bytes) -> None:
    """Fail closed when hidapi does not accept the complete feature report."""

    result = device.send_feature_report(packet)
    expected = CONFIG_REPORT_LEN + 1
    if result != expected:
        raise RuntimeError(
            f"hidapi send_feature_report returned {result}; expected {expected}"
        )


def read_transfer(device) -> tuple[bytes, dict[str, object]]:
    """Read and decode one transfer diagnostic block."""

    raw = bytes(device.get_feature_report(CONFIG_REPORT_ID, CONFIG_REPORT_LEN + 1))
    last_error: DiagnosticsError | None = None
    for candidate in (raw[:CONFIG_REPORT_LEN], raw[1 : 1 + CONFIG_REPORT_LEN]):
        if len(candidate) != CONFIG_REPORT_LEN:
            continue
        try:
            return candidate, decode_transfer(candidate)
        except DiagnosticsError as error:
            last_error = error
    raise last_error or DiagnosticsError("no framing produced a valid transfer block")


def open_bridge():
    """Open the one vendor configuration HID interface."""

    try:
        import hid  # type: ignore[import-not-found]
    except ImportError as error:
        raise RuntimeError("run this with: uv run write_config.py") from error

    device_info = find_bridge(hid.enumerate())
    device_type = getattr(hid, "Device", None)
    if device_type is not None:
        return device_type(path=device_info["path"])
    device = hid.device()
    device.open_path(device_info["path"])
    return device


def send_payload(
    device, payload: bytes, *, target: int = TARGET_SCRATCH
) -> tuple[bytes, dict[str, object]]:
    """Send one payload to a target and return the final transfer block."""

    if len(payload) > TRANSFER_CAPACITY:
        raise ValueError(f"payload is {len(payload)} bytes; maximum is {TRANSFER_CAPACITY}")
    payload_crc = zlib.crc32(payload) & 0xFFFFFFFF
    ensure_report_sent(device, build_begin_packet(target, len(payload), payload_crc))
    for index, start in enumerate(range(0, len(payload), 23)):
        ensure_report_sent(device, build_chunk_packet(index, payload[start : start + 23]))
    ensure_report_sent(device, build_commit_packet(target, len(payload), payload_crc))

    ensure_report_sent(device, build_select_transfer_packet())
    block, decoded = read_transfer(device)
    print(decoded["設定転送"])
    if block[2] != 2 or block[4] != target:
        raise RuntimeError(f"configuration transfer was not committed: {decoded['設定転送']}")
    return block, decoded


def load_payload(path: Path | None, hex_data: str | None) -> bytes:
    """Load bytes from a file or a hexadecimal command-line value."""

    if path is not None and hex_data is not None:
        raise ValueError("choose a file path or --hex, not both")
    if path is not None:
        return path.read_bytes()
    if hex_data is not None:
        try:
            return bytes.fromhex(hex_data)
        except ValueError as error:
            raise ValueError("--hex must contain hexadecimal bytes") from error
    raise ValueError("provide a file path or --hex")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", nargs="?", type=Path, help="file containing the payload bytes")
    parser.add_argument("--hex", dest="hex_data", help="payload bytes written as hexadecimal")
    args = parser.parse_args()

    payload = load_payload(args.path, args.hex_data)
    device = open_bridge()
    try:
        send_payload(device, payload)
    finally:
        device.close()


if __name__ == "__main__":
    main()
