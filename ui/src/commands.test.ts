import { describe, expect, it } from 'vitest';

import {
  UKF_SET_PAIRING_METHOD,
  UKF_SET_PAIRING_MODE,
  buildPairingMethodReport,
  buildPairingModeReport,
} from './commands';
import { CONFIG_PROTOCOL_VERSION, CONFIG_REPORT_LEN, crc32Ieee } from './injection';
import { SimulatedBridge } from './simulatedBridge';
import { setPairingMethod, setPairingMode } from './webhid';

function crcOf(report: Uint8Array): number {
  return new DataView(report.buffer, report.byteOffset).getUint32(28, true);
}

describe('pairing commands', () => {
  it('builds a pairing-mode report the firmware accepts', () => {
    const report = buildPairingModeReport(true);

    expect(report).toHaveLength(CONFIG_REPORT_LEN);
    expect(report[0]).toBe(CONFIG_PROTOCOL_VERSION);
    expect(report[1]).toBe(UKF_SET_PAIRING_MODE);
    expect(report[2]).toBe(1);
    expect(crcOf(report)).toBe(crc32Ieee(report.subarray(0, 28)));
  });

  it('distinguishes closing from opening', () => {
    // A closed window is the safe state: a bridge that adopts whatever
    // advertises nearby turns someone else's keyboard into a trusted source of
    // keystrokes. Sending the wrong byte here opens it silently.
    expect(buildPairingModeReport(false)[2]).toBe(0);
  });

  it('builds a pairing-method report', () => {
    const report = buildPairingMethodReport(true);

    expect(report[1]).toBe(UKF_SET_PAIRING_METHOD);
    expect(report[2]).toBe(1);
    expect(buildPairingMethodReport(false)[2]).toBe(0);
    expect(crcOf(report)).toBe(crc32Ieee(report.subarray(0, 28)));
  });
});

describe('sending them', () => {
  it('opens and closes the window on the device', async () => {
    const bridge = new SimulatedBridge();

    await setPairingMode(bridge, true);
    expect(bridge.state.pairingOpen).toBe(true);

    await setPairingMode(bridge, false);
    expect(bridge.state.pairingOpen).toBe(false);
  });

  it('switches the pairing method on the device', async () => {
    const bridge = new SimulatedBridge();

    await setPairingMethod(bridge, true);

    expect(bridge.state.requirePasskey).toBe(true);
  });

  it('opens a device that is not open yet', async () => {
    const bridge = new SimulatedBridge();

    await setPairingMode(bridge, true);

    expect(bridge.opened).toBe(true);
  });
});
