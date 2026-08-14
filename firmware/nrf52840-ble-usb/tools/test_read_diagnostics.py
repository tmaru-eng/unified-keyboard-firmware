# /// script
# requires-python = ">=3.11"
# ///
"""The decoder has to agree with the firmware's encoder byte for byte.

Both sides write offsets by hand. A field added on one side and not the other
does not fail — it silently prints a neighbouring byte, which during bring-up
is worse than printing nothing.
"""

import struct
import unittest
import zlib

from read_diagnostics import (
    PROFILE_KIND,
    SOURCE_KIND,
    PASSKEY_KIND,
    TRANSFER_KIND,
    DiagnosticsError,
    decode_security,
    decode_profile,
    decode_transfer,
    decode_identity,
    decode_source,
    describe_bond_flags,
    describe_stuck_key,
    _read_block,
)
from reset_xiao import CONFIG_PROTOCOL_VERSION, CONFIG_REPORT_ID, CONFIG_REPORT_LEN


def source_block(
    slot: int = 4,
    transport: int = 3,
    state: int = 5,
    flags: int = 0,
    profile_flags: int = 5,
    address: bytes = bytes(6),
    name: bytes = b"Virtual Input",
) -> bytes:
    """Build the versioned source block at the firmware's fixed offsets."""

    payload = bytearray(CONFIG_REPORT_LEN)
    payload[0] = CONFIG_PROTOCOL_VERSION
    payload[1] = SOURCE_KIND
    payload[2] = 1
    payload[3] = slot
    payload[4] = transport
    payload[5] = state
    payload[6] = flags
    payload[7] = profile_flags
    payload[8:14] = address
    payload[14] = len(name)
    payload[15 : 15 + len(name)] = name
    payload[28:] = struct.pack("<I", zlib.crc32(payload[:28]) & 0xFFFFFFFF)
    return bytes(payload)


def identity_block(major: int, minor: int, patch: int, build_id: int, dirty: int = 0) -> bytes:
    """Build an identity block the way `encode_identity` does."""

    payload = bytearray(CONFIG_REPORT_LEN)
    payload[0] = CONFIG_PROTOCOL_VERSION
    payload[1] = 4
    payload[2:5] = bytes((major, minor, patch))
    struct.pack_into("<I", payload, 5, build_id)
    payload[9] = dirty
    payload[28:] = struct.pack("<I", zlib.crc32(payload[:28]) & 0xFFFFFFFF)
    return bytes(payload)


def transfer_block(
    state: int,
    last_error: int = 0,
    target: int = 1,
    expected_len: int = 0,
    received_len: int = 0,
    next_index: int = 0,
    declared_crc: int = 0,
    computed_crc: int = 0,
) -> bytes:
    """Build a transfer block with the firmware's fixed offsets."""

    payload = bytearray(CONFIG_REPORT_LEN)
    payload[0] = CONFIG_PROTOCOL_VERSION
    payload[1] = TRANSFER_KIND
    payload[2] = state
    payload[3] = last_error
    payload[4] = target
    struct.pack_into("<H", payload, 6, expected_len)
    struct.pack_into("<H", payload, 8, received_len)
    struct.pack_into("<H", payload, 10, next_index)
    struct.pack_into("<I", payload, 12, declared_crc)
    struct.pack_into("<I", payload, 16, computed_crc)
    payload[28:] = struct.pack("<I", zlib.crc32(payload[:28]) & 0xFFFFFFFF)
    return bytes(payload)


def profile_block(
    flags: int,
    stored: int,
    pending: int = 0,
    record_version: int = 1,
    kind: int = PROFILE_KIND,
) -> bytes:
    """Build a profile block with the firmware's fixed offsets."""

    payload = bytearray(CONFIG_REPORT_LEN)
    payload[0] = CONFIG_PROTOCOL_VERSION
    payload[1] = kind
    payload[2] = record_version
    payload[3] = flags
    payload[4] = stored
    payload[5] = pending
    payload[28:] = struct.pack("<I", zlib.crc32(payload[:28]) & 0xFFFFFFFF)
    return bytes(payload)


