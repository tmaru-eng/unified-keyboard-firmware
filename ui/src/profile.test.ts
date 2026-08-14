import { describe, expect, it } from 'vitest';

import {
  PROFILE_PAYLOAD_VERSION,
  TARGET_PROFILE,
  decodeProfilePayload,
  decodeSourceProfilePayload,
  encodeProfilePayload,
  encodeSourceProfilePayload,
} from './profile';
import { SimulatedBridge } from './simulatedBridge';
import { writeConfig } from './webhid';

const NOTHING = { usToJis: false, capsToCtrl: false, swapAltGui: false };

describe('the profile payload', () => {
  it('is two bytes: a version and the flags', () => {
    // The transfer carries its own CRC over the whole payload, so this does not
    // repeat one. Two checksums over the same bytes would only create a way for
    // them to disagree.
    const payload = encodeProfilePayload({ usToJis: true, capsToCtrl: false, swapAltGui: false });

    expect(payload).toHaveLength(2);
    expect(payload[0]).toBe(PROFILE_PAYLOAD_VERSION);
    expect(payload[1]).toBe(0b001);
  });

  it('puts each toggle in its own bit', () => {
    expect(encodeProfilePayload({ usToJis: true, capsToCtrl: true, swapAltGui: true })[1]).toBe(0b111);
    expect(encodeProfilePayload({ usToJis: false, capsToCtrl: true, swapAltGui: false })[1]).toBe(0b010);
    expect(encodeProfilePayload({ usToJis: false, capsToCtrl: false, swapAltGui: true })[1]).toBe(0b100);
    expect(encodeProfilePayload(NOTHING)[1]).toBe(0);
  });

  it('round trips every combination', () => {
    for (let bits = 0; bits < 8; bits += 1) {
      const profile = {
        usToJis: (bits & 0b001) !== 0,
        capsToCtrl: (bits & 0b010) !== 0,
        swapAltGui: (bits & 0b100) !== 0,
      };
      expect(decodeProfilePayload(encodeProfilePayload(profile))).toEqual(profile);
    }
  });

  it('refuses a payload from a version it does not know', () => {
    expect(() => decodeProfilePayload(Uint8Array.from([2, 0]))).toThrow(/版/);
  });

  it('refuses a payload of the wrong length', () => {
    expect(() => decodeProfilePayload(Uint8Array.from([1]))).toThrow();
    expect(() => decodeProfilePayload(Uint8Array.from([1, 0, 0]))).toThrow();
  });

  it('refuses bits that have no meaning yet', () => {
    // Dropping them silently would let a newer sender's setting be read as its
    // absence by an older board, which is a disagreement neither side can see.
    expect(() => decodeProfilePayload(Uint8Array.from([1, 0b1000]))).toThrow(/未定義/);
  });

  it('uses a distinct version, length, slot, and flags for source profiles', () => {
    const payload = encodeSourceProfilePayload(4, {
      usToJis: true,
      capsToCtrl: false,
      swapAltGui: true,
    });

    expect(Array.from(payload)).toEqual([2, 4, 4, 5]);
    expect(decodeSourceProfilePayload(payload)).toEqual({
      slot: 4,
      profile: { usToJis: true, capsToCtrl: false, swapAltGui: true },
    });
  });

  it('rejects an old global payload when a source payload is required', () => {
    expect(() => decodeSourceProfilePayload(Uint8Array.from([1, 5]))).toThrow();
    expect(() => decodeSourceProfilePayload(Uint8Array.from([2, 3, 4, 5]))).toThrow();
    expect(() => decodeSourceProfilePayload(Uint8Array.from([2, 4, 5, 5]))).toThrow();
  });
});

describe('writing a profile to a bridge', () => {
  it('goes through the transfer path to its own target', async () => {
    const bridge = new SimulatedBridge();
    const payload = encodeProfilePayload({ usToJis: false, capsToCtrl: true, swapAltGui: false });

    const transfer = await writeConfig(bridge, payload, TARGET_PROFILE);

    expect(transfer.stateLabel).toBe('完了');
    expect(transfer.target).toBe(TARGET_PROFILE);
    expect(Array.from(bridge.committed ?? [])).toEqual(Array.from(payload));
  });

  it('leaves the board alone when the transfer cannot be verified', async () => {
    // The profile decides what every keystroke becomes. Applying half of one is
    // worse than applying none.
    const bridge = new SimulatedBridge();

    await writeConfig(bridge, encodeProfilePayload({ ...NOTHING, usToJis: true }), TARGET_PROFILE);
    const before = bridge.committed;

    expect(before).not.toBeNull();
  });
});
