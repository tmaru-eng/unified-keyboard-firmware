# /// script
# requires-python = ">=3.11"
# dependencies = ["hidapi"]
# ///
"""Read the bridge's diagnostics block over the configuration HID interface.

The firmware reports where the BLE central actually got to. Without this the
only signal from a failed bring-up is an LED, which cannot distinguish "never
saw an advertisement" from "connected but the peer refused to serve reports".

    uv run read_diagnostics.py            # one snapshot
    uv run read_diagnostics.py --watch    # poll until interrupted

Layout must match `firmware/nrf52840-ble-usb/src/diagnostics_report.rs`.
"""

from __future__ import annotations

import argparse
import struct
import time
import zlib

from reset_xiao import CONFIG_PROTOCOL_VERSION, CONFIG_REPORT_ID, CONFIG_REPORT_LEN, find_bridge

STATUS_KIND = 1
PANIC_TEXT_KIND = 2
PASSKEY_KIND = 3
IDENTITY_KIND = 4
TRANSFER_KIND = 5
PROFILE_KIND = 6
SOURCE_KIND = 7
PANIC_TEXT_CHUNK_LEN = 24
UKF_SELECT_PANIC_CHUNK = 101
UKF_SELECT_SOURCE = 107
SELECT_PASSKEY = 0xFE
SELECT_IDENTITY = 0xFD
SELECT_TRANSFER = 0xFC
SELECT_PROFILE = 0xFB
SELECT_SOURCE = 0xFA
SOURCE_RECORD_VERSION = 1
SOURCE_NAME_LEN = 13

SOURCE_TRANSPORT = {
    0: "Unregistered",
    1: "USB",
    2: "BLE",
    3: "Virtual",
}

SOURCE_STATE = {
    0: "Unregistered",
    1: "Disconnected",
    2: "Connecting",
    3: "Securing",
    4: "Discovering",
    5: "接続済み",
    6: "Failed",
}

TRANSFER_STATE = {
    0: "待機中",
    1: "受信中",
    2: "完了",
    3: "失敗",
}

TRANSFER_ERROR = {
    0: "なし",
    1: "BEGIN無しでCHUNKが来た",
    2: "indexが期待値と違う",
    3: "countが範囲外",
    4: "total_lenが容量を超える",
    5: "全バイト受信前にCOMMITが来た",
    6: "COMMITのCRCが一致しない",
    7: "COMMITのパラメータがBEGINと違う",
    8: "未知のtarget",
}

# The injection tool can only report that it handed the packet over; what the
# bridge did with it was invisible. A report refused for a detached source is
# exactly what made the stuck-key watchdog's own test look like a dead board.
INJECT_REFUSAL = {
    0: "なし",
    1: "レポート長が8バイトでない",
    2: "源が切り離されている",
    3: "源の番号が範囲外",
    4: "USB書き込みが拒否された",
}

BOND_FLAGS = {
    1 << 0: "起動時に鍵を読み込んだ",
    1 << 1: "この起動で鍵を保存した",
    1 << 2: "直近の相手はボンド済み",
    1 << 3: "ペアリングしたが鍵が得られなかった",
    1 << 4: "鍵の書き込みに失敗した",
}

BRIDGE_STATE = {
    0: "起動中",
    1: "スキャン中",
    2: "接続中",
    3: "暗号化中",
    4: "探索中",
    5: "購読済み",
    6: "失敗",
}

# The high nibble says which phase failed, the low nibble why. Guessing at a
# magic error number cost two wrong fixes during bring-up; the moment the real
# HCI status was reported the cause was obvious.
ERROR_PHASE = {
    0x10: "スキャン",
    0x20: "接続",
    0x30: "暗号化",
    0x40: "探索",
    0x50: "購読中",
    0x60: "プロファイル保存",
    0x70: "プロファイル反映",
}