class TransferBlockTest(unittest.TestCase):
    def test_idle_transfer_is_displayed(self):
        self.assertEqual(decode_transfer(transfer_block(0))["設定転送"], "設定転送: 待機中")

    def test_receiving_transfer_uses_fixed_offsets(self):
        decoded = decode_transfer(
            transfer_block(
                1,
                target=1,
                expected_len=512,
                received_len=128,
                next_index=6,
                declared_crc=0x11223344,
            )
        )
        self.assertEqual(decoded["設定転送"], "設定転送: 受信中 128/512バイト、次の断片 6")
        self.assertEqual(decoded["転送target"], 1)
        self.assertEqual(decoded["転送宣言CRC"], "0x11223344")

    def test_failed_transfer_names_the_error(self):
        decoded = decode_transfer(
            transfer_block(
                3,
                last_error=6,
                expected_len=512,
                received_len=512,
                computed_crc=0x55667788,
            )
        )
        self.assertEqual(decoded["設定転送"], "設定転送: 失敗 6 COMMITのCRCが一致しない")

    def test_corrupted_transfer_is_refused(self):
        block = bytearray(transfer_block(2, expected_len=3, received_len=3))
        block[8] ^= 1
        with self.assertRaises(DiagnosticsError):
            decode_transfer(bytes(block))


class ProfileBlockTest(unittest.TestCase):
    def test_saved_us_jis_is_displayed(self):
        decoded = decode_profile(profile_block(0x01, 1))
        self.assertEqual(decoded["プロファイル"], "US→JIS（保存済み）")

    def test_builtin_us_jis_caps_to_ctrl_is_displayed(self):
        decoded = decode_profile(profile_block(0x03, 0))
        self.assertEqual(decoded["プロファイル"], "US→JIS / Caps→Ctrl（組み込みの既定）")

    def test_saved_transparent_profile_is_displayed(self):
        decoded = decode_profile(profile_block(0x00, 1))
        self.assertEqual(decoded["プロファイル"], "変換なし（保存済み）")

    def test_pending_transparent_profile_uses_the_pending_wire_byte(self):
        decoded = decode_profile(profile_block(0x00, 1, pending=1))
        self.assertEqual(decoded["プロファイル"], "変換なし（保存待ち）")

    def test_pending_profile_display_does_not_use_stored_source(self):
        decoded = decode_profile(profile_block(0x01, 0, pending=1))
        self.assertEqual(decoded["プロファイル"], "US→JIS（保存待ち）")

    def test_alt_gui_flag_is_displayed(self):
        decoded = decode_profile(profile_block(0x04, 1))
        self.assertEqual(decoded["プロファイル"], "Alt↔GUI（保存済み）")

    def test_corrupted_profile_is_refused(self):
        block = bytearray(profile_block(0x01, 1))
        block[3] ^= 1
        with self.assertRaises(DiagnosticsError):
            decode_profile(bytes(block))

    def test_profile_version_kind_and_flags_are_validated(self):
        with self.assertRaises(DiagnosticsError):
            decode_profile(profile_block(0x01, 1, record_version=2))
        with self.assertRaises(DiagnosticsError):
            decode_profile(profile_block(0x08, 1))
        with self.assertRaises(DiagnosticsError):
            decode_profile(profile_block(0x01, 1, kind=5))

    def test_profile_pending_flag_is_validated(self):
        with self.assertRaisesRegex(DiagnosticsError, "invalid profile pending flag 2"):
            decode_profile(profile_block(0x00, 0, pending=2))

    def test_profile_bytes_after_pending_remain_reserved(self):
        block = bytearray(profile_block(0x00, 0))
        block[6] = 1
        block[28:] = struct.pack("<I", zlib.crc32(block[:28]) & 0xFFFFFFFF)
        with self.assertRaisesRegex(DiagnosticsError, "non-zero reserved bytes"):
            decode_profile(bytes(block))


