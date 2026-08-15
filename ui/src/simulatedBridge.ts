/**
 * A bridge that speaks the real configuration protocol, in the browser.
 *
 * The point is that nothing in the application knows the difference. It
 * implements the same `ConfigHidDevice` surface the browser hands back for the
 * physical board, so `webhid.ts` runs its real encoders, decoders, CRCs, and
 * framing against it. Only the USB transport itself is missing.
 *
 * This exists because verifying the UI against the board is expensive: the
 * device picker is native, the agent's browser cannot open it, and the one
 * board has to be reflashed to change anything. Development that needs a
 * person present for every check does not get done. Here the same code path
 * runs with nobody present.
 *
 * It is deliberately a *protocol* simulation and not a behaviour simulation. It
 * does not convert keystrokes — `ukf-core` does that, in Rust, and reproducing
 * it here would create a second implementation to disagree with the first.
 */

import {
  CONFIG_PROTOCOL_VERSION,
  CONFIG_REPORT_ID,
  CONFIG_REPORT_LEN,
  crc32Ieee,
} from './injection';
import {
  SECURITY_KIND,
  KEYMAP_CHUNK_COUNT_MAX,
  KEYMAP_CHUNK_DATA_LEN,
  KEYMAP_KIND,
  KEYMAP_REPORT_VERSION,
  SOURCE_KEYMAP_CHUNK_COUNT_MAX,
  SOURCE_KEYMAP_CHUNK_DATA_LEN,
  SOURCE_KEYMAP_REPORT_VERSION,
  SELECT_KEYMAP,
  SELECT_SOURCE,
  SOURCE_KIND,
  STATUS_KIND,
  TRANSFER_KIND,
  UKF_SELECT_PANIC_CHUNK,
  UKF_SELECT_KEYMAP,
  UKF_SELECT_SOURCE_KEYMAP,
  UKF_SELECT_SOURCE,
} from './diagnostics';
import { TARGET_KEYMAP, TARGET_SOURCE_KEYMAP, TRANSFER_CAPACITY, TRANSFER_CHUNK_BYTES } from './transfer';
import { decodeKeymapPayload, decodeSourceKeymapPayload } from './keymap';
import {
  BOND_MANAGEMENT_NAME_LEN,
  BOND_MANAGEMENT_PAYLOAD_LEN,
  BOND_MANAGEMENT_VERSION,
  OP_DELETE,
  OP_RENAME,
  TARGET_BOND_MANAGEMENT,
} from './bondManagement';
import type { ConfigHidDevice } from './webhid';

/** Command that pushes a source report through the pipeline. */
const UKF_INJECT_SOURCE_REPORT = 100;
/** Command that opens or closes the pairing window. */
const UKF_SET_PAIRING_MODE = 102;
/** Command that chooses passkey entry over Just Works. */
const UKF_SET_PAIRING_METHOD = 103;
/** Factory-compatible command that resets into the UF2 bootloader. */
const RESET_INTO_BOOTSEL = 1;
/** Selector value that returns the status block. */
const SELECT_STATUS = 0xff;
/** Selector value that returns the security block. */
const SELECT_SECURITY = 0xfe;
/** Selector value that returns the transfer block. */
const SELECT_TRANSFER = 0xfc;
/** Internal selector value for a source-slot keymap chunk. */
const SELECT_SOURCE_KEYMAP = 0xf8;
/** Starts a transfer. */
const UKF_WRITE_BEGIN = 104;
/** Carries one slice of it. */
const UKF_WRITE_CHUNK = 105;
/** Applies it. */
const UKF_WRITE_COMMIT = 106;
/** Target carrying the explicit source-slot profile payload. */
const TARGET_SOURCE_PROFILE = 3;
/** Live source paths present in the 0.6 firmware binary. */
function isRuntimeSourceSlot(slot: number): boolean {
  return slot >= 0 && slot < 5;
}

/** The bridge lifecycle values the simulation can present. */
export type SimulatedState = 0 | 1 | 2 | 3 | 4 | 5 | 6;

