# /// script
# requires-python = ">=3.11"
# dependencies = ["hidapi"]
# ///
"""Write a data-driven single-layer keymap through target 5.

Examples::

    uv run write_keymap.py --hex "01 01 04 00 05 00"
    uv run write_keymap.py --rules keymap.json
    uv run write_keymap.py --us-jis

The JSON form is a list of objects with ``input_usage``, ``input_shifted``,
``output_usage``, and ``output_shifted`` fields. The ``--us-jis`` option is
the recovery path after a custom hardware test.
"""

from __future__ import annotations

import argparse
import json
from collections.abc import Mapping, Sequence
from pathlib import Path

from write_config import TARGET_KEYMAP, TARGET_SOURCE_KEYMAP, open_bridge, send_payload

KEYMAP_PAYLOAD_VERSION = 1
SOURCE_KEYMAP_PAYLOAD_VERSION = 2
SOURCE_SLOT_COUNT = 5
KEYMAP_RULE_CAPACITY = 32
KEYMAP_RULE_WIRE_LEN = 4
KEYMAP_PAYLOAD_MAX_LEN = 2 + KEYMAP_RULE_CAPACITY * KEYMAP_RULE_WIRE_LEN

DEFAULT_US_JIS_RULES = (
    {"input_usage": 0x35, "input_shifted": False, "output_usage": 0x2F, "output_shifted": True},
    {"input_usage": 0x35, "input_shifted": True, "output_usage": 0x2E, "output_shifted": True},
    {"input_usage": 0x1F, "input_shifted": True, "output_usage": 0x2F, "output_shifted": False},
    {"input_usage": 0x23, "input_shifted": True, "output_usage": 0x2E, "output_shifted": False},
    {"input_usage": 0x24, "input_shifted": True, "output_usage": 0x23, "output_shifted": True},
    {"input_usage": 0x25, "input_shifted": True, "output_usage": 0x34, "output_shifted": True},
    {"input_usage": 0x26, "input_shifted": True, "output_usage": 0x25, "output_shifted": True},
    {"input_usage": 0x27, "input_shifted": True, "output_usage": 0x26, "output_shifted": True},
    {"input_usage": 0x2D, "input_shifted": True, "output_usage": 0x87, "output_shifted": True},
    {"input_usage": 0x2E, "input_shifted": False, "output_usage": 0x2D, "output_shifted": True},
    {"input_usage": 0x2E, "input_shifted": True, "output_usage": 0x33, "output_shifted": True},
    {"input_usage": 0x2F, "input_shifted": False, "output_usage": 0x30, "output_shifted": False},
    {"input_usage": 0x2F, "input_shifted": True, "output_usage": 0x30, "output_shifted": True},
    {"input_usage": 0x30, "input_shifted": False, "output_usage": 0x31, "output_shifted": False},
    {"input_usage": 0x30, "input_shifted": True, "output_usage": 0x31, "output_shifted": True},
    {"input_usage": 0x31, "input_shifted": False, "output_usage": 0x87, "output_shifted": False},
    {"input_usage": 0x31, "input_shifted": True, "output_usage": 0x89, "output_shifted": True},
    {"input_usage": 0x33, "input_shifted": True, "output_usage": 0x34, "output_shifted": False},
    {"input_usage": 0x34, "input_shifted": False, "output_usage": 0x24, "output_shifted": True},
    {"input_usage": 0x34, "input_shifted": True, "output_usage": 0x1F, "output_shifted": True},
)


def _byte(value: object, label: str, *, allow_zero: bool = True) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ValueError(f"{label} must be an integer byte")
    if not allow_zero and value == 0:
        raise ValueError(f"{label} must be greater than zero")
    if not 0 <= value <= 0xFF:
        raise ValueError(f"{label} must be an integer byte")
    return value


def _boolean(value: object, label: str) -> bool:
    if not isinstance(value, bool):
        raise ValueError(f"{label} must be a boolean")
    return value


def encode_keymap_payload(rules: Sequence[Mapping[str, object]]) -> bytes:
    """Encode JSON-shaped keymap rules in the firmware's target 5 format."""

    if len(rules) > KEYMAP_RULE_CAPACITY:
        raise ValueError(f"keymap cannot contain more than {KEYMAP_RULE_CAPACITY} rules")

    payload = bytearray((KEYMAP_PAYLOAD_VERSION, len(rules)))
    seen: set[tuple[int, bool]] = set()
    for index, rule in enumerate(rules):
        if not isinstance(rule, Mapping):
            raise ValueError(f"rules[{index}] must be an object")
        try:
            input_usage = _byte(rule["input_usage"], f"rules[{index}].input_usage", allow_zero=False)
            input_shifted = _boolean(rule["input_shifted"], f"rules[{index}].input_shifted")
            output_usage = _byte(rule["output_usage"], f"rules[{index}].output_usage")
            output_shifted = _boolean(rule["output_shifted"], f"rules[{index}].output_shifted")
        except KeyError as error:
            raise ValueError(f"rules[{index}] is missing {error.args[0]}") from error

        identity = (input_usage, input_shifted)
        if identity in seen:
            raise ValueError(f"rules[{index}] duplicates an input usage and shift state")
        seen.add(identity)
        flags = int(input_shifted) | (int(output_shifted) << 1)
        payload.extend((input_usage, flags, output_usage, 0))
    return bytes(payload)