class IdentityBlockTest(unittest.TestCase):
    def test_normal_identity_is_displayed(self):
        decoded = decode_identity(identity_block(1, 0, 0, 0x1A2B3C4D))
        self.assertEqual(decoded["ファーム版"], "1.0.0 (build 1a2b3c4d)")

    def test_dirty_identity_is_displayed(self):
        decoded = decode_identity(identity_block(1, 0, 0, 0x1A2B3C4D, dirty=1))
        self.assertEqual(decoded["ファーム版"], "1.0.0 (build 1a2b3c4d, 作業ツリーに変更あり)")

    def test_unknown_build_is_displayed(self):
        decoded = decode_identity(identity_block(1, 0, 0, 0))
        self.assertEqual(decoded["ファーム版"], "1.0.0 (build 不明)")

    def test_corrupted_identity_is_refused(self):
        block = bytearray(identity_block(1, 0, 0, 0x1A2B3C4D))
        block[5] ^= 1
        with self.assertRaises(DiagnosticsError):
            decode_identity(bytes(block))


class SourceBlockTest(unittest.TestCase):
    def test_virtual_slot_decodes_the_fixed_wire_bytes(self):
        block = bytes.fromhex(
            "12 07 01 04 03 05 00 05 00 00 00 00 00 00 0d "
            "56 69 72 74 75 61 6c 20 49 6e 70 75 74 cb 7c a5 d8"
        )

        decoded = decode_source(block)

        self.assertEqual(decoded["slot"], 4)
        self.assertEqual(decoded["transport"], "Virtual")
        self.assertEqual(decoded["state"], "接続済み")
        self.assertEqual(decoded["name"], "Virtual Input")
        self.assertFalse(decoded["irk_present"])
        self.assertIsNone(decoded["identity_address"])

    def test_unknown_slot_reserved_bytes_and_crc_are_rejected(self):
        unknown = bytearray(source_block())
        unknown[3] = 5
        unknown[28:] = struct.pack("<I", zlib.crc32(unknown[:28]) & 0xFFFFFFFF)
        with self.assertRaises(DiagnosticsError):
            decode_source(bytes(unknown))

        reserved = bytearray(source_block())
        reserved[6] = 0x80
        reserved[28:] = struct.pack("<I", zlib.crc32(reserved[:28]) & 0xFFFFFFFF)
        with self.assertRaises(DiagnosticsError):
            decode_source(bytes(reserved))

        corrupt = bytearray(source_block())
        corrupt[7] ^= 1
        with self.assertRaises(DiagnosticsError):
            decode_source(bytes(corrupt))

    def test_irk_without_an_identity_address_is_rejected(self):
        invalid = bytearray(source_block(flags=0x02))
        invalid[28:] = struct.pack("<I", zlib.crc32(invalid[:28]) & 0xFFFFFFFF)

        with self.assertRaisesRegex(DiagnosticsError, "IRK"):
            decode_source(bytes(invalid))

    def test_non_identity_block_is_refused(self):
        block = bytearray(identity_block(1, 0, 0, 0x1A2B3C4D))
        block[1] = 3
        block[28:] = struct.pack("<I", zlib.crc32(block[:28]) & 0xFFFFFFFF)
        with self.assertRaises(DiagnosticsError):
            decode_identity(bytes(block))


class FramingTest(unittest.TestCase):
    def test_wrong_report_id_prefix_is_refused_before_crc_guessing(self):
        class Device:
            def get_feature_report(self, report_id: int, length: int) -> bytes:
                self.report_id = report_id
                self.length = length
                return bytes([CONFIG_REPORT_ID + 1]) + source_block()

        with self.assertRaisesRegex(DiagnosticsError, "report ID"):
            _read_block(Device())

    def test_oversized_report_is_refused_before_crc_guessing(self):
        class Device:
            def get_feature_report(self, report_id: int, length: int) -> bytes:
                return bytes([CONFIG_REPORT_ID]) + source_block() + b"\x00"

        with self.assertRaisesRegex(DiagnosticsError, "report length"):
            _read_block(Device())