/** One source record used by the protocol simulation. */
export interface SimulatedSource {
  slot: number;
  transport: 0 | 1 | 2 | 3;
  state: SimulatedState;
  flags: number;
  profileFlags: number;
  /** Identity address in the wire's least-significant-byte-first order. */
  identityAddress: [number, number, number, number, number, number];
  name: string;
}

/** What the simulated board is currently reporting. */
export interface SimulatedBridgeState {
  state: SimulatedState;
  lastError: number;
  advertisementsSeen: number;
  hidAdvertisements: number;
  /** Peer address, least significant byte first, as the wire carries it. */
  lastAddress: [number, number, number, number, number, number];
  lastRssi: number;
  connectionCount: number;
  inputReportsReceived: number;
  reportsForwarded: number;
  panicLine: number;
  panicCount: number;
  discoveryStep: number;
  bondFlags: number;
  stuckKeyReleases: number;
  livenessProbeFailures: number;
  livenessProbeError: number;
  injectRefusal: number;
  notifyHandle: number;
  notifyExpected: number;
  pairingOpen: boolean;
  requirePasskey: boolean;
  resetRequested: boolean;
  /** Four registration slots plus virtual slot four, in slot order. */
  sources: SimulatedSource[];
}

function defaultSources(): SimulatedSource[] {
  const emptyAddress: [number, number, number, number, number, number] = [0, 0, 0, 0, 0, 0];
  return [
    {
      slot: 0,
      transport: 2,
      state: 5,
      flags: 0x03,
      profileFlags: 0x01,
      identityAddress: [0x25, 0x55, 0x13, 0x25, 0xe9, 0xdb],
      name: 'Keyboard A',
    },
    { slot: 1, transport: 0, state: 0, flags: 0, profileFlags: 0, identityAddress: emptyAddress, name: '' },
    { slot: 2, transport: 0, state: 0, flags: 0, profileFlags: 0, identityAddress: emptyAddress, name: '' },
    { slot: 3, transport: 0, state: 0, flags: 0, profileFlags: 0, identityAddress: emptyAddress, name: '' },
    {
      slot: 4,
      transport: 3,
      state: 5,
      flags: 0,
      profileFlags: 0x01,
      identityAddress: emptyAddress,
      name: 'Virtual Input',
    },
  ];
}

/**
 * A board that has been running for a while with a bonded keyboard.
 *
 * Chosen to look like the state the real board was actually observed in on
 * 2026-08-12 rather than an empty one, because an all-zero device hides every
 * formatting decision the diagnostics screen makes.
 */
export function defaultSimulatedState(): SimulatedBridgeState {
  return {
    state: 5,
    lastError: 0,
    advertisementsSeen: 8550,
    hidAdvertisements: 1,
    lastAddress: [0x25, 0x55, 0x13, 0x25, 0xe9, 0xdb],
    lastRssi: -58,
    connectionCount: 1,
    inputReportsReceived: 131,
    reportsForwarded: 131,
    panicLine: 0,
    panicCount: 0,
    discoveryStep: 9,
    bondFlags: 0x05,
    stuckKeyReleases: 0,
    livenessProbeFailures: 0,
    livenessProbeError: 0,
    injectRefusal: 0,
    notifyHandle: 24,
    notifyExpected: 24,
    pairingOpen: false,
    requirePasskey: false,
    resetRequested: false,
    sources: defaultSources(),
  };
}

/** Raised when the application sends something the firmware would reject. */
export class SimulatedProtocolError extends Error {}

function sealed(bytes: Uint8Array): Uint8Array {
  new DataView(bytes.buffer, bytes.byteOffset).setUint32(28, crc32Ieee(bytes.subarray(0, 28)), true);
  return bytes;
}

/**
 * The configuration interface of a board that is not there.
 *
 * Rejects malformed frames exactly as the firmware does. Accepting them would
 * make the simulation more forgiving than the board, which is the one way a
 * simulation actively causes harm: it would let a broken encoder pass here and
 * fail on hardware.
 */
