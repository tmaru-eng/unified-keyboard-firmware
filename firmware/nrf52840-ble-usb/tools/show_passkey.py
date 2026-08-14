# /// script
# requires-python = ">=3.11"
# dependencies = ["hidapi"]
# ///
"""Watch for the pairing passkey and print it as soon as it appears.

The keyboard types the passkey and cannot show one, so the bridge displays it.
The bridge has no screen, so it goes out over the configuration interface and
arrives here.

The passkey is only good for the connection that produced it. Polling has to be
fast enough that the number reaches the keyboard while that connection is still
up, which is why this prints immediately rather than on a refresh interval.
"""

import struct
import sys
import time
import zlib

import hid

CONFIG_REPORT_ID = 100
CONFIG_REPORT_LEN = 32
CONFIG_PROTOCOL_VERSION = 18
UKF_SELECT_PANIC_CHUNK = 101
SELECT_PASSKEY = 0xFE
PASSKEY_KIND = 3
VID, PID = 0x1209, 0x0001


def open_config_interface():
    """Selects the vendor configuration interface, never guessing between HIDs."""
    for info in hid.enumerate(VID, PID):
        if info.get("usage_page") == 0xFF00 and info.get("usage") == 0x20:
            device = hid.device()
            device.open_path(info["path"])
            return device
    raise SystemExit("設定インターフェースが見つかりません")


def crc_ok(payload: bytes) -> bool:
    (expected,) = struct.unpack_from("<I", payload, 28)
    return expected == (zlib.crc32(payload[:28]) & 0xFFFFFFFF)


def select_passkey(device) -> None:
    payload = bytearray(CONFIG_REPORT_LEN)
    payload[0] = CONFIG_PROTOCOL_VERSION
    payload[1] = UKF_SELECT_PANIC_CHUNK
    payload[2] = SELECT_PASSKEY
    payload[28:] = struct.pack("<I", zlib.crc32(payload[:28]) & 0xFFFFFFFF)
    device.send_feature_report(bytes([CONFIG_REPORT_ID]) + bytes(payload))


def read_block(device) -> bytes | None:
    raw = bytes(device.get_feature_report(CONFIG_REPORT_ID, CONFIG_REPORT_LEN + 1))
    # hidapi may or may not keep the report ID in the returned packet; let the
    # CRC decide which framing is real rather than guessing per platform.
    for candidate in (raw[:CONFIG_REPORT_LEN], raw[1 : 1 + CONFIG_REPORT_LEN]):
        if len(candidate) == CONFIG_REPORT_LEN and crc_ok(candidate):
            return candidate
    return None


def main() -> int:
    device = open_config_interface()
    print("パスキー待機中。キーボードをペアリングモードにしてください。Ctrl+Cで終了。", flush=True)
    last_serial = None
    last_failure = None
    try:
        while True:
            select_passkey(device)
            block = read_block(device)
            if block is not None and block[1] == PASSKEY_KIND:
                disconnect_reason, pairing_failure = block[2], block[3]
                passkey, serial = struct.unpack_from("<II", block, 4)
                if serial and serial != last_serial:
                    last_serial = serial
                    print(f"\n  パスキー: {passkey:06d}\n", flush=True)
                    print("  キーボードでこの6桁を打ってEnter。", flush=True)
                # The peer's own account of what it objected to. Printed only on
                # change so a repeating failure does not bury a new one.
                failure = (disconnect_reason, pairing_failure)
                if failure != (0, 0) and failure != last_failure:
                    last_failure = failure
                    print(
                        f"切断理由 HCI 0x{disconnect_reason:02x} / "
                        f"ペアリング失敗 0x{pairing_failure:02x}",
                        flush=True,
                    )
            time.sleep(0.15)
    except KeyboardInterrupt:
        return 0
    finally:
        device.close()


if __name__ == "__main__":
    sys.exit(main())