def security_block(**fields: int) -> bytes:
    """Build a security block the way `encode_passkey` does."""

    payload = bytearray(CONFIG_REPORT_LEN)
    payload[0] = CONFIG_PROTOCOL_VERSION
    payload[1] = PASSKEY_KIND
    payload[12] = fields.get("bond_flags", 0)
    payload[13] = fields.get("hogp_refusal", 0)
    payload[14] = fields.get("notify_refusal", 0)
    payload[15] = fields.get("bond_flash_error", 0)
    struct.pack_into("<H", payload, 16, fields.get("notify_handle", 0))
    struct.pack_into("<H", payload, 18, fields.get("notify_expected", 0))
    struct.pack_into("<H", payload, 20, fields.get("bond_flash_errno", 0))
    payload[22] = fields.get("usb_hogp_refusal", 0)
    payload[23] = fields.get("link_setup_failed", 0)
    payload[24] = fields.get("stuck_key_releases", 0)
    payload[25] = fields.get("liveness_probe_failures", 0)
    payload[26] = fields.get("liveness_probe_error", 0)
    payload[27] = fields.get("inject_refusal", 0)
    payload[28:] = struct.pack("<I", zlib.crc32(payload[:28]) & 0xFFFFFFFF)
    return bytes(payload)


class SecurityBlockTest(unittest.TestCase):
    def test_stuck_key_watchdog_fields_are_read_from_their_own_bytes(self):
        block = security_block(
            stuck_key_releases=3,
            liveness_probe_failures=2,
            liveness_probe_error=0x58,
        )

        decoded = decode_security(block)

        self.assertIn("全解放3回", decoded["押しっぱなし対策"])
        self.assertIn("無応答2回", decoded["押しっぱなし対策"])
        # 0x58 is the subscribed phase with a timeout. Reading it as discovery
        # would point the next person at the nine discovery steps, none of
        # which were running.
        self.assertIn("購読中", decoded["押しっぱなし対策"])
        self.assertIn("タイムアウト", decoded["押しっぱなし対策"])

    def test_a_quiet_watchdog_says_so_rather_than_printing_zeros(self):
        decoded = decode_security(security_block())

        self.assertEqual(decoded["押しっぱなし対策"], "発動なし")

    def test_bond_flags_are_named(self):
        decoded = decode_security(security_block(bond_flags=0x05))

        self.assertIn("起動時に鍵を読み込んだ", decoded["ボンド状態"])
        self.assertIn("直近の相手はボンド済み", decoded["ボンド状態"])
        self.assertNotIn("鍵の書き込みに失敗した", decoded["ボンド状態"])

    def test_a_refused_injection_is_named(self):
        # This is the one that hid for an afternoon: the tool said it sent the
        # packet, the host received nothing, and the firmware knew why but had
        # no way to say it.
        decoded = decode_security(security_block(inject_refusal=2))

        self.assertEqual(decoded["注入の拒否理由"], "源が切り離されている")

    def test_a_forwarded_injection_says_nothing_happened(self):
        self.assertEqual(decode_security(security_block())["注入の拒否理由"], "なし")

    def test_a_corrupted_block_is_refused_rather_than_decoded(self):
        block = bytearray(security_block(stuck_key_releases=1))
        block[24] ^= 1

        with self.assertRaises(DiagnosticsError):
            decode_security(bytes(block))

    def test_a_status_block_is_not_mistaken_for_a_security_block(self):
        block = bytearray(security_block())
        block[1] = 1
        block[28:] = struct.pack("<I", zlib.crc32(block[:28]) & 0xFFFFFFFF)

        with self.assertRaises(DiagnosticsError):
            decode_security(bytes(block))


class DescriptionTest(unittest.TestCase):
    def test_no_bond_reads_as_a_sentence(self):
        self.assertEqual(describe_bond_flags(0), "0x00 保存された鍵は無い")

    def test_a_release_without_a_failed_probe_is_still_reported(self):
        # The watchdog can fire without a probe ever failing: the report task
        # is the backstop for a radio half that never noticed anything.
        self.assertIn("全解放1回", describe_stuck_key(1, 0, 0))


if __name__ == "__main__":
    unittest.main()
