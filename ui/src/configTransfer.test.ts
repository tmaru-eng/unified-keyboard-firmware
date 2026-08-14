import { describe, expect, it } from 'vitest';

import { CONFIG_PROTOCOL_VERSION, CONFIG_REPORT_ID, CONFIG_REPORT_LEN, crc32Ieee } from './injection';
import { SimulatedBridge } from './simulatedBridge';
import {
  TARGET_KEYMAP,
  TARGET_SCRATCH,
  TRANSFER_CAPACITY,
  TRANSFER_CHUNK_BYTES,
  buildWriteChunk,
  buildWriteCommit,
} from './transfer';
import { readTransfer, writeConfig, writeKeymap } from './webhid';

const payloadOf = (length: number) =>
  Uint8Array.from({ length }, (_unused, index) => (index * 7 + 3) & 0xff);

describe('writing a payload the board cannot take in one report', () => {
  it('starts idle, with nothing staged', async () => {
    const bridge = new SimulatedBridge();

    const transfer = await readTransfer(bridge);

    expect(transfer.stateLabel).toBe('待機中');
    expect(transfer.summary).toBe('待機中');
    expect(bridge.committed).toBeNull();
  });

  it('carries a payload that fits one chunk', async () => {
    const bridge = new SimulatedBridge();
    const payload = payloadOf(4);

    const transfer = await writeConfig(bridge, payload);

    expect(transfer.stateLabel).toBe('完了');
    expect(transfer.computedCrc).toBe(crc32Ieee(payload));
    expect(Array.from(bridge.committed ?? [])).toEqual(Array.from(payload));
  });

  it('carries a payload that fills the staging area exactly', async () => {
    // The capacity and the chunk size do not divide evenly, so the last chunk
    // is short. That boundary is where an off-by-one lives.
    const bridge = new SimulatedBridge();
    const payload = payloadOf(TRANSFER_CAPACITY);

    const transfer = await writeConfig(bridge, payload);

    expect(transfer.stateLabel).toBe('完了');
    expect(transfer.receivedLen).toBe(TRANSFER_CAPACITY);
    expect(Array.from(bridge.committed ?? [])).toEqual(Array.from(payload));
  });

  it('carries a payload whose length is not a multiple of the chunk size', async () => {
    const bridge = new SimulatedBridge();
    const payload = payloadOf(TRANSFER_CHUNK_BYTES * 3 + 1);

    const transfer = await writeConfig(bridge, payload);

    expect(transfer.stateLabel).toBe('完了');
    expect(Array.from(bridge.committed ?? [])).toEqual(Array.from(payload));
  });

  it('reads the result back rather than trusting the writes', async () => {
    // A feature report reports a USB-level result. That says the report reached
    // the interface, not that the board accepted what was in it.
    const bridge = new SimulatedBridge();

    const transfer = await writeConfig(bridge, payloadOf(10));

    expect(transfer.declaredCrc).toBe(transfer.computedCrc);
  });

  it('sends the versioned keymap to its dedicated target', async () => {
    const bridge = new SimulatedBridge();
    const transfer = await writeKeymap(bridge, [
      { inputUsage: 0x04, inputShifted: false, outputUsage: 0x05, outputShifted: false },
    ]);

    expect(transfer.target).toBe(TARGET_KEYMAP);
    expect(transfer.stateLabel).toBe('完了');
    expect(bridge.committed?.slice(0, 2)).toEqual(new Uint8Array([1, 1]));
  });
});

