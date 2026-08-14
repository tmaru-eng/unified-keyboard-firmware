/**
 * Carries a payload larger than one report to the bridge.
 *
 * The configuration interface moves 32 bytes at a time, and every field after
 * the version, the command and the CRC is argument space. Profiles fit; a
 * keymap uses this transfer in numbered chunks.
 *
 * The shape is length-first, numbered chunks, explicit commit. The board holds
 * what it receives in a staging buffer and applies nothing until the commit
 * proves the whole payload arrived: a transfer that stops halfway leaves the
 * board exactly as it was. A configuration half-written to a keyboard bridge is
 * not a lesser failure than none — it is the one with no way back.
 *
 * Byte layouts must match `firmware/nrf52840-ble-usb/src/config_hid.rs`.
 */

import { CONFIG_PROTOCOL_VERSION, CONFIG_REPORT_LEN, crc32Ieee } from './injection';

/** Starts a transfer, declaring what is coming. */
export const UKF_WRITE_BEGIN = 104;
/** Carries one numbered slice of the payload. */
export const UKF_WRITE_CHUNK = 105;
/** Applies the transfer, repeating the declaration so it can be checked. */
export const UKF_WRITE_COMMIT = 106;

/**
 * Payload bytes one frame can carry.
 *
 * Thirty-two, less the version, the command, the two-byte index, the count and
 * the four-byte CRC.
 */
export const TRANSFER_CHUNK_BYTES = 23;

/** Largest payload the board's staging buffer holds. */
export const TRANSFER_CAPACITY = 512;

/** The scratch target: received and verified, applied nowhere. */
export const TARGET_SCRATCH = 1;
/** The legacy global profile target. */
export const TARGET_PROFILE = 2;
/** The explicit source-slot profile target. */
export const TARGET_SOURCE_PROFILE = 3;
/** The versioned single-layer keymap target. */
export const TARGET_KEYMAP = 5;

function sealed(report: Uint8Array): Uint8Array {
  new DataView(report.buffer, report.byteOffset).setUint32(
    28,
    crc32Ieee(report.subarray(0, 28)),
    true,
  );
  return report;
}

function declaration(command: number, target: number, payload: Uint8Array): Uint8Array {
  if (payload.length === 0) {
    throw new Error('空のペイロードは転送になりません。');
  }
  if (payload.length > TRANSFER_CAPACITY) {
    throw new Error(
      `ペイロードが大きすぎます（${payload.length}バイト、上限${TRANSFER_CAPACITY}）。`,
    );
  }
  const report = new Uint8Array(CONFIG_REPORT_LEN);
  const view = new DataView(report.buffer);
  report[0] = CONFIG_PROTOCOL_VERSION;
  report[1] = command;
  report[2] = target;
  view.setUint16(4, payload.length, true);
  view.setUint32(6, crc32Ieee(payload), true);
  return sealed(report);
}

/** Builds the frame that announces a transfer. */
export function buildWriteBegin(target: number, payload: Uint8Array): Uint8Array {
  return declaration(UKF_WRITE_BEGIN, target, payload);
}

/** Builds the frame that applies a transfer, repeating what was announced. */
export function buildWriteCommit(target: number, payload: Uint8Array): Uint8Array {
  return declaration(UKF_WRITE_COMMIT, target, payload);
}

/** Builds one numbered slice of the payload. */
export function buildWriteChunk(index: number, data: Uint8Array): Uint8Array {
  if (data.length === 0 || data.length > TRANSFER_CHUNK_BYTES) {
    throw new Error(`断片は1..${TRANSFER_CHUNK_BYTES}バイトです（${data.length}）。`);
  }
  const report = new Uint8Array(CONFIG_REPORT_LEN);
  new DataView(report.buffer).setUint16(2, index, true);
  report[0] = CONFIG_PROTOCOL_VERSION;
  report[1] = UKF_WRITE_CHUNK;
  report[4] = data.length;
  report.set(data, 5);
  return sealed(report);
}

/** Splits a payload into the slices the frames can carry, in order. */
export function splitPayload(payload: Uint8Array): Uint8Array[] {
  if (payload.length === 0) {
    throw new Error('空のペイロードは転送になりません。');
  }
  const chunks: Uint8Array[] = [];
  for (let offset = 0; offset < payload.length; offset += TRANSFER_CHUNK_BYTES) {
    chunks.push(payload.subarray(offset, offset + TRANSFER_CHUNK_BYTES));
  }
  return chunks;
}
