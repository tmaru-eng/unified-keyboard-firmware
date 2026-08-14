#!/usr/bin/env python3
"""Host-only regression tests for XIAO UF2 packaging."""

from __future__ import annotations

import struct
import unittest

from xiao_uf2 import (
    APP_END,
    APP_START,
    RAM_END,
    Uf2Error,
    pack_binary,
    validate_application_vectors,
    validate_uf2,
)


def _image(initial_sp: int, reset_vector: int, length: int = 512) -> bytes:
    """Build a raw image whose first two words are a Cortex-M vector table."""
    return struct.pack("<II", initial_sp, reset_vector) + bytes(length - 8)


class XiaoUf2Tests(unittest.TestCase):
    """Verify deterministic packaging and protected-region rejection."""

    def test_pack_is_deterministic_and_contiguous(self) -> None:
        source = bytes((index * 17) % 256 for index in range(300))
        first = pack_binary(source)
        second = pack_binary(source)
        self.assertEqual(first, second)
        self.assertEqual(validate_uf2(first), (APP_START, APP_START + 512))

    def test_rejects_image_that_reaches_protected_storage(self) -> None:
        with self.assertRaises(Uf2Error):
            pack_binary(bytes(APP_END - APP_START + 1))

    def test_rejects_block_targeting_softdevice(self) -> None:
        uf2 = bytearray(pack_binary(b"safe"))
        struct.pack_into("<I", uf2, 12, APP_START - 256)
        with self.assertRaises(Uf2Error):
            validate_uf2(bytes(uf2))


class ApplicationVectorTests(unittest.TestCase):
    """Verify that an unbootable raw image is rejected before it is packed."""

    def test_accepts_a_linked_application_vector_table(self) -> None:
        image = _image(RAM_END, APP_START + 0x101)
        self.assertEqual(
            validate_application_vectors(image), (RAM_END, APP_START + 0x100)
        )

    def test_rejects_an_image_without_a_vector_table(self) -> None:
        # A build that lost `-Tlink.x` links no vector table at all.
        with self.assertRaises(Uf2Error):
            validate_application_vectors(b"\x00\x00\x00")

    def test_rejects_a_stack_pointer_outside_ram(self) -> None:
        with self.assertRaises(Uf2Error):
            validate_application_vectors(_image(APP_START, APP_START + 0x101))

    def test_rejects_a_reset_vector_without_the_thumb_bit(self) -> None:
        with self.assertRaises(Uf2Error):
            validate_application_vectors(_image(RAM_END, APP_START + 0x100))

    def test_rejects_a_reset_vector_outside_the_packed_image(self) -> None:
        with self.assertRaises(Uf2Error):
            validate_application_vectors(_image(RAM_END, APP_START + 0x4001))


if __name__ == "__main__":
    unittest.main()
