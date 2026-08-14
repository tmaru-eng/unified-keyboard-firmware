#!/usr/bin/env python3
"""Host-only regression tests for the nRF52840 linker memory layout."""

from __future__ import annotations

import re
import unittest
from pathlib import Path


MEMORY_X = Path(__file__).parents[1] / "memory.x"
EXPECTED_RAM_ORIGIN = 0x20010000
EXPECTED_RAM_LENGTH = 0x00030000
EXPECTED_RAM_END = 0x20040000


class MemoryLayoutTests(unittest.TestCase):
    """Keep the conservative S140 RAM reservation visible and stable."""

    def test_s140_ram_boundary_is_conservative_and_reaches_ram_end(self) -> None:
        source = MEMORY_X.read_text(encoding="utf-8")
        match = re.search(
            r"^\s*RAM\s*:\s*ORIGIN\s*=\s*(0x[0-9A-Fa-f]+)\s*,\s*"
            r"LENGTH\s*=\s*(0x[0-9A-Fa-f]+)\s*$",
            source,
            re.MULTILINE,
        )

        self.assertIsNotNone(match, "memory.x must declare a parseable RAM region")
        assert match is not None
        origin = int(match.group(1), 16)
        length = int(match.group(2), 16)

        self.assertEqual(origin, EXPECTED_RAM_ORIGIN)
        self.assertEqual(length, EXPECTED_RAM_LENGTH)
        self.assertEqual(origin + length, EXPECTED_RAM_END)


if __name__ == "__main__":
    unittest.main()
