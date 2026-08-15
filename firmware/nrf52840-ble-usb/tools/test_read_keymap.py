"""Host-only tests for the keymap diagnostics reader."""

import math
import struct
import unittest
import zlib

from read_keymap import (
    KEYMAP_CHUNK_DATA_LEN,
    KEYMAP_KIND,
    KEYMAP_REPORT_VERSION,
    build_select_keymap_packet,
    build_select_source_keymap_packet,
    decode_keymap_chunk,
    decode_source_keymap_chunk,
    read_keymap,
    read_source_keymap,
)
from reset_xiao import CONFIG_PROTOCOL_VERSION, CONFIG_REPORT_ID, CONFIG_REPORT_LEN
from write_keymap import encode_keymap_payload


def keymap_block(payload: bytes, chunk_index: int) -> bytes:
    chunk_count = math.ceil(len(payload) / KEYMAP_CHUNK_DATA_LEN)
    start = chunk_index * KEYMAP_CHUNK_DATA_LEN
    block = bytearray(CONFIG_REPORT_LEN)
    block[0] = CONFIG_PROTOCOL_VERSION
    block[1] = KEYMAP_KIND
    block[2] = KEYMAP_REPORT_VERSION
    block[3] = chunk_index
    block[4] = chunk_count
    struct.pack_into("<H", block, 5, len(payload))
    end = min(start + KEYMAP_CHUNK_DATA_LEN, len(payload))
    block[7 : 7 + end - start] = payload[start:end]
    struct.pack_into("<I", block, 28, zlib.crc32(block[:28]) & 0xFFFFFFFF)
    return bytes(block)


def source_keymap_block(payload: bytes, slot: int, chunk_index: int) -> bytes:
    chunk_data_len = 20
    chunk_count = math.ceil(len(payload) / chunk_data_len)
    start = chunk_index * chunk_data_len
    block = bytearray(CONFIG_REPORT_LEN)
    block[0] = CONFIG_PROTOCOL_VERSION
    block[1] = KEYMAP_KIND
    block[2] = 2
    block[3] = chunk_index
    block[4] = chunk_count
    struct.pack_into("<H", block, 5, len(payload))
    block[7] = slot
    end = min(start + chunk_data_len, len(payload))
    block[8 : 8 + end - start] = payload[start:end]
    struct.pack_into("<I", block, 28, zlib.crc32(block[:28]) & 0xFFFFFFFF)
    return bytes(block)


class KeymapDiagnosticsTest(unittest.TestCase):
    def test_selector_has_the_new_command_and_zero_reserved_bytes(self):
        packet = build_select_keymap_packet(2)

        self.assertEqual(packet[1:4], bytes((CONFIG_PROTOCOL_VERSION, 108, 2)))
        self.assertEqual(packet[4:29], bytes(25))

    def test_reader_reassembles_prefixed_two_chunk_response(self):
        rules = [
            {"input_usage": 4, "input_shifted": False, "output_usage": 5, "output_shifted": False},
            {"input_usage": 31, "input_shifted": True, "output_usage": 47, "output_shifted": False},
            {"input_usage": 32, "input_shifted": False, "output_usage": 48, "output_shifted": True},
            {"input_usage": 33, "input_shifted": True, "output_usage": 49, "output_shifted": True},
            {"input_usage": 34, "input_shifted": False, "output_usage": 50, "output_shifted": False},
            {"input_usage": 35, "input_shifted": True, "output_usage": 51, "output_shifted": True},
        ]
        payload = encode_keymap_payload(rules)

        class Device:
            def __init__(self):
                self.selected = 0
                self.packets = []

            def send_feature_report(self, packet: bytes) -> int:
                self.packets.append(packet)
                self.selected = packet[3]
                return CONFIG_REPORT_LEN + 1

            def get_feature_report(self, report_id: int, length: int) -> bytes:
                self.assert_report(report_id, length)
                return bytes([CONFIG_REPORT_ID]) + keymap_block(payload, self.selected)

            @staticmethod
            def assert_report(report_id: int, length: int) -> None:
                if report_id != CONFIG_REPORT_ID or length != CONFIG_REPORT_LEN + 1:
                    raise AssertionError("unexpected report request")

        device = Device()

        self.assertEqual(read_keymap(device), rules)
        self.assertEqual([packet[3] for packet in device.packets], [0, 1])

    def test_decoder_rejects_non_zero_final_padding(self):
        block = bytearray(keymap_block(bytes((1, 0)), 0))
        block[9] = 1
        struct.pack_into("<I", block, 28, zlib.crc32(block[:28]) & 0xFFFFFFFF)

        with self.assertRaisesRegex(ValueError, "padding"):
            decode_keymap_chunk(bytes(block))

    def test_decoder_rejects_a_chunk_count_that_does_not_fit_the_payload(self):
        block = keymap_block(bytes((1, 0)) + bytes(20), 0)
        malformed = bytearray(block)
        malformed[4] = 1
        struct.pack_into("<I", malformed, 28, zlib.crc32(malformed[:28]) & 0xFFFFFFFF)

        with self.assertRaisesRegex(ValueError, "chunk count"):
            decode_keymap_chunk(bytes(malformed))

    def test_source_selector_carries_slot_and_chunk(self):
        packet = build_select_source_keymap_packet(4, 2)

        self.assertEqual(packet[1:5], bytes((CONFIG_PROTOCOL_VERSION, 109, 4, 2)))
        self.assertEqual(packet[5:29], bytes(24))

    def test_source_reader_reassembles_a_prefixed_two_chunk_response(self):
        rules = [
            {"input_usage": 4, "input_shifted": False, "output_usage": 5, "output_shifted": False},
            {"input_usage": 31, "input_shifted": True, "output_usage": 47, "output_shifted": False},
            {"input_usage": 32, "input_shifted": False, "output_usage": 48, "output_shifted": True},
            {"input_usage": 33, "input_shifted": True, "output_usage": 49, "output_shifted": True},
            {"input_usage": 34, "input_shifted": False, "output_usage": 50, "output_shifted": False},
            {"input_usage": 35, "input_shifted": True, "output_usage": 51, "output_shifted": True},
        ]
        from write_keymap import encode_source_keymap_payload

        payload = encode_source_keymap_payload(3, rules)

        class Device:
            def __init__(self):
                self.selected_slot = 0
                self.selected_chunk = 0
                self.packets = []

            def send_feature_report(self, packet: bytes) -> int:
                self.packets.append(packet)
                self.selected_slot = packet[3]
                self.selected_chunk = packet[4]
                return CONFIG_REPORT_LEN + 1

            def get_feature_report(self, report_id: int, length: int) -> bytes:
                if report_id != CONFIG_REPORT_ID or length != CONFIG_REPORT_LEN + 1:
                    raise AssertionError("unexpected report request")
                return bytes([CONFIG_REPORT_ID]) + source_keymap_block(
                    payload, self.selected_slot, self.selected_chunk
                )

        device = Device()

        self.assertEqual(read_source_keymap(device, 3), rules)
        self.assertEqual([packet[1:5] for packet in device.packets], [bytes((18, 109, 3, 0)), bytes((18, 109, 3, 1))])

    def test_source_decoder_rejects_non_zero_padding(self):
        block = bytearray(source_keymap_block(bytes((2, 0, 0)), 0, 0))
        block[11] = 1
        struct.pack_into("<I", block, 28, zlib.crc32(block[:28]) & 0xFFFFFFFF)

        with self.assertRaisesRegex(ValueError, "padding"):
            decode_source_keymap_chunk(bytes(block))


if __name__ == "__main__":
    unittest.main()
