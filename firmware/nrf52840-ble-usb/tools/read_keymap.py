# /// script
# requires-python = ">=3.11"
# dependencies = ["hidapi"]
# ///
"""Read the active fixed-capacity keymap through the configuration HID path.

    uv run read_keymap.py

The board exposes one 32-byte diagnostics report. A keymap payload is selected
and read in 21-byte chunks, then validated before it is printed as JSON.
"""

from __future__ import annotations

import argparse
import json
import struct
import zlib

from reset_xiao import CONFIG_PROTOCOL_VERSION, CONFIG_REPORT_ID, CONFIG_REPORT_LEN
from write_config import ensure_report_sent, open_bridge, build_packet
from write_keymap import decode_keymap_payload, decode_source_keymap_payload

KEYMAP_KIND = 8
KEYMAP_REPORT_VERSION = 1
KEYMAP_CHUNK_DATA_LEN = 21
KEYMAP_CHUNK_COUNT_MAX = 7
KEYMAP_PAYLOAD_MAX_LEN = 130
UKF_SELECT_KEYMAP = 108
UKF_SELECT_SOURCE_KEYMAP = 109
SOURCE_SLOT_COUNT = 5
SOURCE_KEYMAP_REPORT_VERSION = 2
SOURCE_KEYMAP_CHUNK_DATA_LEN = 20
SOURCE_KEYMAP_CHUNK_COUNT_MAX = 7
SOURCE_KEYMAP_PAYLOAD_MAX_LEN = 3 + 32 * 4


def build_select_keymap_packet(chunk_index: int) -> bytes:
    """Build the selector packet for one keymap diagnostics chunk."""

    if not 0 <= chunk_index < KEYMAP_CHUNK_COUNT_MAX:
        raise ValueError(f"keymap chunk must be from 0 through {KEYMAP_CHUNK_COUNT_MAX - 1}")
    return build_packet(UKF_SELECT_KEYMAP, bytes([chunk_index]))


def build_select_source_keymap_packet(slot: int, chunk_index: int) -> bytes:
    """Build the selector packet for one source-slot keymap chunk."""

    if not 0 <= slot < SOURCE_SLOT_COUNT:
        raise ValueError(f"source slot must be from 0 through {SOURCE_SLOT_COUNT - 1}: {slot}")
    if not 0 <= chunk_index < SOURCE_KEYMAP_CHUNK_COUNT_MAX:
        raise ValueError(
            f"source keymap chunk must be from 0 through {SOURCE_KEYMAP_CHUNK_COUNT_MAX - 1}"
        )
    return build_packet(UKF_SELECT_SOURCE_KEYMAP, bytes([slot, chunk_index]))


def decode_keymap_chunk(block: bytes) -> dict[str, object]:
    """Decode one native 32-byte keymap diagnostics block."""

    if len(block) != CONFIG_REPORT_LEN:
        raise ValueError(f"keymap block must be {CONFIG_REPORT_LEN} bytes: {len(block)}")
    if block[0] != CONFIG_PROTOCOL_VERSION:
        raise ValueError(f"unsupported diagnostics protocol version {block[0]}")
    if block[1] != KEYMAP_KIND:
        raise ValueError(f"unexpected diagnostics kind {block[1]}; expected {KEYMAP_KIND}")
    if block[2] != KEYMAP_REPORT_VERSION:
        raise ValueError(f"unsupported keymap report version {block[2]}")
    expected_crc = struct.unpack_from("<I", block, 28)[0]
    actual_crc = zlib.crc32(block[:28]) & 0xFFFFFFFF
    if expected_crc != actual_crc:
        raise ValueError(
            f"keymap block CRC mismatch: expected 0x{expected_crc:08x}, actual 0x{actual_crc:08x}"
        )

    chunk_index = block[3]
    chunk_count = block[4]
    payload_length = struct.unpack_from("<H", block, 5)[0]
    if not 1 <= chunk_count <= KEYMAP_CHUNK_COUNT_MAX or chunk_index >= chunk_count:
        raise ValueError(f"invalid keymap chunk {chunk_index}/{chunk_count}")
    if not 2 <= payload_length <= KEYMAP_PAYLOAD_MAX_LEN:
        raise ValueError(f"invalid keymap payload length {payload_length}")
    expected_chunk_count = (payload_length + KEYMAP_CHUNK_DATA_LEN - 1) // KEYMAP_CHUNK_DATA_LEN
    if chunk_count != expected_chunk_count:
        raise ValueError(
            f"keymap chunk count {chunk_count} does not match payload length {payload_length}"
        )
    start = chunk_index * KEYMAP_CHUNK_DATA_LEN
    if start >= payload_length:
        raise ValueError(f"keymap chunk starts beyond payload: {start}/{payload_length}")
    meaningful = min(KEYMAP_CHUNK_DATA_LEN, payload_length - start)
    if any(block[7 + meaningful : 28]):
        raise ValueError("keymap chunk padding is not zero")
    return {
        "chunk_index": chunk_index,
        "chunk_count": chunk_count,
        "payload_length": payload_length,
        "data": bytes(block[7 : 7 + meaningful]),
    }


