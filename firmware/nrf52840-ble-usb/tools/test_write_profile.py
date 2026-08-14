# /// script
# requires-python = ">=3.11"
# ///
"""Host-only tests for the profile writer's two-byte payload."""

import unittest

from write_profile import (
    build_profile_payload,
    build_source_profile_payload,
    profile_diagnostic_matches,
)


class ProfilePayloadTest(unittest.TestCase):
    def test_all_supported_flags_use_the_fixed_payload_layout(self):
        for flags in range(8):
            self.assertEqual(
                build_profile_payload(
                    us_jis=bool(flags & 0x01),
                    caps_to_ctrl=bool(flags & 0x02),
                    swap_alt_gui=bool(flags & 0x04),
                ),
                bytes((1, flags)),
            )

    def test_source_profile_payload_has_explicit_version_length_slot_and_flags(self):
        self.assertEqual(
            build_source_profile_payload(
                slot=4,
                us_jis=True,
                caps_to_ctrl=False,
                swap_alt_gui=True,
            ),
            bytes((2, 4, 4, 5)),
        )

    def test_saved_or_pending_profile_is_a_successful_readback(self):
        saved = bytearray(32)
        saved[3] = 1
        saved[4] = 1
        pending = bytearray(32)
        pending[3] = 1
        pending[5] = 1

        self.assertTrue(profile_diagnostic_matches(saved, 1))
        self.assertTrue(profile_diagnostic_matches(pending, 1))

    def test_profile_without_saved_or_pending_state_is_not_a_success(self):
        diagnostic = bytearray(32)
        diagnostic[3] = 1

        self.assertFalse(profile_diagnostic_matches(diagnostic, 1))


if __name__ == "__main__":
    unittest.main()
