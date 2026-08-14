/**
 * The conversion profile, as it travels to the board.
 *
 * Until now this lived only in the firmware, compiled in. That made the bridge
 * depend on the host it was built for: US-to-JIS assumes the machine is set to
 * JIS, and the same board on a US-configured machine produces the wrong
 * symbols. The keys live in the board's own flash so it can be carried between
 * computers — this was the one thing that could not travel with it.
 *
 * The payload is deliberately tiny. It rides the transfer path built for the
 * things that are not: keymaps, several bonds, per-source profiles.
 *
 * Byte layout must match `firmware/nrf52840-ble-usb/src/profile_store.rs`.
 */

/** Version of the payload layout, not of the firmware. */
export const PROFILE_PAYLOAD_VERSION = 1;

/** Version of the explicit source-slot profile payload. */
export const SOURCE_PROFILE_PAYLOAD_VERSION = 2;

/** The transfer target that applies and stores a profile. */
export const TARGET_PROFILE = 2;

/** Transfer target for a source-slot profile, distinct from the legacy target. */
export const TARGET_SOURCE_PROFILE = 3;

const SOURCE_SLOT_COUNT = 5;

const FLAG_US_TO_JIS = 0b001;
const FLAG_CAPS_TO_CTRL = 0b010;
const FLAG_SWAP_ALT_GUI = 0b100;
const KNOWN_FLAGS = FLAG_US_TO_JIS | FLAG_CAPS_TO_CTRL | FLAG_SWAP_ALT_GUI;

/** What the bridge does to every report from a source. */
export interface Profile {
  /** Apply the ANSI US to JIS symbol conversion. */
  usToJis: boolean;
  /** Send Left Control where Caps Lock was pressed. */
  capsToCtrl: boolean;
  /** Swap the Alt and GUI modifiers. */
  swapAltGui: boolean;
}

/** The conversion a bridge carries by default: layout only. */
export const PROFILE_US_JIS: Profile = {
  usToJis: true,
  capsToCtrl: false,
  swapAltGui: false,
};

/** Builds the two bytes the firmware reads. */
export function encodeProfilePayload(profile: Profile): Uint8Array {
  const flags = profileFlags(profile);
  return Uint8Array.from([PROFILE_PAYLOAD_VERSION, flags]);
}

/** Builds the versioned source-slot profile payload. */
export function encodeSourceProfilePayload(slot: number, profile: Profile): Uint8Array {
  if (!Number.isInteger(slot) || slot < 0 || slot >= SOURCE_SLOT_COUNT) {
    throw new RangeError(`source slot must be an integer from 0 through 4: ${slot}`);
  }
  return Uint8Array.from([SOURCE_PROFILE_PAYLOAD_VERSION, 4, slot, profileFlags(profile)]);
}

/**
 * Reads a payload back, refusing anything it does not fully understand.
 *
 * An unknown bit is an error rather than something to mask off. A newer sender
 * setting a flag this build has no name for, read as that flag being absent, is
 * a disagreement neither side can see — and what it silently changes here is
 * what every keystroke becomes.
 */
export function decodeProfilePayload(payload: Uint8Array): Profile {
  if (payload.length !== 2) {
    throw new Error(`プロファイルは2バイトです（${payload.length}）。`);
  }
  if (payload[0] !== PROFILE_PAYLOAD_VERSION) {
    throw new Error(`対応していないプロファイルの版 ${payload[0]}。`);
  }
  const flags = payload[1];
  if ((flags & ~KNOWN_FLAGS) !== 0) {
    throw new Error(`未定義のビットが立っています（0x${flags.toString(16)}）。`);
  }
  return profileFromFlags(flags);
}

/** Decodes a source-specific profile payload without accepting legacy bytes. */
export function decodeSourceProfilePayload(payload: Uint8Array): { slot: number; profile: Profile } {
  if (payload.length !== 4) {
    throw new Error(`source profile payload must be four bytes: ${payload.length}`);
  }
  if (payload[0] !== SOURCE_PROFILE_PAYLOAD_VERSION) {
    throw new Error(`unsupported source profile version ${payload[0]}`);
  }
  if (payload[1] !== 4) {
    throw new Error(`unsupported source profile payload length ${payload[1]}`);
  }
  if (payload[2] >= SOURCE_SLOT_COUNT) {
    throw new Error(`unknown source slot ${payload[2]}`);
  }
  return { slot: payload[2], profile: profileFromFlags(payload[3]) };
}

function profileFlags(profile: Profile): number {
  return (
    (profile.usToJis ? FLAG_US_TO_JIS : 0) |
    (profile.capsToCtrl ? FLAG_CAPS_TO_CTRL : 0) |
    (profile.swapAltGui ? FLAG_SWAP_ALT_GUI : 0)
  );
}

function profileFromFlags(flags: number): Profile {
  if ((flags & ~KNOWN_FLAGS) !== 0) {
    throw new Error(`譛ｪ螳夂ｾｩ縺ｮ繝薙ャ繝医′遶九▲縺ｦ縺・∪縺呻ｼ・x${flags.toString(16)}・峨Ａ`);
  }
  return {
    usToJis: (flags & FLAG_US_TO_JIS) !== 0,
    capsToCtrl: (flags & FLAG_CAPS_TO_CTRL) !== 0,
    swapAltGui: (flags & FLAG_SWAP_ALT_GUI) !== 0,
  };
}

/** Renders a profile as the sentence a person reads on the screen. */
export function describeProfile(profile: Profile): string {
  const parts: string[] = [];
  if (profile.usToJis) {
    parts.push('US→JIS');
  }
  if (profile.capsToCtrl) {
    parts.push('Caps→Ctrl');
  }
  if (profile.swapAltGui) {
    parts.push('Alt⇄GUI');
  }
  return parts.length === 0 ? '変換なし' : parts.join(' / ');
}