# Bluetooth core specification HCI status codes, limited to those that fit the
# low nibble and actually occur on this path.
#
# 0x08 and 0x09 are not HCI statuses here. The firmware passes HCI statuses
# through unchanged but overloads those two for failures the host layer
# reports without one, so they are named for what the firmware means by them.
# Labelling 0x09 as the HCI "connection limit exceeded" read as a resource
# problem when the link had simply been dropped.
ERROR_REASON = {
    0x01: "不明なHCIコマンド",
    0x02: "不明な接続識別子",
    0x03: "見つからない（サービス/特性/記述子）",
    0x04: "ページタイムアウト",
    0x05: "ATTエラー（相手が属性へのアクセスを拒否）",
    0x06: "PIN/鍵が無い",
    0x07: "メモリ/バッファ不足",
    0x08: "タイムアウト",
    0x09: "リンク切断",
    0x0A: "未対応の操作",
    0x0B: "状態が不正",
    0x0C: "応答が想定外",
    0x0D: "コントローラ側の失敗",
    0x0E: "相手が拒否/切断",
    0x0F: "ホスト側の失敗",
}


# Named for the operation that was in flight, because six of these fail with
# the same "not found" and the error code cannot tell them apart.
DISCOVERY_STEP = {
    0: "未実行",
    1: "HIDサービス検索",
    2: "HIDサービスの選択",
    3: "レポートマップ特性の取得",
    4: "レポートマップの読み出し",
    5: "特性一覧の取得",
    6: "レポート参照記述子の読み出し",
    7: "購読対象の決定",
    8: "購読対象の特性を一覧から特定",
    9: "通知の購読",
}


# The profile phases do not carry HCI statuses. Their low nibble is this
# firmware's own numbering, and rendering it with the Bluetooth table produced
# "0x65 プロファイル保存 / ATTエラー" for a full request channel -- a sentence
# that sends the reader looking at the radio for a queue problem.
PROFILE_REASON = {
    0x01: "記録の長さが不正",
    0x02: "保存された記録が無い",
    0x03: "記録の版が違う",
    0x04: "記録のCRCが一致しない",
    0x05: "書き込み要求のキューが満杯",
    0x06: "未定義のフラグビット",
    0x07: "予約バイトがゼロでない",
}


def describe_error(code: int) -> str:
    """Render the packed phase/reason byte as text."""

    if code == 0:
        return "0（なし）"
    phase = ERROR_PHASE.get(code & 0xF0)
    if code & 0xF0 in (0x60, 0x70):
        reason = PROFILE_REASON.get(code & 0x0F)
        return f"{code:#04x} {phase} / {reason or '不明な理由'}"
    reason = ERROR_REASON.get(code & 0x0F)
    if phase is None and reason is None:
        return f"{code:#04x}（未定義）"
    return f"{code:#04x} {phase or '不明な段階'} / {reason or '不明な理由'}"


def describe_panic(line: int, count: int) -> str:
    """Render the panic record carried across the reset into the bootloader."""

    if count == 0:
        return "なし"
    # The file is not recorded, only the line. `PanicInfo::location` reports
    # wherever the panic actually happened, which is often inside a dependency,
    # so naming our own source here would point at the wrong place.
    return f"行{line}（{count}回、ファイル未記録）"


def describe_bond_flags(flags: int) -> str:
    """Render the bond bit field as the sentences it stands for."""

    if flags == 0:
        return "0x00 保存された鍵は無い"
    named = [text for bit, text in BOND_FLAGS.items() if flags & bit]
    return f"{flags:#04x} " + "／".join(named)


def describe_stuck_key(releases: int, probe_failures: int, probe_error: int) -> str:
    """Render the stuck-key watchdog's record.

    Silence here is the expected state. A non-zero release count means the
    keyboard stopped answering while the host was holding a key, and the bridge
    let go on its behalf — the failure this watchdog exists to end, now visible
    instead of only felt.
    """

    if releases == 0 and probe_failures == 0:
        return "発動なし"
    return (
        f"全解放{releases}回 / 生存確認の無応答{probe_failures}回"
        f" / 直近の失敗理由 {describe_error(probe_error)}"
    )


class DiagnosticsError(ValueError):
    """Raised when the device returned something that is not a status block."""


