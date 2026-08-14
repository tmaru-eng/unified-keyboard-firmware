/**
 * Configuration commands the bridge accepts beyond report injection.
 *
 * These are what make the bridge usable on a machine that has none of this
 * project's tooling. `pairing_mode.py` needs Python and `uv`; a page served
 * over HTTPS needs a browser. Welcoming a new keyboard on someone else's
 * computer only works through the second.
 *
 * Numbers must match `firmware/nrf52840-ble-usb/src/uf2_reset.rs`.
 */

import { CONFIG_PROTOCOL_VERSION, CONFIG_REPORT_LEN, crc32Ieee } from './injection';

/** Opens or closes the bridge to keyboards it has not bonded with. */
export const UKF_SET_PAIRING_MODE = 102;
/** Requires a passkey for the next pairing, rather than Just Works. */
export const UKF_SET_PAIRING_METHOD = 103;

function command(id: number, argument: number): Uint8Array {
  const report = new Uint8Array(CONFIG_REPORT_LEN);
  report[0] = CONFIG_PROTOCOL_VERSION;
  report[1] = id;
  report[2] = argument;
  new DataView(report.buffer).setUint32(28, crc32Ieee(report.subarray(0, 28)), true);
  return report;
}

/**
 * Builds the report that opens or closes the pairing window.
 *
 * The window closes itself after two minutes, and closes as soon as a keyboard
 * bonds. Both are the firmware's doing; this only asks.
 */
export function buildPairingModeReport(open: boolean): Uint8Array {
  return command(UKF_SET_PAIRING_MODE, open ? 1 : 0);
}

/** Builds the report that chooses passkey entry over Just Works. */
export function buildPairingMethodReport(requirePasskey: boolean): Uint8Array {
  return command(UKF_SET_PAIRING_METHOD, requirePasskey ? 1 : 0);
}
