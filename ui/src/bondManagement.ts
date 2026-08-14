/**
 * Wire payloads for naming and removing one registered BLE bond.
 *
 * Listing is the source-diagnostics path. These are the two mutations that
 * need the versioned transfer target, and the firmware applies them only at a
 * quiet radio boundary.
 */

/** Transfer target for registered BLE bond management. */
export const TARGET_BOND_MANAGEMENT = 4;
/** Current bond-management payload version. */
export const BOND_MANAGEMENT_VERSION = 1;
/** Rename operation. */
export const OP_RENAME = 1;
/** Delete operation. */
export const OP_DELETE = 2;
/** Fixed payload size carried through the generic transfer protocol. */
export const BOND_MANAGEMENT_PAYLOAD_LEN = 18;
/** Maximum UTF-8 bytes retained for one display name. */
export const BOND_MANAGEMENT_NAME_LEN = 13;

function validateSlot(slot: number): void {
  if (!Number.isInteger(slot) || slot < 0 || slot >= 4) {
    throw new RangeError(`bond slot must be an integer from 0 through 3: ${slot}`);
  }
}

function encodeName(name: string): Uint8Array {
  const bytes = new TextEncoder().encode(name);
  if (bytes.length > BOND_MANAGEMENT_NAME_LEN) {
    throw new RangeError(`bond name must be at most ${BOND_MANAGEMENT_NAME_LEN} UTF-8 bytes`);
  }
  return bytes;
}

/** Builds a fixed-width rename payload. */
export function buildRenamePayload(slot: number, name: string): Uint8Array {
  validateSlot(slot);
  const encoded = encodeName(name);
  const payload = new Uint8Array(BOND_MANAGEMENT_PAYLOAD_LEN);
  payload[0] = BOND_MANAGEMENT_VERSION;
  payload[1] = OP_RENAME;
  payload[2] = slot;
  payload[3] = encoded.length;
  payload.set(encoded, 4);
  return payload;
}

/** Builds a fixed-width delete payload with canonical zero padding. */
export function buildDeletePayload(slot: number): Uint8Array {
  validateSlot(slot);
  const payload = new Uint8Array(BOND_MANAGEMENT_PAYLOAD_LEN);
  payload[0] = BOND_MANAGEMENT_VERSION;
  payload[1] = OP_DELETE;
  payload[2] = slot;
  return payload;
}