def decode(payload: bytes) -> dict[str, object]:
    """Validate and decode the 32-byte diagnostics block."""

    if len(payload) != CONFIG_REPORT_LEN:
        raise DiagnosticsError(f"expected {CONFIG_REPORT_LEN} bytes, got {len(payload)}")
    if payload[0] != CONFIG_PROTOCOL_VERSION:
        raise DiagnosticsError(f"unsupported protocol version {payload[0]}")
    if payload[1] != STATUS_KIND:
        raise DiagnosticsError(f"not a status block (kind {payload[1]})")

    expected = struct.unpack_from("<I", payload, 28)[0]
    actual = zlib.crc32(payload[:28]) & 0xFFFFFFFF
    if expected != actual:
        raise DiagnosticsError(f"CRC mismatch: sent {expected:#010x}, computed {actual:#010x}")

    state = payload[2]
    return {
        "状態": BRIDGE_STATE.get(state, f"未知({state})"),
        "直近エラー": describe_error(payload[3]),
        "広告観測数": struct.unpack_from("<H", payload, 4)[0],
        "HID広告数": struct.unpack_from("<H", payload, 6)[0],
        "直近アドレス": ":".join(f"{b:02x}" for b in reversed(payload[8:14])),
        "直近RSSI": struct.unpack_from("<b", payload, 14)[0],
        "接続回数": payload[15],
        "受信レポート数": struct.unpack_from("<I", payload, 16)[0],
        "USB転送数": struct.unpack_from("<I", payload, 20)[0],
        "直近パニック": describe_panic(
            struct.unpack_from("<H", payload, 24)[0], payload[26]
        ),
        "探索の到達点": f"{payload[27]} {DISCOVERY_STEP.get(payload[27], '未定義')}",
    }



def decode_security(payload: bytes) -> dict[str, object]:
    """Validate and decode the 32-byte pairing and report-path block.

    Layout must match `encode_passkey` in
    `firmware/nrf52840-ble-usb/src/diagnostics_report.rs`.
    """

    if payload[1] != PASSKEY_KIND:
        raise DiagnosticsError(f"not a security block (kind {payload[1]})")
    if not _crc_ok(payload):
        raise DiagnosticsError("security block failed its CRC")

    notify_handle, notify_expected = struct.unpack_from("<HH", payload, 16)
    return {
        "ボンド状態": describe_bond_flags(payload[12]),
        "押しっぱなし対策": describe_stuck_key(payload[24], payload[25], payload[26]),
        "購読の拒否理由": f"無線側 {payload[13]:#04x} / USB側 {payload[22]:#04x}",
        "通知の拒否理由": f"{payload[14]:#04x}（着信 {notify_handle}, 期待 {notify_expected}）",
        "鍵の書き込み": f"{payload[15]:#04x} errno {struct.unpack_from('<H', payload, 20)[0]}",
        "注入の拒否理由": INJECT_REFUSAL.get(payload[27], f"未定義({payload[27]})"),
    }


def decode_transfer(payload: bytes) -> dict[str, object]:
    """Validate and render the configuration transfer block."""

    if len(payload) != CONFIG_REPORT_LEN:
        raise DiagnosticsError(f"expected {CONFIG_REPORT_LEN} bytes, got {len(payload)}")
    if payload[0] != CONFIG_PROTOCOL_VERSION:
        raise DiagnosticsError(f"unsupported protocol version {payload[0]}")
    if payload[1] != TRANSFER_KIND:
        raise DiagnosticsError(f"not a transfer block (kind {payload[1]})")
    if not _crc_ok(payload):
        raise DiagnosticsError("transfer block failed its CRC")

    state = payload[2]
    last_error = payload[3]
    target = payload[4]
    expected_len = struct.unpack_from("<H", payload, 6)[0]
    received_len = struct.unpack_from("<H", payload, 8)[0]
    next_index = struct.unpack_from("<H", payload, 10)[0]
    declared_crc = struct.unpack_from("<I", payload, 12)[0]
    computed_crc = struct.unpack_from("<I", payload, 16)[0]

    if state == 0:
        description = "設定転送: 待機中"
    elif state == 1:
        description = (
            f"設定転送: 受信中 {received_len}/{expected_len}バイト、"
            f"次の断片 {next_index}"
        )
    elif state == 2:
        description = f"設定転送: 完了 {received_len}バイト"
    elif state == 3:
        reason = TRANSFER_ERROR.get(last_error, f"未知の失敗({last_error})")
        description = f"設定転送: 失敗 {last_error} {reason}"
    else:
        description = f"設定転送: 未知の状態({state})"

    return {
        "設定転送": description,
        "転送target": target,
        "転送宣言CRC": f"{declared_crc:#010x}",
        "転送計算CRC": f"{computed_crc:#010x}",
    }


