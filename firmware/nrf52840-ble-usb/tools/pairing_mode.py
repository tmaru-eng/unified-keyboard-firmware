# /// script
# requires-python = ">=3.11"
# dependencies = ["hidapi"]
# ///
"""Open or close pairing, and choose how the next pairing authenticates.

Once a keyboard is bonded the bridge waits for that one alone, so adopting a
different keyboard has to be asked for. Without this the only way to change
keyboards would be to reflash.

    uv run pairing_mode.py on
    uv run pairing_mode.py off
    uv run pairing_mode.py passkey     # require a passkey for the next pairing
    uv run pairing_mode.py justworks   # pair with whoever answers

Just Works offers no protection against someone in radio range completing the
pairing in the keyboard's place. It is a bring-up convenience. Passkey entry
closes that, but the number has to reach a person within the thirty seconds
the specification allows for pairing, so read it with `show_passkey.py` running
before opening pairing — relaying it by hand does not fit in thirty seconds.
"""

import struct
import sys
import zlib

import hid

CONFIG_REPORT_ID = 100
CONFIG_REPORT_LEN = 32
CONFIG_PROTOCOL_VERSION = 18
UKF_SET_PAIRING_MODE = 102
UKF_SET_PAIRING_METHOD = 103
VID, PID = 0x1209, 0x0001


def main() -> int:
    actions = ("on", "off", "passkey", "justworks")
    if len(sys.argv) != 2 or sys.argv[1] not in actions:
        print("使い方: uv run pairing_mode.py on|off|passkey|justworks")
        return 2
    action = sys.argv[1]

    # Never guess between HID devices; the configuration interface is the one
    # with the vendor usage page, and sending this to the keyboard interface
    # would be a keystroke rather than a command.
    device = None
    for info in hid.enumerate(VID, PID):
        if info.get("usage_page") == 0xFF00 and info.get("usage") == 0x20:
            device = hid.device()
            device.open_path(info["path"])
            break
    if device is None:
        print("設定インターフェースが見つかりません")
        return 1

    payload = bytearray(CONFIG_REPORT_LEN)
    payload[0] = CONFIG_PROTOCOL_VERSION
    if action in ("on", "off"):
        payload[1] = UKF_SET_PAIRING_MODE
        payload[2] = 1 if action == "on" else 0
    else:
        payload[1] = UKF_SET_PAIRING_METHOD
        payload[2] = 1 if action == "passkey" else 0
    payload[28:] = struct.pack("<I", zlib.crc32(payload[:28]) & 0xFFFFFFFF)
    try:
        device.send_feature_report(bytes([CONFIG_REPORT_ID]) + bytes(payload))
    finally:
        device.close()

    messages = {
        "on": "ペアリングモードを開きました。キーボード側もペアリングにしてください。",
        "off": "ペアリングモードを閉じました。ボンド済みのキーボードだけに接続します。",
        "passkey": "次のペアリングはパスキーを要求します。show_passkey.py で番号を見てください。",
        "justworks": "次のペアリングはJust Worksです。電波の届く範囲の相手を拒めません。",
    }
    print(messages[action])
    return 0


if __name__ == "__main__":
    sys.exit(main())
