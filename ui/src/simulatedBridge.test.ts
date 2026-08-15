import { describe, expect, it } from 'vitest';

import { CONFIG_PROTOCOL_VERSION, CONFIG_REPORT_ID, CONFIG_REPORT_LEN, crc32Ieee } from './injection';
import { SimulatedBridge, SimulatedProtocolError, defaultSimulatedState } from './simulatedBridge';
import type { KeymapRule } from './keymap';
import {
  deleteBond,
  injectSourceReport,
  readSecurity,
  readSource,
  readSources,
  readStatus,
  readKeymap,
  readSourceKeymap,
  renameBond,
  writeKeymap,
  writeSourceKeymap,
  writeSourceProfile,
} from './webhid';

describe('a bridge that is not there', () => {
  it('answers the application through its real encoders and decoders', async () => {
    // The value of the simulation is that nothing above it knows. If the
    // application had a second path for it, the path exercised here would not
    // be the one that runs against hardware.
    const bridge = new SimulatedBridge();

    const status = await readStatus(bridge);

    expect(status.stateLabel).toBe('購読済み');
    expect(status.connectionCount).toBe(1);
    expect(status.lastAddress).toBe('db:e9:25:13:55:25');
    expect(status.lastRssi).toBe(-58);
    expect(status.discoveryStepLabel).toBe('9 通知の購読');
  });

  it('opens itself when the application reads without opening first', async () => {
    const bridge = new SimulatedBridge();

    await readStatus(bridge);

    expect(bridge.opened).toBe(true);
  });

  it('returns the security block only after it is selected', async () => {
    const bridge = new SimulatedBridge();
    bridge.setState({ stuckKeyReleases: 3, bondFlags: 0x01 });

    const security = await readSecurity(bridge);

    expect(security.stuckKeyReleases).toBe(3);
    expect(security.bondSummary).toBe('起動時に鍵を読み込んだ');
  });

  it('goes back to the status block after every read', async () => {
    // The firmware resets its selector on each read. A simulation that kept the
    // selection would let the application forget to select and still work here,
    // then fail on hardware.
    const bridge = new SimulatedBridge();
    await readSecurity(bridge);

    const status = await readStatus(bridge);

    expect(status.stateLabel).toBe('購読済み');
  });

  it('counts an injected report the way the board does', async () => {
    const bridge = new SimulatedBridge();
    const before = bridge.state.reportsForwarded;

    await injectSourceReport(bridge, 0x02, [0x1f]);

    expect(bridge.state.reportsForwarded).toBe(before + 1);
    expect((await readStatus(bridge)).reportsForwarded).toBe(before + 1);
  });

  it('reads BLE slot zero and virtual slot four through the selector path', async () => {
    const bridge = new SimulatedBridge();

    const sources = await readSources(bridge);

    expect(sources).toHaveLength(5);
    expect(sources[0]).toMatchObject({ slot: 0, transportLabel: 'BLE', state: 5 });
    expect(sources[4]).toMatchObject({
      slot: 4,
      transportLabel: 'Virtual',
      name: 'Virtual Input',
      irkPresent: false,
    });
  });

  it('applies a source profile through the explicit target and reads it back', async () => {
    const bridge = new SimulatedBridge();

    await writeSourceProfile(bridge, 4, { usToJis: true, capsToCtrl: false, swapAltGui: true });

    expect((await readSource(bridge, 4)).profileFlags).toBe(0b101);
  });

  it('reads back a multi-chunk keymap and preserves the editor model', async () => {
    const bridge = new SimulatedBridge();
    const rules: KeymapRule[] = [
      { inputUsage: 0x04, inputShifted: false, outputUsage: 0x05, outputShifted: false },
      { inputUsage: 0x1f, inputShifted: true, outputUsage: 0x2f, outputShifted: false },
      { inputUsage: 0x20, inputShifted: false, outputUsage: 0x30, outputShifted: true },
      { inputUsage: 0x21, inputShifted: true, outputUsage: 0x31, outputShifted: true },
      { inputUsage: 0x22, inputShifted: false, outputUsage: 0x32, outputShifted: false },
      { inputUsage: 0x23, inputShifted: true, outputUsage: 0x33, outputShifted: true },
    ];

    await writeKeymap(bridge, rules);

    expect(await readKeymap(bridge)).toEqual(rules);
  });

  it('keeps source-slot keymaps independent through target 6', async () => {
    const bridge = new SimulatedBridge();
    const rules: KeymapRule[] = [
      { inputUsage: 0x04, inputShifted: false, outputUsage: 0x1d, outputShifted: false },
    ];

    await writeSourceKeymap(bridge, 4, rules);

    expect(await readSourceKeymap(bridge, 4)).toEqual(rules);
    expect(await readSourceKeymap(bridge, 0)).toEqual([]);
  });

  it('refuses source profile writes for an unregistered bond slot', async () => {
    const bridge = new SimulatedBridge();

    await expect(
      writeSourceProfile(bridge, 1, { usToJis: true, capsToCtrl: false, swapAltGui: false }),
    ).rejects.toThrow(/not registered/);
    expect((await readSource(bridge, 1)).profileFlags).toBe(0);
  });

  it('renames a registered source through the bond-management target', async () => {
    const bridge = new SimulatedBridge();

  const transfer = await renameBond(bridge, 0, 'Keyboard B');

    expect(transfer.state).toBe(2);
  expect((await readSource(bridge, 0)).name).toBe('Keyboard B');
  });

  it('deletes a registered source and exposes the empty slot', async () => {
    const bridge = new SimulatedBridge();

    await deleteBond(bridge, 0);

    expect((await readSource(bridge, 0)).state).toBe(0);
    expect((await readSource(bridge, 0)).transportLabel).toBe('Unregistered');
  });

  it('is no more forgiving than the firmware', async () => {
    // A simulation that accepts what the board refuses is worse than none: it
    // lets a broken encoder pass here and fail on hardware.
    const bridge = new SimulatedBridge();
    const corrupt = new Uint8Array(CONFIG_REPORT_LEN);
    corrupt[0] = CONFIG_PROTOCOL_VERSION;
    corrupt[1] = 100;

    await expect(bridge.sendFeatureReport(CONFIG_REPORT_ID, corrupt)).rejects.toThrow(
      SimulatedProtocolError,
    );
  });

  it('rejects non-zero padding after a short write chunk', async () => {
    const bridge = new SimulatedBridge();
    const frame = new Uint8Array(CONFIG_REPORT_LEN);
    frame[0] = CONFIG_PROTOCOL_VERSION;
    frame[1] = 105;
    frame[2] = 0;
    frame[4] = 1;
    frame[5] = 0x04;
    frame[6] = 0xff;
    new DataView(frame.buffer).setUint32(28, crc32Ieee(frame.subarray(0, 28)), true);

    await expect(bridge.sendFeatureReport(CONFIG_REPORT_ID, frame)).rejects.toThrow(/padding/);
  });

  it('refuses an unsupported protocol version', async () => {
    const bridge = new SimulatedBridge();
    const frame = new Uint8Array(CONFIG_REPORT_LEN);
    frame[0] = CONFIG_PROTOCOL_VERSION + 1;
    new DataView(frame.buffer).setUint32(28, crc32Ieee(frame.subarray(0, 28)), true);

    await expect(bridge.sendFeatureReport(CONFIG_REPORT_ID, frame)).rejects.toThrow(/バージョン/);
  });

  it('refuses a command it does not know', async () => {
    const bridge = new SimulatedBridge();
    const frame = new Uint8Array(CONFIG_REPORT_LEN);
    frame[0] = CONFIG_PROTOCOL_VERSION;
    frame[1] = 200;
    new DataView(frame.buffer).setUint32(28, crc32Ieee(frame.subarray(0, 28)), true);

    await expect(bridge.sendFeatureReport(CONFIG_REPORT_ID, frame)).rejects.toThrow(/未知のコマンド/);
  });

  it('accepts the report-ID-prefixed framing the Python tools send', async () => {
    const bridge = new SimulatedBridge();
    const payload = new Uint8Array(CONFIG_REPORT_LEN);
    payload[0] = CONFIG_PROTOCOL_VERSION;
    payload[1] = 102; // pairing mode
    payload[2] = 1;
    new DataView(payload.buffer).setUint32(28, crc32Ieee(payload.subarray(0, 28)), true);
    const prefixed = new Uint8Array(CONFIG_REPORT_LEN + 1);
    prefixed[0] = CONFIG_REPORT_ID;
    prefixed.set(payload, 1);

    await bridge.sendFeatureReport(CONFIG_REPORT_ID, prefixed);

    expect(bridge.state.pairingOpen).toBe(true);
  });

  it('rejects a prefixed frame with the wrong report ID', async () => {
    const bridge = new SimulatedBridge();
    const payload = new Uint8Array(CONFIG_REPORT_LEN);
    payload[0] = CONFIG_PROTOCOL_VERSION;
    payload[1] = 102;
    const prefixed = new Uint8Array(CONFIG_REPORT_LEN + 1);
    prefixed[0] = CONFIG_REPORT_ID + 1;
    prefixed.set(payload, 1);

    await expect(bridge.sendFeatureReport(CONFIG_REPORT_ID, prefixed)).rejects.toThrow(
      SimulatedProtocolError,
    );
  });

  it('starts from a board that has been running, not an empty one', () => {
    // An all-zero device hides every formatting decision the diagnostics screen
    // makes, so a screenshot of it proves nothing.
    const state = defaultSimulatedState();

    expect(state.inputReportsReceived).toBeGreaterThan(0);
    expect(state.bondFlags).not.toBe(0);
  });
});