describe('a transfer that does not complete', () => {
  async function send(bridge: SimulatedBridge, report: Uint8Array): Promise<void> {
    await bridge.sendFeatureReport(CONFIG_REPORT_ID, report);
  }

  function beginFrame(target: number, totalLen: number, crc: number): Uint8Array {
    const report = new Uint8Array(CONFIG_REPORT_LEN);
    const view = new DataView(report.buffer);
    report[0] = CONFIG_PROTOCOL_VERSION;
    report[1] = 104;
    report[2] = target;
    view.setUint16(4, totalLen, true);
    view.setUint32(6, crc, true);
    view.setUint32(28, crc32Ieee(report.subarray(0, 28)), true);
    return report;
  }

  it('applies nothing when the commit disagrees with what arrived', async () => {
    // This is the property the whole shape exists for. A configuration written
    // halfway to a keyboard bridge is not a lesser failure than none: it is the
    // one with no way back.
    const bridge = new SimulatedBridge();
    const payload = payloadOf(30);
    const wrong = payloadOf(30);
    wrong[0] ^= 0xff;

    await send(bridge, beginFrame(TARGET_SCRATCH, payload.length, crc32Ieee(wrong)));
    await send(bridge, buildWriteChunk(0, payload.subarray(0, TRANSFER_CHUNK_BYTES)));
    await send(bridge, buildWriteChunk(1, payload.subarray(TRANSFER_CHUNK_BYTES)));
    await send(bridge, buildWriteCommit(TARGET_SCRATCH, wrong));

    const transfer = await readTransfer(bridge);
    expect(transfer.stateLabel).toBe('失敗');
    expect(transfer.lastError).toBe(6);
    expect(bridge.committed).toBeNull();
  });

  it('refuses a chunk that arrives before the transfer starts', async () => {
    const bridge = new SimulatedBridge();

    await send(bridge, buildWriteChunk(0, payloadOf(4)));

    const transfer = await readTransfer(bridge);
    expect(transfer.lastError).toBe(1);
    expect(transfer.lastErrorLabel).toBe('開始せずに断片が来た');
  });

  it('refuses a chunk that skips a number', async () => {
    const bridge = new SimulatedBridge();
    const payload = payloadOf(46);
    await send(bridge, beginFrame(TARGET_SCRATCH, payload.length, crc32Ieee(payload)));

    await send(bridge, buildWriteChunk(1, payload.subarray(0, TRANSFER_CHUNK_BYTES)));

    const transfer = await readTransfer(bridge);
    expect(transfer.lastError).toBe(2);
    expect(transfer.nextIndex).toBe(0);
  });

  it('refuses a commit before everything has arrived', async () => {
    const bridge = new SimulatedBridge();
    const payload = payloadOf(46);
    await send(bridge, beginFrame(TARGET_SCRATCH, payload.length, crc32Ieee(payload)));
    await send(bridge, buildWriteChunk(0, payload.subarray(0, TRANSFER_CHUNK_BYTES)));

    await send(bridge, buildWriteCommit(TARGET_SCRATCH, payload));

    const transfer = await readTransfer(bridge);
    expect(transfer.lastError).toBe(5);
    expect(bridge.committed).toBeNull();
  });

  it('refuses an unknown target', async () => {
    const bridge = new SimulatedBridge();

    await send(bridge, beginFrame(9, 4, 0));

    expect((await readTransfer(bridge)).lastError).toBe(8);
  });

  it('refuses a payload larger than the staging area', async () => {
    const bridge = new SimulatedBridge();

    await send(bridge, beginFrame(TARGET_SCRATCH, TRANSFER_CAPACITY + 1, 0));

    expect((await readTransfer(bridge)).lastError).toBe(4);
  });

  it('starts clean when a new transfer begins after an abandoned one', async () => {
    const bridge = new SimulatedBridge();
    const abandoned = payloadOf(46);
    await send(bridge, beginFrame(TARGET_SCRATCH, abandoned.length, crc32Ieee(abandoned)));
    await send(bridge, buildWriteChunk(0, abandoned.subarray(0, TRANSFER_CHUNK_BYTES)));

    const payload = payloadOf(8);
    const transfer = await writeConfig(bridge, payload);

    expect(transfer.stateLabel).toBe('完了');
    expect(Array.from(bridge.committed ?? [])).toEqual(Array.from(payload));
  });

  it('reports progress while it is still arriving', async () => {
    const bridge = new SimulatedBridge();
    const payload = payloadOf(46);
    await send(bridge, beginFrame(TARGET_SCRATCH, payload.length, crc32Ieee(payload)));
    await send(bridge, buildWriteChunk(0, payload.subarray(0, TRANSFER_CHUNK_BYTES)));

    const transfer = await readTransfer(bridge);

    expect(transfer.summary).toBe(`受信中 ${TRANSFER_CHUNK_BYTES}/46バイト、次の断片 1`);
  });
});
