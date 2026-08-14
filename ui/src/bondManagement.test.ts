import { describe, expect, it } from 'vitest';
import {
  BOND_MANAGEMENT_PAYLOAD_LEN,
  buildDeletePayload,
  buildRenamePayload,
  OP_DELETE,
  OP_RENAME,
} from './bondManagement';

describe('bond-management payloads', () => {
  it('builds a fixed-width UTF-8 rename payload', () => {
    const payload = buildRenamePayload(2, 'Corne');

    expect(payload).toHaveLength(BOND_MANAGEMENT_PAYLOAD_LEN);
    expect(Array.from(payload.slice(0, 9))).toEqual([1, OP_RENAME, 2, 5, 67, 111, 114, 110, 101]);
    expect(payload.slice(9).every((byte) => byte === 0)).toBe(true);
  });

  it('builds a canonical delete payload', () => {
    const payload = buildDeletePayload(3);

    expect(Array.from(payload)).toEqual([1, OP_DELETE, 3, ...new Array(15).fill(0)]);
  });

  it('rejects invalid slots and names that exceed the byte capacity', () => {
    expect(() => buildRenamePayload(4, 'no')).toThrow(/slot/);
    expect(() => buildDeletePayload(-1)).toThrow(/slot/);
    expect(() => buildRenamePayload(0, '12345678901234')).toThrow(/13 UTF-8 bytes/);
    expect(() => buildRenamePayload(0, 'あいうえお')).toThrow(/13 UTF-8 bytes/);
  });
});