def decode_source_keymap_chunk(block: bytes) -> dict[str, object]:
    """Decode one version-2 source-slot keymap diagnostics block."""

    if len(block) != CONFIG_REPORT_LEN:
        raise ValueError(f"source keymap block must be {CONFIG_REPORT_LEN} bytes: {len(block)}")
    if block[0] != CONFIG_PROTOCOL_VERSION:
        raise ValueError(f"unsupported diagnostics protocol version {block[0]}")
    if block[1] != KEYMAP_KIND:
        raise ValueError(f"unexpected diagnostics kind {block[1]}; expected {KEYMAP_KIND}")
    if block[2] != SOURCE_KEYMAP_REPORT_VERSION:
        raise ValueError(f"unsupported source keymap report version {block[2]}")
    expected_crc = struct.unpack_from("<I", block, 28)[0]
    actual_crc = zlib.crc32(block[:28]) & 0xFFFFFFFF
    if expected_crc != actual_crc:
        raise ValueError(
            f"source keymap block CRC mismatch: expected 0x{expected_crc:08x}, actual 0x{actual_crc:08x}"
        )

    chunk_index = block[3]
    chunk_count = block[4]
    payload_length = struct.unpack_from("<H", block, 5)[0]
    slot = block[7]
    if not 0 <= slot < SOURCE_SLOT_COUNT:
        raise ValueError(f"unknown source keymap slot {slot}")
    if not 1 <= chunk_count <= SOURCE_KEYMAP_CHUNK_COUNT_MAX or chunk_index >= chunk_count:
        raise ValueError(f"invalid source keymap chunk {chunk_index}/{chunk_count}")
    if not 3 <= payload_length <= SOURCE_KEYMAP_PAYLOAD_MAX_LEN:
        raise ValueError(f"invalid source keymap payload length {payload_length}")
    expected_chunk_count = (payload_length + SOURCE_KEYMAP_CHUNK_DATA_LEN - 1) // SOURCE_KEYMAP_CHUNK_DATA_LEN
    if chunk_count != expected_chunk_count:
        raise ValueError(
            f"source keymap chunk count {chunk_count} does not match payload length {payload_length}"
        )
    start = chunk_index * SOURCE_KEYMAP_CHUNK_DATA_LEN
    if start >= payload_length:
        raise ValueError(f"source keymap chunk starts beyond payload: {start}/{payload_length}")
    meaningful = min(SOURCE_KEYMAP_CHUNK_DATA_LEN, payload_length - start)
    if any(block[8 + meaningful : 28]):
        raise ValueError("source keymap chunk padding is not zero")
    return {
        "source_slot": slot,
        "chunk_index": chunk_index,
        "chunk_count": chunk_count,
        "payload_length": payload_length,
        "data": bytes(block[8 : 8 + meaningful]),
    }


def read_keymap_block(device) -> dict[str, object]:
    """Read one keymap block, accepting hidapi's native or prefixed framing."""

    raw = bytes(device.get_feature_report(CONFIG_REPORT_ID, CONFIG_REPORT_LEN + 1))
    if len(raw) == CONFIG_REPORT_LEN + 1 and raw[0] != CONFIG_REPORT_ID:
        raise ValueError(
            f"unexpected report ID prefix {raw[0]}; expected {CONFIG_REPORT_ID}"
        )
    errors: list[ValueError] = []
    for candidate in (raw[:CONFIG_REPORT_LEN], raw[1 : 1 + CONFIG_REPORT_LEN]):
        if len(candidate) != CONFIG_REPORT_LEN:
            continue
        try:
            return decode_keymap_chunk(candidate)
        except ValueError as error:
            errors.append(error)
    detail = str(errors[-1]) if errors else f"unexpected report length {len(raw)}"
    raise ValueError(f"could not decode keymap diagnostics: {detail}")


