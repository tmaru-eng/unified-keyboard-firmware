import { describe, expect, it } from 'vitest';

import {
  BOOT_KEY_SLOTS,
  CONFIG_PROTOCOL_VERSION,
  CONFIG_REPORT_ID,
  CONFIG_REPORT_LEN,
  INJECTED_REPORT_LEN,
  UKF_INJECT_SOURCE_REPORT,
  buildInjectionPayload,
  buildSourceReport,
  crc32Ieee,
  modifierBits,
} from './injection';

const hex = (bytes: Uint8Array): string =>
  Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join(' ');

describe('crc32Ieee', () => {
  it('matches the IEEE check value', () => {
    // The published check value for CRC-32/ISO-HDLC. The firmware's Rust
    // implementation asserts the same constant.
    expect(crc32Ieee(new TextEncoder().encode('123456789'))).toBe(0xcbf43926);
  });
});

describe('buildInjectionPayload', () => {
  it('reproduces a packet the board has already accepted', () => {
    // Captured from `uv run inject_report.py --shift 0x1f --print-packet`,
    // whose output the firmware accepted over USB on 2026-08-11. The leading
    // 0x64 in that capture is hidapi's report-ID prefix; WebHID carries the ID
    // out of band, so it is not part of the payload.
    const expected =
      '12 64 02 00 1f 00 00 00 00 00 00 00 00 00 00 00 ' +
      '00 00 00 00 00 00 00 00 00 00 00 00 62 f5 7b 6e';

    const sourceReport = buildSourceReport(modifierBits(['leftShift']), [0x1f]);
    expect(hex(buildInjectionPayload(sourceReport))).toBe(expected);
  });

  it('places the version, command and source report where the firmware reads them', () => {
    const sourceReport = buildSourceReport(modifierBits(['leftShift']), [0x1f]);
    const payload = buildInjectionPayload(sourceReport);

    expect(payload).toHaveLength(CONFIG_REPORT_LEN);
    expect(payload[0]).toBe(CONFIG_PROTOCOL_VERSION);
    expect(payload[1]).toBe(UKF_INJECT_SOURCE_REPORT);
    expect(Array.from(payload.subarray(2, 2 + INJECTED_REPORT_LEN))).toEqual(
      Array.from(sourceReport),
    );
  });

  it('carries an all-zero report so a held key can always be released', () => {
    const payload = buildInjectionPayload(buildSourceReport(0, []));

    expect(Array.from(payload.subarray(2, 2 + INJECTED_REPORT_LEN))).toEqual(
      new Array(INJECTED_REPORT_LEN).fill(0),
    );
    // The CRC still has to be valid, or the firmware rejects the release.
    const crcOffset = CONFIG_REPORT_LEN - 4;
    const view = new DataView(payload.buffer);
    expect(view.getUint32(crcOffset, true)).toBe(crc32Ieee(payload.subarray(0, crcOffset)));
  });

  it('rejects a source report of the wrong length', () => {
    expect(() => buildInjectionPayload(new Uint8Array(7))).toThrow(RangeError);
  });
});

describe('buildSourceReport', () => {
  it('keeps byte one reserved', () => {
    expect(buildSourceReport(0xff, [0x04])[1]).toBe(0);
  });

  it('rejects more keys than the boot protocol can carry', () => {
    const tooMany = new Array(BOOT_KEY_SLOTS + 1).fill(0x04);
    expect(() => buildSourceReport(0, tooMany)).toThrow(RangeError);
  });
});

describe('protocol constants', () => {
  it('uses the factory report ID and an extension command above the factory range', () => {
    expect(CONFIG_REPORT_ID).toBe(100);
    // The factory firmware allocates 0..=50, reserving 40..=50 for its
    // unimplemented keymap API.
    expect(UKF_INJECT_SOURCE_REPORT).toBeGreaterThan(50);
  });
});
