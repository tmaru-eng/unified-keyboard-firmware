"""Host-only tests for the target 5 keymap writer."""

import json
import struct
import tempfile
import unittest
import zlib
from pathlib import Path

from reset_xiao import CONFIG_PROTOCOL_VERSION, CONFIG_REPORT_ID, CONFIG_REPORT_LEN
from write_config import send_payload
from write_keymap import (
    DEFAULT_US_JIS_RULES,
    decode_keymap_payload,
    decode_source_keymap_payload,
    encode_keymap_payload,
    encode_source_keymap_payload,
    load_payload,
)


class KeymapPayloadTest(unittest.TestCase):
    def test_empty_keymap_has_only_version_and_count(self):
        self.assertEqual(encode_keymap_payload([]), bytes((1, 0)))

    def test_rule_payload_uses_the_firmware_wire_layout(self):
        self.assertEqual(
            encode_keymap_payload(
                [
                    {
                        "input_usage": 0x04,
                        "input_shifted": False,
                        "output_usage": 0x05,
                        "output_shifted": True,
                    }
                ]
            ),
            bytes((1, 1, 0x04, 0x02, 0x05, 0)),
        )

    def test_duplicate_input_and_shift_state_is_rejected(self):
        rule = {
            "input_usage": 0x04,
            "input_shifted": False,
            "output_usage": 0x05,
            "output_shifted": False,
        }
        with self.assertRaisesRegex(ValueError, "duplicates"):
            encode_keymap_payload([rule, rule])

    def test_rule_must_be_a_json_object(self):
        with self.assertRaisesRegex(ValueError, "must be an object"):
            encode_keymap_payload([1])

    def test_factory_map_is_a_complete_versioned_payload(self):
        payload = encode_keymap_payload(DEFAULT_US_JIS_RULES)
        self.assertEqual(payload[:2], bytes((1, 20)))
        self.assertEqual(len(payload), 82)

    def test_payload_decodes_to_the_json_rule_shape(self):
        payload = encode_keymap_payload(
            [{"input_usage": 4, "input_shifted": False, "output_usage": 5, "output_shifted": True}]
        )

        self.assertEqual(
            decode_keymap_payload(payload),
            [{"input_usage": 4, "input_shifted": False, "output_usage": 5, "output_shifted": True}],
        )

    def test_payload_decoder_rejects_a_non_zero_reserved_byte(self):
        payload = bytearray((1, 1, 4, 0, 5, 0))
        payload[5] = 1

        with self.assertRaisesRegex(ValueError, "reserved"):
            decode_keymap_payload(bytes(payload))

    def test_source_payload_carries_its_slot_and_uses_version_two(self):
        rules = [{"input_usage": 4, "input_shifted": False, "output_usage": 5, "output_shifted": True}]

        payload = encode_source_keymap_payload(3, rules)

        self.assertEqual(payload, bytes((2, 3, 1, 4, 2, 5, 0)))
        self.assertEqual(decode_source_keymap_payload(payload), (3, rules))

    def test_source_payload_rejects_a_different_owner_slot(self):
        payload = bytearray((2, 1, 0))
        payload[1] = 5

        with self.assertRaisesRegex(ValueError, "source slot"):
            decode_source_keymap_payload(bytes(payload))

    def test_json_rules_are_loaded_as_the_same_payload(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "keymap.json"
            path.write_text(
                json.dumps(
                    [
                        {
                            "input_usage": 0x04,
                            "input_shifted": False,
                            "output_usage": 0x05,
                            "output_shifted": False,
                        }
                    ]
                ),
                encoding="utf-8",
            )
            self.assertEqual(load_payload(path, None), bytes((1, 1, 4, 0, 5, 0)))

    def test_hex_and_json_are_mutually_exclusive(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "keymap.json"
            path.write_text("[]", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "not both"):
                load_payload(path, "01 00")

    def test_transfer_writer_uses_target_5_for_begin_and_commit(self):
        class Device:
            def __init__(self):
                self.packets = []

            def send_feature_report(self, packet: bytes) -> int:
                self.packets.append(packet)
                return CONFIG_REPORT_LEN + 1

            def get_feature_report(self, report_id: int, length: int) -> bytes:
                payload = bytearray(CONFIG_REPORT_LEN)
                payload[0] = CONFIG_PROTOCOL_VERSION
                payload[1] = 5
                payload[2] = 2
                payload[4] = 5
                payload[28:] = struct.pack(
                    "<I", zlib.crc32(payload[:28]) & 0xFFFFFFFF
                )
                return bytes([CONFIG_REPORT_ID]) + bytes(payload)

        device = Device()
        send_payload(device, bytes((1, 0)), target=5)

        self.assertEqual(device.packets[0][1:3], bytes((CONFIG_PROTOCOL_VERSION, 104)))
        self.assertEqual(device.packets[-2][1:3], bytes((CONFIG_PROTOCOL_VERSION, 106)))
        self.assertEqual(device.packets[0][3], 5)
        self.assertEqual(device.packets[-2][3], 5)

    def test_transfer_writer_uses_target_6_for_source_keymap(self):
        class Device:
            def __init__(self):
                self.packets = []

            def send_feature_report(self, packet: bytes) -> int:
                self.packets.append(packet)
                return CONFIG_REPORT_LEN + 1

            def get_feature_report(self, report_id: int, length: int) -> bytes:
                payload = bytearray(CONFIG_REPORT_LEN)
                payload[0] = CONFIG_PROTOCOL_VERSION
                payload[1] = 5
                payload[2] = 2
                payload[4] = 6
                payload[28:] = struct.pack(
                    "<I", zlib.crc32(payload[:28]) & 0xFFFFFFFF
                )
                return bytes([CONFIG_REPORT_ID]) + bytes(payload)

        device = Device()
        send_payload(device, bytes((2, 3, 0)), target=6)

        self.assertEqual(device.packets[0][1:3], bytes((CONFIG_PROTOCOL_VERSION, 104)))
        self.assertEqual(device.packets[-2][1:3], bytes((CONFIG_PROTOCOL_VERSION, 106)))
        self.assertEqual(device.packets[0][3], 6)
        self.assertEqual(device.packets[-2][3], 6)


if __name__ == "__main__":
    unittest.main()