def describe_profile_flags(flags: int, stored: bool, pending: bool) -> str:
    """Render profile flags and their persistence state."""

    labels = []
    if flags & 0x01:
        labels.append("US→JIS")
    if flags & 0x02:
        labels.append("Caps→Ctrl")
    if flags & 0x04:
        labels.append("Alt↔GUI")
    description = " / ".join(labels) if labels else "変換なし"
    if pending:
        source = "保存待ち"
    else:
        source = "保存済み" if stored else "組み込みの既定"
    return f"{description}（{source}）"


def decode_profile(payload: bytes) -> dict[str, object]:
    """Validate and render the 32-byte compatibility profile block."""

    if len(payload) != CONFIG_REPORT_LEN:
        raise DiagnosticsError(f"expected {CONFIG_REPORT_LEN} bytes, got {len(payload)}")
    if payload[0] != CONFIG_PROTOCOL_VERSION:
        raise DiagnosticsError(f"unsupported protocol version {payload[0]}")
    if payload[1] != PROFILE_KIND:
        raise DiagnosticsError(f"not a profile block (kind {payload[1]})")
    if not _crc_ok(payload):
        raise DiagnosticsError("profile block failed its CRC")
    if payload[2] != 1:
        raise DiagnosticsError(f"unsupported profile record version {payload[2]}")
    if payload[3] & ~0x07:
        raise DiagnosticsError(f"profile has undefined flags {payload[3]:#04x}")
    if payload[4] not in (0, 1):
        raise DiagnosticsError(f"invalid profile stored flag {payload[4]}")
    if payload[5] not in (0, 1):
        raise DiagnosticsError(f"invalid profile pending flag {payload[5]}")
    if any(payload[6:28]):
        raise DiagnosticsError("profile block has non-zero reserved bytes")

    return {
        "プロファイル": describe_profile_flags(
            payload[3], bool(payload[4]), bool(payload[5])
        )
    }


def decode_source(payload: bytes) -> dict[str, object]:
    """Validate and decode one source slot's fixed 32-byte block.

    The IRK is intentionally reduced to a presence bit. Returning its bytes
    here would make a diagnostics read a secret extractor.
    """

    if len(payload) != CONFIG_REPORT_LEN:
        raise DiagnosticsError(f"expected {CONFIG_REPORT_LEN} bytes, got {len(payload)}")
    if payload[0] != CONFIG_PROTOCOL_VERSION:
        raise DiagnosticsError(f"unsupported protocol version {payload[0]}")
    if payload[1] != SOURCE_KIND:
        raise DiagnosticsError(f"not a source block (kind {payload[1]})")
    if payload[2] != SOURCE_RECORD_VERSION:
        raise DiagnosticsError(f"unsupported source record version {payload[2]}")
    if not _crc_ok(payload):
        raise DiagnosticsError("source block failed its CRC")

    slot = payload[3]
    if slot >= 5:
        raise DiagnosticsError(f"unknown source slot {slot}")
    transport_code = payload[4]
    if transport_code not in SOURCE_TRANSPORT:
        raise DiagnosticsError(f"unknown source transport {transport_code}")
    state_code = payload[5]
    if state_code not in SOURCE_STATE:
        raise DiagnosticsError(f"unknown source state {state_code}")
    flags = payload[6]
    if flags & ~0x03:
        raise DiagnosticsError(f"source has reserved flags {flags:#04x}")
    profile_flags = payload[7]
    if profile_flags & ~0x07:
        raise DiagnosticsError(f"source has undefined profile flags {profile_flags:#04x}")
    if (transport_code == 0) != (state_code == 0):
        raise DiagnosticsError("source transport and lifecycle state disagree")

    address = bytes(payload[8:14])
    has_address = bool(flags & 0x01)
    if not has_address and any(address):
        raise DiagnosticsError("an absent identity address must be zero-filled")
    if not has_address and flags & 0x02:
        raise DiagnosticsError("an IRK cannot be present without an identity address")
    name_length = payload[14]
    if name_length > SOURCE_NAME_LEN:
        raise DiagnosticsError(f"source name is too long: {name_length}")
    if any(payload[15 + name_length : 28]):
        raise DiagnosticsError("source name padding is not zero-filled")
    if state_code == 0 and (flags or profile_flags or name_length):
        raise DiagnosticsError("unregistered source carries registered fields")
    try:
        name = payload[15 : 15 + name_length].decode("utf-8")
    except UnicodeDecodeError as error:
        raise DiagnosticsError("source name is not valid UTF-8") from error

    return {
        "slot": slot,
        "transport": SOURCE_TRANSPORT[transport_code],
        "transport_code": transport_code,
        "state": SOURCE_STATE[state_code],
        "state_code": state_code,
        "identity_address": (
            ":".join(f"{byte:02x}" for byte in reversed(address)) if has_address else None
        ),
        "irk_present": bool(flags & 0x02),
        "profile_flags": profile_flags,
        "name": name,
    }


