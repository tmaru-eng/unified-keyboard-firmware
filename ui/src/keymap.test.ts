import { describe, expect, it } from 'vitest';

import {
  KEYMAP_SOURCE_PAYLOAD_VERSION,
  KEYMAP_PAYLOAD_VERSION,
  decodeKeymapPayload,
  decodeSourceKeymapPayload,
  encodeKeymapPayload,
  encodeSourceKeymapPayload,
  type KeymapRule,
} from './keymap';

const CUSTOM_RULES: KeymapRule[] = [
  { inputUsage: 0x04, inputShifted: false, outputUsage: 0x05, outputShifted: false },
  { inputUsage: 0x1f, inputShifted: true, outputUsage: 0x2f, outputShifted: false },
];

describe('the first data-driven keymap payload', () => {
  it('round-trips usage mappings and shift flags', () => {
    const payload = encodeKeymapPayload(CUSTOM_RULES);

    expect(Array.from(payload)).toEqual([
      KEYMAP_PAYLOAD_VERSION,
      2,
      0x04,
      0,
      0x05,
      0,
      0x1f,
      0b01,
      0x2f,
      0,
    ]);
    expect(decodeKeymapPayload(payload)).toEqual(CUSTOM_RULES);
  });

  it('allows an explicitly empty keymap', () => {
    expect(Array.from(encodeKeymapPayload([]))).toEqual([KEYMAP_PAYLOAD_VERSION, 0]);
    expect(decodeKeymapPayload(new Uint8Array([KEYMAP_PAYLOAD_VERSION, 0]))).toEqual([]);
  });

  it('rejects malformed, duplicated, and oversized maps', () => {
    expect(() => decodeKeymapPayload(new Uint8Array([KEYMAP_PAYLOAD_VERSION]))).toThrow(/too short/);
    expect(() => decodeKeymapPayload(new Uint8Array([2, 0]))).toThrow(/version/);
    expect(() => decodeKeymapPayload(new Uint8Array([KEYMAP_PAYLOAD_VERSION, 1]))).toThrow(/length/);
    expect(() =>
      decodeKeymapPayload(new Uint8Array([KEYMAP_PAYLOAD_VERSION, 1, 0x04, 0x04, 0x05, 0])),
    ).toThrow(/unknown flags/);
    expect(() =>
      decodeKeymapPayload(new Uint8Array([KEYMAP_PAYLOAD_VERSION, 1, 0x04, 0, 0x05, 1])),
    ).toThrow(/reserved/);
    expect(() =>
      decodeKeymapPayload(
        new Uint8Array([
          KEYMAP_PAYLOAD_VERSION,
          2,
          0x04,
          0,
          0x05,
          0,
          0x04,
          0,
          0x06,
          0,
        ]),
      ),
    ).toThrow(/duplicates/);
    expect(() =>
      encodeKeymapPayload(new Array(33).fill(CUSTOM_RULES[0]).map((rule, index) => ({
        ...rule,
        inputUsage: (index % 0xff) + 1,
      }))),
    ).toThrow(/more than 32/);
  });

  it('round-trips a keymap tied to a source slot', () => {
    const payload = encodeSourceKeymapPayload(3, CUSTOM_RULES);

    expect(Array.from(payload)).toEqual([
      KEYMAP_SOURCE_PAYLOAD_VERSION,
      3,
      2,
      0x04,
      0,
      0x05,
      0,
      0x1f,
      0b01,
      0x2f,
      0,
    ]);
    expect(decodeSourceKeymapPayload(payload)).toEqual({ slot: 3, rules: CUSTOM_RULES });
  });

  it('rejects source keymap payloads outside the registered and virtual slots', () => {
    expect(() => encodeSourceKeymapPayload(5, CUSTOM_RULES)).toThrow(/source slot/);
    expect(() => decodeSourceKeymapPayload(new Uint8Array([KEYMAP_SOURCE_PAYLOAD_VERSION, 5, 0]))).toThrow(/source slot/);
  });
});
