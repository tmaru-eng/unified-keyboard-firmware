# /// script
# requires-python = ">=3.11"
# dependencies = ["hidapi"]
# ///
"""Write the bridge's compatibility profile and verify it over diagnostics.

Examples::

    uv run write_profile.py --us-jis
    uv run write_profile.py --us-jis --caps-to-ctrl
    uv run write_profile.py --no-us-jis --no-caps-to-ctrl --swap-alt-gui
"""

from __future__ import annotations

import argparse
import time
import zlib

from read_diagnostics import DiagnosticsError, decode_profile, read_source
from reset_xiao import CONFIG_REPORT_ID, CONFIG_REPORT_LEN
from write_config import (
    SELECT_TRANSFER,
    TARGET_PROFILE,
    TARGET_SOURCE_PROFILE,
    UKF_SELECT_PANIC_CHUNK,
    build_begin_packet,
    build_chunk_packet,
    build_commit_packet,
    build_packet,
    ensure_report_sent,
    open_bridge,
    read_transfer,
)

SELECT_PROFILE = 0xFB
PROFILE_PAYLOAD_VERSION = 1
PROFILE_PAYLOAD_LEN = 2
SOURCE_PROFILE_PAYLOAD_VERSION = 2
SOURCE_PROFILE_PAYLOAD_LEN = 4
PROFILE_VERIFY_ATTEMPTS = 40
PROFILE_VERIFY_DELAY = 0.05


def profile_flags(*, us_jis: bool, caps_to_ctrl: bool, swap_alt_gui: bool) -> int:
    """Pack the three supported profile switches into the wire bit field."""

    return int(us_jis) | (int(caps_to_ctrl) << 1) | (int(swap_alt_gui) << 2)


def build_profile_payload(*, us_jis: bool, caps_to_ctrl: bool, swap_alt_gui: bool) -> bytes:
    """Build the two-byte profile payload without adding another CRC."""

    return bytes(
        [
            PROFILE_PAYLOAD_VERSION,
            profile_flags(
                us_jis=us_jis,
                caps_to_ctrl=caps_to_ctrl,
                swap_alt_gui=swap_alt_gui,
            ),
        ]
    )


def build_source_profile_payload(
    *, slot: int, us_jis: bool, caps_to_ctrl: bool, swap_alt_gui: bool
) -> bytes:
    """Build the explicit version/length/slot source-profile payload."""

    if not isinstance(slot, int) or isinstance(slot, bool) or not 0 <= slot < 5:
        raise ValueError(f"source slot must be an integer from 0 through 4: {slot}")
    return bytes(
        (
            SOURCE_PROFILE_PAYLOAD_VERSION,
            SOURCE_PROFILE_PAYLOAD_LEN,
            slot,
            profile_flags(
                us_jis=us_jis,
                caps_to_ctrl=caps_to_ctrl,
                swap_alt_gui=swap_alt_gui,
            ),
        )
    )


def build_select_profile_packet() -> bytes:
    """Build the selector request for the profile diagnostic block."""

    return build_packet(UKF_SELECT_PANIC_CHUNK, bytes([SELECT_PROFILE]))


def read_profile(device) -> tuple[bytes, dict[str, object]]:
    """Read and decode one profile diagnostic block."""

    raw = bytes(device.get_feature_report(CONFIG_REPORT_ID, CONFIG_REPORT_LEN + 1))
    last_error: DiagnosticsError | None = None
    for candidate in (raw[:CONFIG_REPORT_LEN], raw[1 : 1 + CONFIG_REPORT_LEN]):
        if len(candidate) != CONFIG_REPORT_LEN:
            continue
        try:
            return candidate, decode_profile(candidate)
        except DiagnosticsError as error:
            last_error = error
    raise last_error or DiagnosticsError("no framing produced a valid profile block")


def profile_diagnostic_matches(payload: bytes, expected_flags: int) -> bool:
    """Accept either durable or radio-quiet-deferred profile persistence."""

    if len(payload) < 6 or payload[3] != expected_flags:
        return False
    persistence_state = (payload[4], payload[5])
    return persistence_state in ((1, 0), (0, 1))