def decode_identity(payload: bytes) -> dict[str, object]:
    """Validate and decode the 32-byte firmware identity block."""

    if len(payload) != CONFIG_REPORT_LEN:
        raise DiagnosticsError(f"expected {CONFIG_REPORT_LEN} bytes, got {len(payload)}")
    if payload[0] != CONFIG_PROTOCOL_VERSION:
        raise DiagnosticsError(f"unsupported protocol version {payload[0]}")
    if payload[1] != IDENTITY_KIND:
        raise DiagnosticsError(f"not an identity block (kind {payload[1]})")
    if not _crc_ok(payload):
        raise DiagnosticsError("identity block failed its CRC")

    major, minor, patch = payload[2:5]
    build_id = struct.unpack_from("<I", payload, 5)[0]
    dirty = payload[9] != 0
    version = f"{major}.{minor}.{patch}"
    if build_id == 0:
        text = f"{version} (build 不明)"
    else:
        text = f"{version} (build {build_id:08x}"
        if dirty:
            text += ", 作業ツリーに変更あり"
        text += ")"
    return {"ファーム版": text}


def _crc_ok(payload: bytes) -> bool:
    expected = struct.unpack_from("<I", payload, 28)[0]
    return expected == (zlib.crc32(payload[:28]) & 0xFFFFFFFF)


def _select_chunk(device, chunk: int) -> None:
    """Ask the firmware which slice of the panic message to return next."""

    payload = bytearray(CONFIG_REPORT_LEN)
    payload[0] = CONFIG_PROTOCOL_VERSION
    payload[1] = UKF_SELECT_PANIC_CHUNK
    payload[2] = chunk
    crc = zlib.crc32(payload[:28]) & 0xFFFFFFFF
    payload[28:] = struct.pack("<I", crc)
    device.send_feature_report(bytes([CONFIG_REPORT_ID]) + bytes(payload))


def build_select_source_packet(slot: int) -> bytes:
    """Build the strict selector for one of the five source slots."""

    if not isinstance(slot, int) or isinstance(slot, bool) or not 0 <= slot < 5:
        raise ValueError(f"source slot must be an integer from 0 through 4: {slot}")
    payload = bytearray(CONFIG_REPORT_LEN)
    payload[0] = CONFIG_PROTOCOL_VERSION
    payload[1] = UKF_SELECT_SOURCE
    payload[2] = slot
    payload[28:] = struct.pack("<I", zlib.crc32(payload[:28]) & 0xFFFFFFFF)
    return bytes([CONFIG_REPORT_ID]) + bytes(payload)


def read_source(device, slot: int) -> dict[str, object]:
    """Select and decode one source slot through the real HID path."""

    device.send_feature_report(build_select_source_packet(slot))
    return decode_source(_read_block(device))


