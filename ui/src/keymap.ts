/**
 * Wire format for the first data-driven, single-layer keymap.
 *
 * The firmware codec lives in `firmware/nrf52840-ble-usb/src/keymap_store.rs`.
 * Keep this small model intentionally separate from the future layer/action
 * editor so the transfer contract can be tested before that UI exists.
 */

export const KEYMAP_PAYLOAD_VERSION = 1;
export const KEYMAP_RULE_WIRE_LEN = 4;
export const KEYMAP_PAYLOAD_HEADER_LEN = 2;
export const KEYMAP_RULE_CAPACITY = 32;
export const KEYMAP_PAYLOAD_MAX_LEN =
  KEYMAP_PAYLOAD_HEADER_LEN + KEYMAP_RULE_CAPACITY * KEYMAP_RULE_WIRE_LEN;

const INPUT_SHIFTED = 1 << 0;
const OUTPUT_SHIFTED = 1 << 1;
const RULE_FLAGS_MASK = INPUT_SHIFTED | OUTPUT_SHIFTED;

export interface KeymapRule {
  inputUsage: number;
  inputShifted: boolean;
  outputUsage: number;
  outputShifted: boolean;
}

function assertByte(value: number, label: string, allowZero = true): void {
  if (!Number.isInteger(value) || value < (allowZero ? 0 : 1) || value > 0xff) {
    throw new Error(`${label} must be a byte${allowZero ? '' : ' greater than zero'}.`);
  }
}

function assertRule(rule: KeymapRule, index: number): void {
  assertByte(rule.inputUsage, `rules[${index}].inputUsage`, false);
  assertByte(rule.outputUsage, `rules[${index}].outputUsage`);
  if (typeof rule.inputShifted !== 'boolean' || typeof rule.outputShifted !== 'boolean') {
    throw new Error(`rules[${index}] shift flags must be booleans.`);
  }
}

function duplicateOf(rule: KeymapRule, rules: readonly KeymapRule[]): boolean {
  return rules.some(
    (other) =>
      other.inputUsage === rule.inputUsage && other.inputShifted === rule.inputShifted,
  );
}

/** Encodes a complete keymap payload for the generic configuration transfer. */
export function encodeKeymapPayload(rules: readonly KeymapRule[]): Uint8Array {
  if (rules.length > KEYMAP_RULE_CAPACITY) {
    throw new Error(`keymap cannot contain more than ${KEYMAP_RULE_CAPACITY} rules.`);
  }
  const payload = new Uint8Array(KEYMAP_PAYLOAD_HEADER_LEN + rules.length * KEYMAP_RULE_WIRE_LEN);
  payload[0] = KEYMAP_PAYLOAD_VERSION;
  payload[1] = rules.length;
  rules.forEach((rule, index) => {
    assertRule(rule, index);
    if (duplicateOf(rule, rules.slice(0, index))) {
      throw new Error(`rules[${index}] duplicates an input usage and shift state.`);
    }
    const offset = KEYMAP_PAYLOAD_HEADER_LEN + index * KEYMAP_RULE_WIRE_LEN;
    payload[offset] = rule.inputUsage;
    payload[offset + 1] =
      (rule.inputShifted ? INPUT_SHIFTED : 0) | (rule.outputShifted ? OUTPUT_SHIFTED : 0);
    payload[offset + 2] = rule.outputUsage;
    // offset + 3 is reserved and remains zero.
  });
  return payload;
}

/** Decodes and validates a complete keymap payload from the bridge. */
export function decodeKeymapPayload(payload: Uint8Array): KeymapRule[] {
  if (payload.length < KEYMAP_PAYLOAD_HEADER_LEN) {
    throw new Error(`keymap payload is too short: ${payload.length}.`);
  }
  if (payload[0] !== KEYMAP_PAYLOAD_VERSION) {
    throw new Error(`unsupported keymap payload version ${payload[0]}.`);
  }
  const count = payload[1];
  if (count > KEYMAP_RULE_CAPACITY) {
    throw new Error(`keymap contains too many rules: ${count}.`);
  }
  const expectedLength = KEYMAP_PAYLOAD_HEADER_LEN + count * KEYMAP_RULE_WIRE_LEN;
  if (payload.length !== expectedLength) {
    throw new Error(`keymap payload length must be ${expectedLength}: ${payload.length}.`);
  }

  const rules: KeymapRule[] = [];
  for (let index = 0; index < count; index += 1) {
    const offset = KEYMAP_PAYLOAD_HEADER_LEN + index * KEYMAP_RULE_WIRE_LEN;
    const flags = payload[offset + 1];
    if ((flags & ~RULE_FLAGS_MASK) !== 0) {
      throw new Error(`rules[${index}] contains unknown flags ${flags}.`);
    }
    if (payload[offset + 3] !== 0) {
      throw new Error(`rules[${index}] has a non-zero reserved byte.`);
    }
    const rule = {
      inputUsage: payload[offset],
      inputShifted: (flags & INPUT_SHIFTED) !== 0,
      outputUsage: payload[offset + 2],
      outputShifted: (flags & OUTPUT_SHIFTED) !== 0,
    } satisfies KeymapRule;
    assertRule(rule, index);
    if (duplicateOf(rule, rules)) {
      throw new Error(`rules[${index}] duplicates an input usage and shift state.`);
    }
    rules.push(rule);
  }
  return rules;
}