def send_profile(
    device, payload: bytes, *, slot: int | None = None
) -> tuple[bytes, dict[str, object], dict[str, object]]:
    """Transfer, persist, and verify one profile payload."""

    target = TARGET_PROFILE if slot is None else TARGET_SOURCE_PROFILE
    expected_length = PROFILE_PAYLOAD_LEN if slot is None else SOURCE_PROFILE_PAYLOAD_LEN
    if len(payload) != expected_length:
        raise ValueError(f"profile payload must be {expected_length} bytes")

    payload_crc = zlib.crc32(payload) & 0xFFFFFFFF
    ensure_report_sent(device, build_begin_packet(target, len(payload), payload_crc))
    ensure_report_sent(device, build_chunk_packet(0, payload))
    ensure_report_sent(device, build_commit_packet(target, len(payload), payload_crc))

    ensure_report_sent(device, build_packet(UKF_SELECT_PANIC_CHUNK, bytes([SELECT_TRANSFER])))
    transfer_block, transfer = read_transfer(device)
    print(transfer["設定転送"])
    if transfer_block[2] != 2:
        raise RuntimeError(f"configuration transfer was not committed: {transfer['設定転送']}")

    expected_flags = payload[1] if slot is None else payload[3]
    last_error: DiagnosticsError | None = None
    if slot is not None:
        for _ in range(PROFILE_VERIFY_ATTEMPTS):
            try:
                source = read_source(device, slot)
            except DiagnosticsError as error:
                last_error = error
                time.sleep(PROFILE_VERIFY_DELAY)
                continue
            if source["slot"] == slot and source["profile_flags"] == expected_flags:
                print(source)
                return transfer_block, transfer, source
            time.sleep(PROFILE_VERIFY_DELAY)
        raise RuntimeError(f"source profile diagnostics could not be verified: {last_error}")

    profile_block = b""
    profile: dict[str, object] = {}
    for _ in range(PROFILE_VERIFY_ATTEMPTS):
        ensure_report_sent(device, build_select_profile_packet())
        try:
            profile_block, profile = read_profile(device)
        except DiagnosticsError as error:
            last_error = error
            time.sleep(PROFILE_VERIFY_DELAY)
            continue
        if profile_diagnostic_matches(profile_block, expected_flags):
            break
        time.sleep(PROFILE_VERIFY_DELAY)
    else:
        if last_error is not None:
            raise RuntimeError(f"profile diagnostics could not be read: {last_error}")
        raise RuntimeError(
            f"profile was not persisted: flags={profile_block[3]:#04x}, "
            f"stored={profile_block[4]}"
        )

    print(profile["プロファイル"])
    return transfer_block, transfer, profile


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--us-jis",
        dest="us_jis",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="enable or disable US-to-JIS conversion (default: disabled)",
    )
    parser.add_argument(
        "--caps-to-ctrl",
        dest="caps_to_ctrl",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="enable or disable Caps Lock to Control (default: disabled)",
    )
    parser.add_argument(
        "--swap-alt-gui",
        dest="swap_alt_gui",
        action=argparse.BooleanOptionalAction,
        default=False,
        help="enable or disable Alt/GUI swapping (default: disabled)",
    )
    parser.add_argument(
        "--slot",
        type=int,
        help="write the explicit source-slot profile target instead of the legacy global profile",
    )
    args = parser.parse_args()
    if args.slot is None:
        payload = build_profile_payload(
            us_jis=args.us_jis,
            caps_to_ctrl=args.caps_to_ctrl,
            swap_alt_gui=args.swap_alt_gui,
        )
    else:
        payload = build_source_profile_payload(
            slot=args.slot,
            us_jis=args.us_jis,
            caps_to_ctrl=args.caps_to_ctrl,
            swap_alt_gui=args.swap_alt_gui,
        )

    device = open_bridge()
    try:
        send_profile(device, payload, slot=args.slot)
    finally:
        device.close()


if __name__ == "__main__":
    main()