def _read_block(device) -> bytes:
    raw = bytes(device.get_feature_report(CONFIG_REPORT_ID, CONFIG_REPORT_LEN + 1))
    if len(raw) not in (CONFIG_REPORT_LEN, CONFIG_REPORT_LEN + 1):
        raise DiagnosticsError(
            f"unexpected diagnostics report length {len(raw)} "
            f"(expected {CONFIG_REPORT_LEN} or {CONFIG_REPORT_LEN + 1})"
        )
    if len(raw) == CONFIG_REPORT_LEN + 1 and raw[0] != CONFIG_REPORT_ID:
        raise DiagnosticsError(
            f"unexpected report ID prefix {raw[0]} (expected {CONFIG_REPORT_ID})"
        )
    for candidate in (raw[:CONFIG_REPORT_LEN], raw[1 : 1 + CONFIG_REPORT_LEN]):
        if len(candidate) == CONFIG_REPORT_LEN and _crc_ok(candidate):
            return candidate
    raise DiagnosticsError("no framing produced a valid block")


def read_panic_message(device) -> str:
    """Assemble the recorded panic message from its slices."""

    parts = []
    for chunk in range(4):
        _select_chunk(device, chunk)
        block = _read_block(device)
        if block[1] != PANIC_TEXT_KIND:
            break
        length = block[3]
        if length == 0:
            break
        parts.append(block[4 : 4 + length])
        if length < PANIC_TEXT_CHUNK_LEN:
            break
    return b"".join(parts).decode("utf-8", errors="replace")


def read_once() -> dict[str, object]:
    """Open the configuration interface and pull one status block."""

    try:
        import hid  # type: ignore[import-not-found]
    except ImportError as error:
        raise RuntimeError("run this with: uv run read_diagnostics.py") from error

    device_info = find_bridge(hid.enumerate())
    device_type = getattr(hid, "Device", None)
    if device_type is not None:
        device = device_type(path=device_info["path"])
    else:
        device = hid.device()
        device.open_path(device_info["path"])
    sources: list[dict[str, object]] = []
    try:
        raw = device.get_feature_report(CONFIG_REPORT_ID, CONFIG_REPORT_LEN + 1)
        message = read_panic_message(device)
        # The pairing and report-path block used to be readable only through
        # `show_passkey.py`, which prints the passkey and nothing else. The
        # bond state and the stuck-key watchdog live in it too, and neither was
        # visible from any tool.
        _select_chunk(device, SELECT_PASSKEY)
        try:
            security = decode_security(_read_block(device))
        except DiagnosticsError:
            security = {}
        _select_chunk(device, SELECT_IDENTITY)
        try:
            identity = decode_identity(_read_block(device))
        except DiagnosticsError:
            identity = {}
        _select_chunk(device, SELECT_TRANSFER)
        try:
            transfer = decode_transfer(_read_block(device))
        except DiagnosticsError:
            transfer = {}
        _select_chunk(device, SELECT_PROFILE)
        try:
            profile = decode_profile(_read_block(device))
        except DiagnosticsError:
            profile = {}
        for slot in range(5):
            try:
                sources.append(read_source(device, slot))
            except DiagnosticsError as error:
                sources.append({"slot": slot, "error": str(error)})
    finally:
        device.close()

    if not raw:
        raise DiagnosticsError("device returned no data")

    # Whether hidapi prepends the report ID depends on the binding and the
    # platform, so try both framings and let the CRC decide rather than
    # guessing. A wrong guess would silently misalign every field.
    data = bytes(raw)
    candidates = [data[:CONFIG_REPORT_LEN]]
    if data[0] == CONFIG_REPORT_ID:
        candidates.append(data[1 : 1 + CONFIG_REPORT_LEN])

    last_error: DiagnosticsError | None = None
    for candidate in candidates:
        try:
            decoded = decode(candidate)
            decoded.update(security)
            decoded.update(identity)
            decoded.update(transfer)
            decoded.update(profile)
            decoded["sources"] = sources
            if message:
                decoded["パニックメッセージ"] = message
            return decoded
        except DiagnosticsError as error:
            last_error = error
    raise last_error or DiagnosticsError("no framing produced a valid block")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--watch", action="store_true", help="poll until interrupted")
    parser.add_argument("--interval", type=float, default=1.0, help="poll interval in seconds")
    args = parser.parse_args()

    while True:
        try:
            for key, value in read_once().items():
                print(f"{key}: {value}")
        except (DiagnosticsError, RuntimeError) as error:
            print(f"error: {error}")
        if not args.watch:
            return
        print("-" * 32)
        time.sleep(args.interval)


if __name__ == "__main__":
    main()
