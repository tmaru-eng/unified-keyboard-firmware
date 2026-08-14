# /// script
# requires-python = ">=3.11"
# dependencies = ["hidapi"]
# ///
"""Checks on real hardware that a transfer the board cannot verify is refused.

Every other test of this path proves the happy case. The property the shape
exists for is the other one: a configuration half-written to a keyboard bridge
is not a lesser failure than none, it is the one with no way back. So the board
has to refuse, and it has to say which refusal it was.

    uv run transfer_refusal_check.py
"""

from __future__ import annotations

import struct
import zlib

import hid

from read_diagnostics import SELECT_TRANSFER, TRANSFER_ERROR, decode_transfer
from reset_xiao import (
    CONFIG_PROTOCOL_VERSION,
    CONFIG_REPORT_ID,
    CONFIG_REPORT_LEN,
    find_bridge,
)

UKF_SELECT_PANIC_CHUNK = 101
UKF_WRITE_BEGIN = 104
UKF_WRITE_CHUNK = 105
UKF_WRITE_COMMIT = 106
TARGET_SCRATCH = 1
CHUNK_BYTES = 23


def sealed(payload: bytearray) -> bytes:
    payload[28:] = struct.pack("<I", zlib.crc32(payload[:28]) & 0xFFFFFFFF)
    return bytes([CONFIG_REPORT_ID]) + bytes(payload)


def declaration(command: int, target: int, total_len: int, payload_crc: int) -> bytes:
    frame = bytearray(CONFIG_REPORT_LEN)
    frame[0] = CONFIG_PROTOCOL_VERSION
    frame[1] = command
    frame[2] = target
    struct.pack_into("<H", frame, 4, total_len)
    struct.pack_into("<I", frame, 6, payload_crc)
    return sealed(frame)


def chunk(index: int, data: bytes) -> bytes:
    frame = bytearray(CONFIG_REPORT_LEN)
    frame[0] = CONFIG_PROTOCOL_VERSION
    frame[1] = UKF_WRITE_CHUNK
    struct.pack_into("<H", frame, 2, index)
    frame[4] = len(data)
    frame[5 : 5 + len(data)] = data
    return sealed(frame)


def read_transfer(device) -> dict[str, object]:
    selector = bytearray(CONFIG_REPORT_LEN)
    selector[0] = CONFIG_PROTOCOL_VERSION
    selector[1] = UKF_SELECT_PANIC_CHUNK
    selector[2] = SELECT_TRANSFER
    device.send_feature_report(sealed(selector))
    raw = bytes(device.get_feature_report(CONFIG_REPORT_ID, CONFIG_REPORT_LEN + 1))
    for candidate in (raw[:CONFIG_REPORT_LEN], raw[1 : 1 + CONFIG_REPORT_LEN]):
        if len(candidate) == CONFIG_REPORT_LEN:
            try:
                return decode_transfer(candidate)
            except Exception:
                continue
    raise SystemExit("no framing produced a valid transfer block")


def case(device, name: str, steps, expected_error: int) -> bool:
    """Runs one refusal and checks which refusal it was.

    The block is rendered as a sentence rather than a number, so the check reads
    the sentence the expected error produces. Comparing against a key the
    decoder does not return would compare a dict to an int and call every
    correct refusal a failure -- which is exactly what the first version of this
    script did.
    """

    for frame in steps:
        device.send_feature_report(frame)
    result = read_transfer(device)
    sentence = str(result["設定転送"])
    ok = sentence.startswith(f"設定転送: 失敗 {expected_error} ")
    print(f"{'PASS' if ok else 'FAIL'}  {name}: {sentence}")
    if not ok:
        print(f"      期待した理由: {expected_error} {TRANSFER_ERROR.get(expected_error)}")
    return ok


def main() -> None:
    device = hid.device()
    device.open_path(find_bridge(hid.enumerate())["path"])
    payload = bytes((index * 7 + 3) & 0xFF for index in range(30))
    wrong = bytes([payload[0] ^ 0xFF]) + payload[1:]
    results = []
    try:
        # A commit whose CRC does not describe what arrived.
        results.append(
            case(
                device,
                "CRCが一致しないコミット",
                [
                    declaration(UKF_WRITE_BEGIN, TARGET_SCRATCH, len(payload), zlib.crc32(wrong) & 0xFFFFFFFF),
                    chunk(0, payload[:CHUNK_BYTES]),
                    chunk(1, payload[CHUNK_BYTES:]),
                    declaration(UKF_WRITE_COMMIT, TARGET_SCRATCH, len(payload), zlib.crc32(wrong) & 0xFFFFFFFF),
                ],
                6,
            )
        )
        # A chunk that skips a number.
        results.append(
            case(
                device,
                "番号の飛んだ断片",
                [
                    declaration(UKF_WRITE_BEGIN, TARGET_SCRATCH, len(payload), zlib.crc32(payload) & 0xFFFFFFFF),
                    chunk(1, payload[:CHUNK_BYTES]),
                ],
                2,
            )
        )
        # A commit before everything arrived.
        results.append(
            case(
                device,
                "全部届く前のコミット",
                [
                    declaration(UKF_WRITE_BEGIN, TARGET_SCRATCH, len(payload), zlib.crc32(payload) & 0xFFFFFFFF),
                    chunk(0, payload[:CHUNK_BYTES]),
                    declaration(UKF_WRITE_COMMIT, TARGET_SCRATCH, len(payload), zlib.crc32(payload) & 0xFFFFFFFF),
                ],
                5,
            )
        )
        # A declaration larger than the staging area.
        results.append(
            case(
                device,
                "容量を超える宣言",
                [declaration(UKF_WRITE_BEGIN, TARGET_SCRATCH, 513, 0)],
                4,
            )
        )
        # An unknown target.
        results.append(case(device, "未知の書き込み先", [declaration(UKF_WRITE_BEGIN, 9, 4, 0)], 8))

        # And after all that, a good transfer still works: the board is not
        # left wedged by a refusal.
        good = bytes(range(10))
        for frame in [
            declaration(UKF_WRITE_BEGIN, TARGET_SCRATCH, len(good), zlib.crc32(good) & 0xFFFFFFFF),
            chunk(0, good),
            declaration(UKF_WRITE_COMMIT, TARGET_SCRATCH, len(good), zlib.crc32(good) & 0xFFFFFFFF),
        ]:
            device.send_feature_report(frame)
        recovered = read_transfer(device)
        ok = "完了" in str(recovered["設定転送"])
        results.append(ok)
        print(f"{'PASS' if ok else 'FAIL'}  拒否のあとも正常な転送が通る: {recovered['設定転送']}")
    finally:
        device.close()

    print()
    print("すべて期待どおり" if all(results) else "期待と違う結果がある")


if __name__ == "__main__":
    main()
