#!/usr/bin/env python3
"""Create and validate UF2 files for the factory XIAO nRF52840 Sense layout."""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

UF2_BLOCK_SIZE = 512
UF2_PAYLOAD_SIZE = 256
UF2_MAGIC_START_0 = 0x0A324655
UF2_MAGIC_START_1 = 0x9E5D5157
UF2_MAGIC_END = 0x0AB16F30
UF2_FLAG_FAMILY_ID = 0x00002000
NRF52840_FAMILY_ID = 0xADA52840

APP_START = 0x00027000
APP_END = 0x000EC000
RAM_START = 0x20000000
RAM_END = 0x20040000


class Uf2Error(ValueError):
    """Raised when input cannot be represented by this board-safe UF2 profile."""


def pack_binary(binary: bytes, base: int = APP_START) -> bytes:
    """Pack a raw application binary as deterministic 256-byte UF2 blocks."""
    if base != APP_START:
        raise Uf2Error(f"application base must be 0x{APP_START:08X}")
    if not binary:
        raise Uf2Error("input binary is empty")

    block_count = (len(binary) + UF2_PAYLOAD_SIZE - 1) // UF2_PAYLOAD_SIZE
    padded_end = base + block_count * UF2_PAYLOAD_SIZE
    if padded_end > APP_END:
        raise Uf2Error(
            f"image ends at 0x{padded_end:08X}, beyond application end 0x{APP_END:08X}"
        )

    output = bytearray()
    for block_number in range(block_count):
        offset = block_number * UF2_PAYLOAD_SIZE
        payload = binary[offset : offset + UF2_PAYLOAD_SIZE]
        payload = payload.ljust(UF2_PAYLOAD_SIZE, b"\x00")
        header = struct.pack(
            "<IIIIIIII",
            UF2_MAGIC_START_0,
            UF2_MAGIC_START_1,
            UF2_FLAG_FAMILY_ID,
            base + offset,
            UF2_PAYLOAD_SIZE,
            block_number,
            block_count,
            NRF52840_FAMILY_ID,
        )
        output.extend(header)
        output.extend(payload)
        output.extend(bytes(476 - UF2_PAYLOAD_SIZE))
        output.extend(struct.pack("<I", UF2_MAGIC_END))
    return bytes(output)


def validate_application_vectors(binary: bytes) -> tuple[int, int]:
    """Reject a raw image whose Cortex-M vector table cannot boot this board.

    A build that loses its linker script still produces a non-empty binary, and
    the bootloader will happily write it and then jump into nothing. Checking
    the two mandatory vector-table entries turns that silent failure into a
    build error before anything is flashed.
    """
    if len(binary) < 8:
        raise Uf2Error("image is shorter than the mandatory Cortex-M vector table")

    initial_sp, reset_vector = struct.unpack("<II", binary[:8])
    if not RAM_START < initial_sp <= RAM_END or initial_sp % 4:
        raise Uf2Error(
            f"initial stack pointer 0x{initial_sp:08X} is not word-aligned RAM in "
            f"0x{RAM_START:08X}..0x{RAM_END:08X}"
        )
    if not reset_vector & 1:
        raise Uf2Error(
            f"reset vector 0x{reset_vector:08X} does not have the Thumb bit set"
        )

    reset_address = reset_vector & ~1
    if not APP_START <= reset_address < APP_START + len(binary):
        raise Uf2Error(
            f"reset vector 0x{reset_address:08X} is outside the packed image "
            f"0x{APP_START:08X}..0x{APP_START + len(binary):08X}"
        )
    return initial_sp, reset_address


def validate_uf2(data: bytes) -> tuple[int, int]:
    """Validate UF2 structure and board-safe addresses; return address range."""
    if not data or len(data) % UF2_BLOCK_SIZE:
        raise Uf2Error("UF2 length must be a non-zero multiple of 512 bytes")

    actual_count = len(data) // UF2_BLOCK_SIZE
    addresses: list[int] = []
    declared_count: int | None = None
    for index in range(actual_count):
        block = data[index * UF2_BLOCK_SIZE : (index + 1) * UF2_BLOCK_SIZE]
        fields = struct.unpack("<IIIIIIII", block[:32])
        magic0, magic1, flags, target, payload_size, number, count, family = fields
        end_magic = struct.unpack("<I", block[-4:])[0]
        if (magic0, magic1, end_magic) != (
            UF2_MAGIC_START_0,
            UF2_MAGIC_START_1,
            UF2_MAGIC_END,
        ):
            raise Uf2Error(f"block {index} has invalid UF2 magic")
        if flags & UF2_FLAG_FAMILY_ID == 0 or family != NRF52840_FAMILY_ID:
            raise Uf2Error(f"block {index} is not tagged for nRF52840")
        if payload_size != UF2_PAYLOAD_SIZE:
            raise Uf2Error(f"block {index} payload size is not 256 bytes")
        if number != index:
            raise Uf2Error(f"block {index} has non-deterministic block number {number}")
        if declared_count is None:
            declared_count = count
        if count != declared_count or count != actual_count:
            raise Uf2Error(f"block {index} has inconsistent block count")
        if target < APP_START or target + payload_size > APP_END:
            raise Uf2Error(
                f"block {index} target 0x{target:08X} is outside "
                f"0x{APP_START:08X}..0x{APP_END:08X}"
            )
        addresses.append(target)

    expected = [APP_START + i * UF2_PAYLOAD_SIZE for i in range(actual_count)]
    if addresses != expected:
        raise Uf2Error("UF2 targets must be contiguous from the application origin")
    return addresses[0], addresses[-1] + UF2_PAYLOAD_SIZE


def main() -> int:
    """Run the pack or check command."""
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    pack = subparsers.add_parser("pack", help="convert a raw binary to UF2")
    pack.add_argument("input", type=Path)
    pack.add_argument("output", type=Path)
    check = subparsers.add_parser("check", help="validate an existing UF2")
    check.add_argument("input", type=Path)
    args = parser.parse_args()

    try:
        if args.command == "pack":
            source = args.input.read_bytes()
            initial_sp, reset_address = validate_application_vectors(source)
            result = pack_binary(source)
            validate_uf2(result)
            args.output.write_bytes(result)
            print(
                f"wrote {args.output} ({len(result) // UF2_BLOCK_SIZE} UF2 blocks, "
                f"SP 0x{initial_sp:08X}, reset 0x{reset_address:08X})"
            )
        else:
            start, end = validate_uf2(args.input.read_bytes())
            print(f"valid XIAO nRF52840 Sense UF2: 0x{start:08X}..0x{end:08X}")
    except (OSError, Uf2Error) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
