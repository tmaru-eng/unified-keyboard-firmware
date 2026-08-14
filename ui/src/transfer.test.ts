import { describe, expect, it } from 'vitest';

import { CONFIG_PROTOCOL_VERSION, CONFIG_REPORT_LEN, crc32Ieee } from './injection';
import {
  TRANSFER_CAPACITY,
  TRANSFER_CHUNK_BYTES,
  TARGET_SOURCE_PROFILE,
  UKF_WRITE_BEGIN,
  UKF_WRITE_CHUNK,
  UKF_WRITE_COMMIT,
  buildWriteBegin,
  buildWriteChunk,
  buildWriteCommit,
  splitPayload,
} from './transfer';

function frameCrc(report: Uint8Array): number {
  return new DataView(report.buffer, report.byteOffset).getUint32(28, true);
}

function view(report: Uint8Array): DataView {
  return new DataView(report.buffer, report.byteOffset);
}

describe('the write frames', () => {
  it('declares the target, length and payload CRC when it begins', () => {
    const payload = Uint8Array.from([1, 2, 3, 4]);

    const report = buildWriteBegin(1, payload);

    expect(report).toHaveLength(CONFIG_REPORT_LEN);
    expect(report[0]).toBe(CONFIG_PROTOCOL_VERSION);
    expect(report[1]).toBe(UKF_WRITE_BEGIN);
    expect(report[2]).toBe(1);
    expect(view(report).getUint16(4, true)).toBe(4);
    expect(view(report).getUint32(6, true)).toBe(crc32Ieee(payload));
    expect(frameCrc(report)).toBe(crc32Ieee(report.subarray(0, 28)));
  });

  it('carries the index, the count and the bytes in a chunk', () => {
    const data = Uint8Array.from([9, 8, 7]);

    const report = buildWriteChunk(5, data);

    expect(report[1]).toBe(UKF_WRITE_CHUNK);
    expect(view(report).getUint16(2, true)).toBe(5);
    expect(report[4]).toBe(3);
    expect(Array.from(report.subarray(5, 8))).toEqual([9, 8, 7]);
    // The unused tail has to be zero: the firmware reads `count` bytes, and
    // leaving rubbish behind it would make two identical transfers produce
    // different frames.
    expect(report.subarray(8, 28).every((byte) => byte === 0)).toBe(true);
    expect(frameCrc(report)).toBe(crc32Ieee(report.subarray(0, 28)));
  });

  it('repeats the declaration when it commits', () => {
    // The firmware checks that the commit agrees with the begin. Repeating the
    // three values is what lets it notice a sender that changed its mind
    // halfway, or two senders talking to one board.
    const payload = Uint8Array.from([1, 2, 3, 4]);

    const report = buildWriteCommit(1, payload);

    expect(report[1]).toBe(UKF_WRITE_COMMIT);
    expect(report[2]).toBe(1);
    expect(view(report).getUint16(4, true)).toBe(4);
    expect(view(report).getUint32(6, true)).toBe(crc32Ieee(payload));
  });

  it('refuses a chunk that does not fit one frame', () => {
    expect(() => buildWriteChunk(0, new Uint8Array(TRANSFER_CHUNK_BYTES + 1))).toThrow();
  });

  it('refuses an empty chunk', () => {
    // The firmware rejects a count of zero, so building one would only produce
    // a frame that is guaranteed to be refused.
    expect(() => buildWriteChunk(0, new Uint8Array(0))).toThrow();
  });

  it('refuses a payload larger than the board can hold', () => {
    expect(() => buildWriteBegin(1, new Uint8Array(TRANSFER_CAPACITY + 1))).toThrow();
  });
});

describe('splitting a payload', () => {
  it('fills every chunk but the last', () => {
    const payload = new Uint8Array(TRANSFER_CHUNK_BYTES * 2 + 5);

    const chunks = splitPayload(payload);

    expect(chunks).toHaveLength(3);
    expect(chunks[0]).toHaveLength(TRANSFER_CHUNK_BYTES);
    expect(chunks[1]).toHaveLength(TRANSFER_CHUNK_BYTES);
    expect(chunks[2]).toHaveLength(5);
  });

  it('produces one chunk for a payload that already fits', () => {
    expect(splitPayload(new Uint8Array(1))).toHaveLength(1);
  });

  it('preserves every byte in order', () => {
    const payload = Uint8Array.from({ length: 100 }, (_unused, index) => index);

    const rejoined = new Uint8Array(100);
    let offset = 0;
    for (const chunk of splitPayload(payload)) {
      rejoined.set(chunk, offset);
      offset += chunk.length;
    }

    expect(Array.from(rejoined)).toEqual(Array.from(payload));
  });

  it('has nothing to send for an empty payload', () => {
    // A zero-length write is not a transfer. Letting it through would send a
    // begin and a commit with no chunks between them, which the firmware
    // refuses anyway.
    expect(() => splitPayload(new Uint8Array(0))).toThrow();
  });
});

describe('source profile target', () => {
  it('is distinct from the legacy global profile target', () => {
    expect(TARGET_SOURCE_PROFILE).toBe(3);
    expect(TARGET_SOURCE_PROFILE).not.toBe(2);
  });
});