export class SimulatedBridge implements ConfigHidDevice {
  opened = false;
  readonly productName = 'シミュレーション（実機ではありません）';
  readonly collections = [{ usagePage: 0xff00, usage: 0x20 }];
  /** Which block the next read returns. Reset after every read, as the firmware does. */
  #selector = SELECT_STATUS;
  #selectedSource = 0;
  #selectedKeymapChunk = 0;
  #keymapPayload = Uint8Array.from([1, 0]);
  #selectedSourceKeymapSlot = 0;
  #selectedSourceKeymapChunk = 0;
  #sourceKeymapPayloads = Array.from(
    { length: 5 },
    (_, slot) => Uint8Array.from([2, slot, 0]),
  );
  /**
   * The staging area, mirroring the board's.
   *
   * The firmware is the authority on these rules; this exists so the sender can
   * be exercised without one. Where the two disagree, the firmware is right and
   * this is the bug.
   */
  #transfer = {
    state: 0,
    lastError: 0,
    target: 0,
    expectedLen: 0,
    receivedLen: 0,
    nextIndex: 0,
    declaredCrc: 0,
    computedCrc: 0,
    buffer: new Uint8Array(TRANSFER_CAPACITY),
  };
  #state: SimulatedBridgeState;

  constructor(state: SimulatedBridgeState = defaultSimulatedState()) {
    this.#state = state;
  }

  /** The current simulated state, for a test or a screen that wants to show it. */
  get state(): Readonly<SimulatedBridgeState> {
    return this.#state;
  }

  /** Replaces the simulated state, so a screen can be shown a chosen situation. */
  setState(patch: Partial<SimulatedBridgeState>): void {
    this.#state = { ...this.#state, ...patch };
  }

  async open(): Promise<void> {
    this.opened = true;
  }

  async sendFeatureReport(reportId: number, data: BufferSource): Promise<void> {
    if (reportId !== CONFIG_REPORT_ID) {
      throw new SimulatedProtocolError(`レポートID ${reportId} は設定インターフェースのものではありません`);
    }
    const view = ArrayBuffer.isView(data)
      ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
      : new Uint8Array(data as ArrayBuffer);
    // hidapi prefixes the report ID on some platforms and the browser does not.
    // The firmware accepts both, so this has to as well or the simulation would
    // refuse a frame the board accepts.
    if (view.length === CONFIG_REPORT_LEN + 1 && view[0] !== reportId) {
      throw new SimulatedProtocolError(`譁ｰ縺励＞繝ｬ繝昴・繝・D ${view[0]} 縺ｯ CONFIG_REPORT_ID ${reportId} 縺ｨ荳閾ｴ縺励∪縺帙ｓ`);
    }
    const payload = view.length === CONFIG_REPORT_LEN + 1 ? view.subarray(1) : view;
    if (payload.length !== CONFIG_REPORT_LEN) {
      throw new SimulatedProtocolError(`${CONFIG_REPORT_LEN}バイトではありません（${payload.length}）`);
    }
    if (payload[0] !== CONFIG_PROTOCOL_VERSION) {
      throw new SimulatedProtocolError(`対応していないプロトコルバージョン ${payload[0]}`);
    }
    const frame = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
    const expected = frame.getUint32(28, true);
    const actual = crc32Ieee(payload.subarray(0, 28));
    if (expected !== actual) {
      throw new SimulatedProtocolError('CRCが一致しません');
    }

    switch (payload[1]) {
      case RESET_INTO_BOOTSEL:
        this.#state.resetRequested = true;
        return;
      case UKF_SELECT_PANIC_CHUNK:
        this.#selector = payload[2];
        return;
      case UKF_SELECT_SOURCE:
        if (payload[2] >= 5) {
          throw new SimulatedProtocolError(`unknown source slot ${payload[2]}`);
        }
        if (payload.slice(3, 28).some((byte) => byte !== 0)) {
          throw new SimulatedProtocolError('source selector reserved bytes must be zero');
        }
        this.#selectedSource = payload[2];
        this.#selector = SELECT_SOURCE;
        return;
      case UKF_SELECT_KEYMAP:
        if (payload[2] >= KEYMAP_CHUNK_COUNT_MAX) {
          throw new SimulatedProtocolError(`unknown keymap chunk ${payload[2]}`);
        }
        if (payload.slice(3, 28).some((byte) => byte !== 0)) {
          throw new SimulatedProtocolError('keymap selector reserved bytes must be zero');
        }
        this.#selectedKeymapChunk = payload[2];
        this.#selector = SELECT_KEYMAP;
        return;
      case UKF_SELECT_SOURCE_KEYMAP:
        if (payload[2] >= 5) {
          throw new SimulatedProtocolError(`unknown source keymap slot ${payload[2]}`);
        }
        if (payload[3] >= SOURCE_KEYMAP_CHUNK_COUNT_MAX) {
          throw new SimulatedProtocolError(`unknown source keymap chunk ${payload[3]}`);
        }
        if (payload.slice(4, 28).some((byte) => byte !== 0)) {
          throw new SimulatedProtocolError('source keymap selector reserved bytes must be zero');
        }
        this.#selectedSourceKeymapSlot = payload[2];
        this.#selectedSourceKeymapChunk = payload[3];
        this.#selector = SELECT_SOURCE_KEYMAP;
        return;
      case UKF_SET_PAIRING_MODE:
        this.#state.pairingOpen = payload[2] !== 0;
        return;
      case UKF_SET_PAIRING_METHOD:
        this.#state.requirePasskey = payload[2] !== 0;
        return;
      case UKF_WRITE_BEGIN:
        this.#begin(payload, frame);
        return;
      case UKF_WRITE_CHUNK:
        if (payload[4] < TRANSFER_CHUNK_BYTES && payload.slice(5 + payload[4], 28).some((byte) => byte !== 0)) {
          throw new SimulatedProtocolError('write chunk padding must be zero');
        }
        this.#chunk(payload, frame);
        return;
      case UKF_WRITE_COMMIT:
        this.#commit(payload, frame);
        return;
      case UKF_INJECT_SOURCE_REPORT:
        // Counted, not converted. The conversion lives in `ukf-core` and a
        // second implementation here would be a second thing to be wrong.
        this.#state.inputReportsReceived += 1;
        this.#state.reportsForwarded += 1;
        this.#state.injectRefusal = 0;
        return;
      default:
        throw new SimulatedProtocolError(`未知のコマンド ${payload[1]}`);
    }
  }

  async receiveFeatureReport(reportId: number): Promise<DataView> {
    if (reportId !== CONFIG_REPORT_ID) {
      throw new SimulatedProtocolError(`レポートID ${reportId} は設定インターフェースのものではありません`);
    }
    // The firmware swaps the selector back on every read, so a read that is not
    // preceded by a selection always returns the status block.
    const selector = this.#selector;
    this.#selector = SELECT_STATUS;
    let bytes: Uint8Array;
    if (selector === SELECT_SECURITY) {
      bytes = this.#securityBlock();
    } else if (selector === SELECT_TRANSFER) {
      bytes = this.#transferBlock();
    } else if (selector === SELECT_SOURCE) {
      bytes = this.#sourceBlock();
    } else if (selector === SELECT_KEYMAP) {
      bytes = this.#keymapBlock();
    } else if (selector === SELECT_SOURCE_KEYMAP) {
      bytes = this.#sourceKeymapBlock();
    } else {
      bytes = this.#statusBlock();
    }
    return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  #statusBlock(): Uint8Array {
    const bytes = new Uint8Array(CONFIG_REPORT_LEN);
    const view = new DataView(bytes.buffer);
    const state = this.#state;
    bytes[0] = CONFIG_PROTOCOL_VERSION;
    bytes[1] = STATUS_KIND;
    bytes[2] = state.state;
    bytes[3] = state.lastError;
    view.setUint16(4, state.advertisementsSeen, true);
    view.setUint16(6, state.hidAdvertisements, true);
    bytes.set(state.lastAddress, 8);
    view.setInt8(14, state.lastRssi);
    bytes[15] = state.connectionCount;
    view.setUint32(16, state.inputReportsReceived, true);
    view.setUint32(20, state.reportsForwarded, true);
    view.setUint16(24, state.panicLine, true);
    bytes[26] = state.panicCount;
    bytes[27] = state.discoveryStep;
    return sealed(bytes);
  }

  #securityBlock(): Uint8Array {
    const bytes = new Uint8Array(CONFIG_REPORT_LEN);
    const view = new DataView(bytes.buffer);
    const state = this.#state;
    bytes[0] = CONFIG_PROTOCOL_VERSION;
    bytes[1] = SECURITY_KIND;
    bytes[12] = state.bondFlags;
    view.setUint16(16, state.notifyHandle, true);
    view.setUint16(18, state.notifyExpected, true);
    bytes[24] = state.stuckKeyReleases;
    bytes[25] = state.livenessProbeFailures;
    bytes[26] = state.livenessProbeError;
    bytes[27] = state.injectRefusal;
    return sealed(bytes);
  }

  #sourceBlock(): Uint8Array {
    const source = this.#state.sources[this.#selectedSource];
    if (!source || source.slot !== this.#selectedSource) {
      throw new SimulatedProtocolError(`missing source slot ${this.#selectedSource}`);
    }
    if (source.name.length > 13) {
      throw new SimulatedProtocolError('source name exceeds fixed capacity');
    }
    const bytes = new Uint8Array(CONFIG_REPORT_LEN);
    bytes[0] = CONFIG_PROTOCOL_VERSION;
    bytes[1] = SOURCE_KIND;
    bytes[2] = 1;
    bytes[3] = source.slot;
    bytes[4] = source.transport;
    bytes[5] = source.state;
    bytes[6] = source.flags;
    bytes[7] = source.profileFlags;
    bytes.set(source.identityAddress, 8);
    const encodedName = new TextEncoder().encode(source.name);
    if (encodedName.length > 13) {
      throw new SimulatedProtocolError('source name exceeds fixed UTF-8 capacity');
    }
    bytes[14] = encodedName.length;
    bytes.set(encodedName, 15);
    return sealed(bytes);
  }

  #fail(code: number): void {
    this.#transfer.state = 3;
    this.#transfer.lastError = code;
  }

  #begin(payload: Uint8Array, frame: DataView): void {
    // A begin always resets, so an abandoned transfer cannot poison the next.
    const target = payload[2];
    const total = frame.getUint16(4, true);
    const declared = frame.getUint32(6, true);
    this.#transfer = { ...this.#transfer, state: 1, lastError: 0, target, expectedLen: total, receivedLen: 0, nextIndex: 0, declaredCrc: declared, computedCrc: 0 };
    if (
      target !== 1 &&
      target !== 2 &&
      target !== TARGET_SOURCE_PROFILE &&
      target !== TARGET_BOND_MANAGEMENT &&
      target !== TARGET_KEYMAP &&
      target !== TARGET_SOURCE_KEYMAP
    ) {
      this.#fail(8);
      return;
    }
    if (total === 0 || total > TRANSFER_CAPACITY) {
      this.#fail(4);
    }
  }

  #chunk(payload: Uint8Array, frame: DataView): void {
    if (this.#transfer.state !== 1) {
      this.#fail(1);
      return;
    }
    const index = frame.getUint16(2, true);
    const count = payload[4];
    if (count === 0 || count > TRANSFER_CHUNK_BYTES) {
      this.#fail(3);
      return;
    }
    if (index !== this.#transfer.nextIndex) {
      this.#fail(2);
      return;
    }
    if (this.#transfer.receivedLen + count > this.#transfer.expectedLen) {
      this.#fail(4);
      return;
    }
    this.#transfer.buffer.set(payload.subarray(5, 5 + count), this.#transfer.receivedLen);
    this.#transfer.receivedLen += count;
    this.#transfer.nextIndex += 1;
  }

  #commit(payload: Uint8Array, frame: DataView): void {
    if (this.#transfer.state !== 1) {
      this.#fail(1);
      return;
    }
    if (this.#transfer.receivedLen !== this.#transfer.expectedLen) {
      this.#fail(5);
      return;
    }
    if (
      payload[2] !== this.#transfer.target ||
      frame.getUint16(4, true) !== this.#transfer.expectedLen ||
      frame.getUint32(6, true) !== this.#transfer.declaredCrc
    ) {
      this.#fail(7);
      return;
    }
    const computed = crc32Ieee(this.#transfer.buffer.subarray(0, this.#transfer.receivedLen));
    if (computed !== this.#transfer.declaredCrc) {
      this.#transfer.computedCrc = computed;
      this.#fail(6);
      return;
    }
    this.#transfer.computedCrc = computed;
    if (this.#transfer.target === TARGET_SOURCE_PROFILE) {
      const payload = this.#transfer.buffer.subarray(0, this.#transfer.receivedLen);
      if (
        payload.length !== 4 ||
        payload[0] !== 2 ||
        payload[1] !== 4 ||
        payload[2] >= 5 ||
        (payload[3] & ~0x07) !== 0
      ) {
        this.#fail(3);
        return;
      }
      if (!isRuntimeSourceSlot(payload[2]) || (payload[2] !== 4 && this.#state.sources[payload[2]].transport === 0)) {
        throw new SimulatedProtocolError(
          `source slot ${payload[2]} is not registered`,
        );
      }
      this.#state.sources[payload[2]].profileFlags = payload[3];
    }
    if (this.#transfer.target === TARGET_BOND_MANAGEMENT) {
      this.#applyBondManagement(this.#transfer.buffer.subarray(0, this.#transfer.receivedLen));
    }
    if (this.#transfer.target === TARGET_KEYMAP) {
      const payload = this.#transfer.buffer.subarray(0, this.#transfer.receivedLen);
      try {
        decodeKeymapPayload(payload);
      } catch {
        this.#fail(3);
        return;
      }
      this.#keymapPayload = payload.slice();
    }
    if (this.#transfer.target === TARGET_SOURCE_KEYMAP) {
      const payload = this.#transfer.buffer.subarray(0, this.#transfer.receivedLen);
      try {
        const decoded = decodeSourceKeymapPayload(payload);
        this.#sourceKeymapPayloads[decoded.slot] = payload.slice();
      } catch {
        this.#fail(3);
        return;
      }
    }
    this.#transfer.state = 2;
    this.#transfer.lastError = 0;
  }

  #applyBondManagement(payload: Uint8Array): void {
    if (payload.length !== BOND_MANAGEMENT_PAYLOAD_LEN || payload[0] !== BOND_MANAGEMENT_VERSION) {
      throw new SimulatedProtocolError('invalid bond-management payload');
    }
    const slot = payload[2];
    if (slot >= 4) {
      throw new SimulatedProtocolError(`bond slot ${slot} is not registered`);
    }
    const source = this.#state.sources[slot];
    if (!source || source.transport === 0) {
      throw new SimulatedProtocolError(`bond slot ${slot} is not registered`);
    }
    if (payload[1] === OP_RENAME) {
      const length = payload[3];
      if (length > BOND_MANAGEMENT_NAME_LEN || payload.slice(4 + length).some((byte) => byte !== 0)) {
        throw new SimulatedProtocolError('bond rename padding is not zero-filled');
      }
      try {
        source.name = new TextDecoder('utf-8', { fatal: true }).decode(payload.slice(4, 4 + length));
      } catch {
        throw new SimulatedProtocolError('bond name is not valid UTF-8');
      }
      return;
    }
    if (payload[1] === OP_DELETE) {
      if (payload[3] !== 0 || payload.slice(4).some((byte) => byte !== 0)) {
        throw new SimulatedProtocolError('delete payload carries name bytes');
      }
      const emptyAddress: [number, number, number, number, number, number] = [0, 0, 0, 0, 0, 0];
      this.#state.sources[slot] = {
        slot,
        transport: 0,
        state: 0,
        flags: 0,
        profileFlags: 0,
        identityAddress: emptyAddress,
        name: '',
      };
      return;
    }
    throw new SimulatedProtocolError(`unknown bond operation ${payload[1]}`);
  }

  /** What the board received, once a commit has been accepted. */
  get committed(): Uint8Array | null {
    return this.#transfer.state === 2
      ? this.#transfer.buffer.slice(0, this.#transfer.receivedLen)
      : null;
  }

  #transferBlock(): Uint8Array {
    const bytes = new Uint8Array(CONFIG_REPORT_LEN);
    const view = new DataView(bytes.buffer);
    const transfer = this.#transfer;
    bytes[0] = CONFIG_PROTOCOL_VERSION;
    bytes[1] = TRANSFER_KIND;
    bytes[2] = transfer.state;
    bytes[3] = transfer.lastError;
    bytes[4] = transfer.target;
    view.setUint16(6, transfer.expectedLen, true);
    view.setUint16(8, transfer.receivedLen, true);
    view.setUint16(10, transfer.nextIndex, true);
    view.setUint32(12, transfer.declaredCrc, true);
    view.setUint32(16, transfer.computedCrc, true);
    return sealed(bytes);
  }

  #keymapBlock(): Uint8Array {
    const chunkCount = Math.ceil(this.#keymapPayload.length / KEYMAP_CHUNK_DATA_LEN);
    const start = this.#selectedKeymapChunk * KEYMAP_CHUNK_DATA_LEN;
    if (start >= this.#keymapPayload.length || this.#selectedKeymapChunk >= chunkCount) {
      throw new SimulatedProtocolError(`keymap chunk ${this.#selectedKeymapChunk} is out of range`);
    }
    const bytes = new Uint8Array(CONFIG_REPORT_LEN);
    const end = Math.min(start + KEYMAP_CHUNK_DATA_LEN, this.#keymapPayload.length);
    bytes[0] = CONFIG_PROTOCOL_VERSION;
    bytes[1] = KEYMAP_KIND;
    bytes[2] = KEYMAP_REPORT_VERSION;
    bytes[3] = this.#selectedKeymapChunk;
    bytes[4] = chunkCount;
    new DataView(bytes.buffer).setUint16(5, this.#keymapPayload.length, true);
    bytes.set(this.#keymapPayload.subarray(start, end), 7);
    return sealed(bytes);
  }

  #sourceKeymapBlock(): Uint8Array {
    const payload = this.#sourceKeymapPayloads[this.#selectedSourceKeymapSlot];
    const chunkCount = Math.ceil(payload.length / SOURCE_KEYMAP_CHUNK_DATA_LEN);
    const start = this.#selectedSourceKeymapChunk * SOURCE_KEYMAP_CHUNK_DATA_LEN;
    if (start >= payload.length || this.#selectedSourceKeymapChunk >= chunkCount) {
      throw new SimulatedProtocolError(
        `source keymap chunk ${this.#selectedSourceKeymapChunk} is out of range`,
      );
    }
    const bytes = new Uint8Array(CONFIG_REPORT_LEN);
    const end = Math.min(start + SOURCE_KEYMAP_CHUNK_DATA_LEN, payload.length);
    bytes[0] = CONFIG_PROTOCOL_VERSION;
    bytes[1] = KEYMAP_KIND;
    bytes[2] = SOURCE_KEYMAP_REPORT_VERSION;
    bytes[3] = this.#selectedSourceKeymapChunk;
    bytes[4] = chunkCount;
    new DataView(bytes.buffer).setUint16(5, payload.length, true);
    bytes[7] = this.#selectedSourceKeymapSlot;
    bytes.set(payload.subarray(start, end), 8);
    return sealed(bytes);
  }
}
