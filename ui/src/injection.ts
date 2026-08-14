/**
 * Wire format for pushing a source keyboard report into the bridge.
 *
 * The firmware treats an injected report as if a source keyboard had sent it,
 * applies that source's profile, and emits the converted result from its USB
 * HID keyboard interface. That lets the on-screen keyboard drive the real
 * conversion path without a BLE keyboard attached.
 *
 * Every constant here has to match `firmware/nrf52840-ble-usb/src/uf2_reset.rs`
 * exactly. `injection.test.ts` pins the encoder against a packet captured from
 * the Python tool that is already known to be accepted by the board.
 */

/** HID report ID of the vendor configuration interface. */
export const CONFIG_REPORT_ID = 100;
/** Byte length of the configuration feature report payload. */
export const CONFIG_REPORT_LEN = 32;
/** Protocol version the factory firmware and this repository both speak. */
export const CONFIG_PROTOCOL_VERSION = 18;
/**
 * Command that pushes a source report through the pipeline.
 *
 * The factory firmware allocates commands 0..=50, so this repository's
 * extensions start at 100 and can never collide with a future factory command.
 */
export const UKF_INJECT_SOURCE_REPORT = 100;
/** Byte length of a USB HID boot keyboard report. */
export const INJECTED_REPORT_LEN = 8;
/** Number of simultaneous key slots in a boot keyboard report. */
export const BOOT_KEY_SLOTS = 6;

/** Modifier bits as defined by the HID Usage Tables. */
export const MODIFIER = {
  leftCtrl: 0x01,
  leftShift: 0x02,
  leftAlt: 0x04,
  leftGui: 0x08,
  rightCtrl: 0x10,
  rightShift: 0x20,
  rightAlt: 0x40,
  rightGui: 0x80,
} as const;

export type ModifierName = keyof typeof MODIFIER;

/** Reflected CRC-32/ISO-HDLC, the variant the factory protocol uses. */
export function crc32Ieee(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      // Branch-free reflected polynomial, matching the Rust implementation.
      const mask = -(crc & 1);
      crc = (crc >>> 1) ^ (0xedb88320 & mask);
    }
  }
  return (~crc) >>> 0;
}

/** Builds the eight-byte boot keyboard report a source would have sent. */
export function buildSourceReport(modifiers: number, usages: readonly number[]): Uint8Array {
  if (usages.length > BOOT_KEY_SLOTS) {
    throw new RangeError(`a boot keyboard report carries at most ${BOOT_KEY_SLOTS} keys`);
  }
  const report = new Uint8Array(INJECTED_REPORT_LEN);
  report[0] = modifiers & 0xff;
  // Byte 1 is reserved and always zero.
  usages.forEach((usage, index) => {
    report[2 + index] = usage & 0xff;
  });
  return report;
}

/**
 * Builds the 32-byte feature report payload carrying one source report.
 *
 * WebHID takes the report ID separately, so the returned buffer is the payload
 * only. `hidapi` on the command line prefixes the ID instead; both framings are
 * accepted by the firmware.
 */
export function buildInjectionPayload(sourceReport: Uint8Array): Uint8Array {
  if (sourceReport.length !== INJECTED_REPORT_LEN) {
    throw new RangeError(`source report must be ${INJECTED_REPORT_LEN} bytes`);
  }
  const payload = new Uint8Array(CONFIG_REPORT_LEN);
  payload[0] = CONFIG_PROTOCOL_VERSION;
  payload[1] = UKF_INJECT_SOURCE_REPORT;
  payload.set(sourceReport, 2);

  const crcOffset = CONFIG_REPORT_LEN - 4;
  const crc = crc32Ieee(payload.subarray(0, crcOffset));
  const view = new DataView(payload.buffer);
  view.setUint32(crcOffset, crc, true);
  return payload;
}

/** Combines named modifiers into their HID bitmap. */
export function modifierBits(names: readonly ModifierName[]): number {
  return names.reduce((bits, name) => bits | MODIFIER[name], 0);
}
