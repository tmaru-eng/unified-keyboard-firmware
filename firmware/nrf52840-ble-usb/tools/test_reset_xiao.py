import unittest

from reset_xiao import (
    CONFIG_REPORT_ID,
    CONFIG_REPORT_LEN,
    CONFIG_PROTOCOL_VERSION,
    CONFIG_USAGE,
    CONFIG_USAGE_PAGE,
    LEGACY_PRODUCT_ID,
    LEGACY_VENDOR_ID,
    NEW_PRODUCT_ID,
    NEW_VENDOR_ID,
    RESET_INTO_BOOTSEL,
    build_reset_report,
    ensure_report_sent,
    find_bridge,
)


def interface(
    vendor_id=LEGACY_VENDOR_ID,
    product_id=LEGACY_PRODUCT_ID,
    usage_page=CONFIG_USAGE_PAGE,
    usage=CONFIG_USAGE,
    path=b"config",
):
    return {
        "vendor_id": vendor_id,
        "product_id": product_id,
        "usage_page": usage_page,
        "usage": usage,
        "path": path,
    }


class ResetXiaoTests(unittest.TestCase):
    def test_report_send_requires_the_complete_hidapi_length(self):
        ensure_report_sent(CONFIG_REPORT_LEN + 1)

    def test_report_send_rejects_hidapi_failure(self):
        with self.assertRaisesRegex(RuntimeError, "returned -1"):
            ensure_report_sent(-1)

    def test_packet_is_hidapi_report_id_plus_32_byte_payload(self):
        report = build_reset_report()
        self.assertEqual(len(report), CONFIG_REPORT_LEN + 1)
        self.assertEqual(report[0], CONFIG_REPORT_ID)

    def test_packet_contains_protocol_version_and_reset_command(self):
        report = build_reset_report()
        self.assertEqual(report[1], CONFIG_PROTOCOL_VERSION)
        self.assertEqual(report[2], RESET_INTO_BOOTSEL)

    def test_packet_crc_matches_the_factory_contract(self):
        self.assertEqual(build_reset_report()[29:], bytes.fromhex("42 47 c8 f6"))

    def test_device_selection_accepts_the_legacy_identity(self):
        matching = interface()
        self.assertIs(find_bridge([matching]), matching)

    def test_device_selection_accepts_the_new_identity(self):
        matching = interface(NEW_VENDOR_ID, NEW_PRODUCT_ID, path=b"new-config")
        self.assertIs(find_bridge([matching]), matching)

    def test_device_selection_requires_the_vendor_usage_page(self):
        with self.assertRaisesRegex(RuntimeError, "found 0"):
            find_bridge([interface(usage_page=0x0001)])

    def test_device_selection_requires_the_config_usage(self):
        with self.assertRaisesRegex(RuntimeError, "found 0"):
            find_bridge([interface(usage=1)])

    def test_device_selection_rejects_an_unapproved_vendor(self):
        with self.assertRaisesRegex(RuntimeError, "found 0"):
            find_bridge([interface(vendor_id=0xCAFE + 1)])

    def test_device_selection_rejects_an_unapproved_product(self):
        with self.assertRaisesRegex(RuntimeError, "found 0"):
            find_bridge([interface(product_id=0xBAF2 + 1)])

    def test_device_selection_rejects_string_identifiers(self):
        with self.assertRaisesRegex(RuntimeError, "found 0"):
            find_bridge([interface(vendor_id="0xCAFE")])

    def test_device_selection_requires_usage_fields(self):
        missing_usage = interface()
        del missing_usage["usage"]
        with self.assertRaisesRegex(RuntimeError, "found 0"):
            find_bridge([missing_usage])

    def test_device_selection_rejects_duplicate_legacy_interfaces(self):
        with self.assertRaisesRegex(RuntimeError, "found 2"):
            find_bridge([interface(path=b"first"), interface(path=b"second")])

    def test_device_selection_rejects_duplicate_new_interfaces(self):
        with self.assertRaisesRegex(RuntimeError, "found 2"):
            find_bridge(
                [
                    interface(NEW_VENDOR_ID, NEW_PRODUCT_ID, path=b"first"),
                    interface(NEW_VENDOR_ID, NEW_PRODUCT_ID, path=b"second"),
                ]
            )

    def test_device_selection_rejects_mixed_legacy_and_new_interfaces(self):
        with self.assertRaisesRegex(RuntimeError, "found 2"):
            find_bridge(
                [
                    interface(path=b"legacy"),
                    interface(NEW_VENDOR_ID, NEW_PRODUCT_ID, path=b"new"),
                ]
            )

    def test_device_selection_rejects_empty_enumeration(self):
        with self.assertRaisesRegex(RuntimeError, "found 0"):
            find_bridge([])


if __name__ == "__main__":
    unittest.main()