def decode_keymap_payload(payload: bytes) -> list[dict[str, object]]:
    """Decode and validate the firmware's versioned keymap payload."""

    if len(payload) < 2:
        raise ValueError(f"keymap payload is too short: {len(payload)}")
    if payload[0] != KEYMAP_PAYLOAD_VERSION:
        raise ValueError(f"unsupported keymap payload version {payload[0]}")
    count = payload[1]
    if count > KEYMAP_RULE_CAPACITY:
        raise ValueError(f"keymap contains too many rules: {count}")
    expected_length = 2 + count * KEYMAP_RULE_WIRE_LEN
    if len(payload) != expected_length:
        raise ValueError(f"keymap payload length must be {expected_length}: {len(payload)}")

    rules: list[dict[str, object]] = []
    seen: set[tuple[int, bool]] = set()
    for index in range(count):
        offset = 2 + index * KEYMAP_RULE_WIRE_LEN
        input_usage = payload[offset]
        flags = payload[offset + 1]
        if input_usage == 0:
            raise ValueError(f"rules[{index}].input_usage must be greater than zero")
        if flags & ~0x03:
            raise ValueError(f"rules[{index}] contains unknown flags {flags}")
        if payload[offset + 3] != 0:
            raise ValueError(f"rules[{index}] has a non-zero reserved byte")
        input_shifted = bool(flags & 0x01)
        identity = (input_usage, input_shifted)
        if identity in seen:
            raise ValueError(f"rules[{index}] duplicates an input usage and shift state")
        seen.add(identity)
        rules.append(
            {
                "input_usage": input_usage,
                "input_shifted": input_shifted,
                "output_usage": payload[offset + 2],
                "output_shifted": bool(flags & 0x02),
            }
        )
    return rules


def _source_slot(slot: object) -> int:
    if isinstance(slot, bool) or not isinstance(slot, int) or not 0 <= slot < SOURCE_SLOT_COUNT:
        raise ValueError(f"source slot must be an integer from 0 through {SOURCE_SLOT_COUNT - 1}: {slot}")
    return slot


def encode_source_keymap_payload(
    slot: int, rules: Sequence[Mapping[str, object]]
) -> bytes:
    """Encode a source-owned keymap payload for target 6."""

    _source_slot(slot)
    legacy = encode_keymap_payload(rules)
    return bytes((SOURCE_KEYMAP_PAYLOAD_VERSION, slot)) + legacy[1:]


def decode_source_keymap_payload(payload: bytes) -> tuple[int, list[dict[str, object]]]:
    """Decode a target 6 payload and return its explicit source slot."""

    if len(payload) < 3:
        raise ValueError(f"source keymap payload is too short: {len(payload)}")
    if payload[0] != SOURCE_KEYMAP_PAYLOAD_VERSION:
        raise ValueError(f"unsupported source keymap payload version {payload[0]}")
    slot = _source_slot(payload[1])
    legacy = bytes((KEYMAP_PAYLOAD_VERSION, payload[2])) + payload[3:]
    return slot, decode_keymap_payload(legacy)


def load_payload(path: Path | None, hex_data: str | None) -> bytes:
    """Load either a JSON rule list or a complete wire payload."""

    if path is not None and hex_data is not None:
        raise ValueError("choose a JSON file or --hex, not both")
    if path is not None:
        try:
            rules = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise ValueError(f"could not read keymap JSON: {path}") from error
        if not isinstance(rules, list):
            raise ValueError("keymap JSON must contain a list of rules")
        return encode_keymap_payload(rules)
    if hex_data is not None:
        try:
            return bytes.fromhex(hex_data)
        except ValueError as error:
            raise ValueError("--hex must contain hexadecimal bytes") from error
    raise ValueError("provide --rules, --hex, or --us-jis")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--rules", type=Path, help="JSON file containing keymap rules")
    source.add_argument("--hex", dest="hex_data", help="complete versioned payload in hex")
    source.add_argument("--us-jis", action="store_true", help="restore the factory keymap")
    parser.add_argument(
        "--slot",
        type=int,
        choices=range(SOURCE_SLOT_COUNT),
        help="write one source-owned keymap (target 6) instead of the legacy global keymap (target 5)",
    )
    args = parser.parse_args()

    if args.slot is None:
        payload = encode_keymap_payload(DEFAULT_US_JIS_RULES) if args.us_jis else load_payload(args.rules, args.hex_data)
        target = TARGET_KEYMAP
    elif args.rules is not None:
        rules = json.loads(args.rules.read_text(encoding="utf-8"))
        if not isinstance(rules, list):
            raise ValueError("keymap JSON must contain a list of rules")
        payload = encode_source_keymap_payload(args.slot, rules)
        target = TARGET_SOURCE_KEYMAP
    elif args.us_jis:
        payload = encode_source_keymap_payload(args.slot, DEFAULT_US_JIS_RULES)
        target = TARGET_SOURCE_KEYMAP
    else:
        if args.hex_data is None:
            raise ValueError("provide --rules, --hex, or --us-jis")
        payload = bytes.fromhex(args.hex_data)
        encoded_slot, _rules = decode_source_keymap_payload(payload)
        if encoded_slot != args.slot:
            raise ValueError(f"source keymap payload belongs to slot {encoded_slot}, not {args.slot}")
        target = TARGET_SOURCE_KEYMAP
    device = open_bridge()
    try:
        send_payload(device, payload, target=target)
    finally:
        device.close()


if __name__ == "__main__":
    main()