def read_keymap(device) -> list[dict[str, object]]:
    """Read, reassemble, and validate the complete active keymap."""

    chunks: list[dict[str, object]] = []
    payload_length: int | None = None
    chunk_count: int | None = None
    for index in range(KEYMAP_CHUNK_COUNT_MAX):
        ensure_report_sent(device, build_select_keymap_packet(index))
        chunk = read_keymap_block(device)
        if chunk["chunk_index"] != index:
            raise ValueError(
                f"keymap chunk order changed: expected {index}, got {chunk['chunk_index']}"
            )
        if payload_length is None:
            payload_length = int(chunk["payload_length"])
            chunk_count = int(chunk["chunk_count"])
        elif chunk["payload_length"] != payload_length or chunk["chunk_count"] != chunk_count:
            raise ValueError("keymap chunk metadata changed during read")
        chunks.append(chunk)
        if len(chunks) == chunk_count:
            break

    if payload_length is None or chunk_count is None or len(chunks) != chunk_count:
        raise ValueError("keymap read ended before all chunks arrived")
    payload = bytearray(payload_length)
    for chunk in chunks:
        start = int(chunk["chunk_index"]) * KEYMAP_CHUNK_DATA_LEN
        data = chunk["data"]
        assert isinstance(data, bytes)
        payload[start : start + len(data)] = data
    return decode_keymap_payload(bytes(payload))


def read_source_keymap(device, slot: int) -> list[dict[str, object]]:
    """Read, reassemble, and validate one source-owned keymap."""

    if not 0 <= slot < SOURCE_SLOT_COUNT:
        raise ValueError(f"source slot must be from 0 through {SOURCE_SLOT_COUNT - 1}: {slot}")
    chunks: list[dict[str, object]] = []
    payload_length: int | None = None
    chunk_count: int | None = None
    for index in range(SOURCE_KEYMAP_CHUNK_COUNT_MAX):
        ensure_report_sent(device, build_select_source_keymap_packet(slot, index))
        raw = bytes(device.get_feature_report(CONFIG_REPORT_ID, CONFIG_REPORT_LEN + 1))
        errors: list[ValueError] = []
        chunk: dict[str, object] | None = None
        for candidate in (raw[:CONFIG_REPORT_LEN], raw[1 : 1 + CONFIG_REPORT_LEN]):
            if len(candidate) != CONFIG_REPORT_LEN:
                continue
            try:
                chunk = decode_source_keymap_chunk(candidate)
                break
            except ValueError as error:
                errors.append(error)
        if chunk is None:
            detail = str(errors[-1]) if errors else f"unexpected report length {len(raw)}"
            raise ValueError(f"could not decode source keymap diagnostics: {detail}")
        if chunk["source_slot"] != slot or chunk["chunk_index"] != index:
            raise ValueError(
                f"source keymap chunk order changed: expected slot {slot} chunk {index}, "
                f"got slot {chunk['source_slot']} chunk {chunk['chunk_index']}"
            )
        if payload_length is None:
            payload_length = int(chunk["payload_length"])
            chunk_count = int(chunk["chunk_count"])
        elif chunk["payload_length"] != payload_length or chunk["chunk_count"] != chunk_count:
            raise ValueError("source keymap chunk metadata changed during read")
        chunks.append(chunk)
        if len(chunks) == chunk_count:
            break

    if payload_length is None or chunk_count is None or len(chunks) != chunk_count:
        raise ValueError("source keymap read ended before all chunks arrived")
    payload = bytearray(payload_length)
    for chunk in chunks:
        start = int(chunk["chunk_index"]) * SOURCE_KEYMAP_CHUNK_DATA_LEN
        data = chunk["data"]
        assert isinstance(data, bytes)
        payload[start : start + len(data)] = data
    encoded_slot, rules = decode_source_keymap_payload(bytes(payload))
    if encoded_slot != slot:
        raise ValueError(f"source keymap payload belongs to slot {encoded_slot}, not {slot}")
    return rules


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--slot", type=int, choices=range(SOURCE_SLOT_COUNT))
    args = parser.parse_args()
    device = open_bridge()
    try:
        rules = read_keymap(device) if args.slot is None else read_source_keymap(device, args.slot)
        print(json.dumps(rules, ensure_ascii=False, indent=2))
    finally:
        device.close()


if __name__ == "__main__":
    main()
