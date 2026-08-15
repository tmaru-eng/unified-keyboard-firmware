/**
 * Decodes the bridge's diagnostics blocks.
 *
 * Every offset here has a counterpart in
 * `firmware/nrf52840-ble-usb/src/diagnostics_report.rs`. Both sides write them
 * by hand, so `diagnostics.test.ts` pins the positions: a field added on one
 * side and not the other does not fail loudly, it silently reports a
 * neighbouring byte, which during bring-up reads as a broken board.
 *
 * The Python tool `read_diagnostics.py` decodes the same blocks and is the
 * implementation that has been run against real hardware. Where the two differ,
 * the Python one is the reference.
 */

import {
  CONFIG_PROTOCOL_VERSION,
  CONFIG_REPORT_ID,
  CONFIG_REPORT_LEN,
  crc32Ieee,
} from './injection';

/** Block kind carrying the bridge's lifecycle and counters. */
export const STATUS_KIND = 1;
/** Block kind carrying pairing state and the report path's refusals. */
export const SECURITY_KIND = 3;
/** Block kind carrying the state of a configuration transfer. */
export const TRANSFER_KIND = 5;
/** Block kind carrying one fixed-capacity source slot. */
export const SOURCE_KIND = 7;
/** Block kind carrying one chunk of the active keymap payload. */
export const KEYMAP_KIND = 8;
/** Selector value asking for the transfer block. */
export const SELECT_TRANSFER = 0xfc;
/** Command selecting which block the next read returns. */
export const UKF_SELECT_PANIC_CHUNK = 101;
/** Selector value asking for the security block. */
export const SELECT_SECURITY = 0xfe;
/** Selector value asking for the source slot selected by the command path. */
export const SELECT_SOURCE = 0xfa;
/** Selector value returned for a selected keymap chunk. */
export const SELECT_KEYMAP = 0xf9;
/** Command selecting a source slot for the next diagnostics read. */
export const UKF_SELECT_SOURCE = 107;
/** Command selecting a keymap chunk for the next diagnostics read. */
export const UKF_SELECT_KEYMAP = 108;
/** Command selecting a source-slot keymap chunk for the next diagnostics read. */
export const UKF_SELECT_SOURCE_KEYMAP = 109;
/** Bytes of keymap payload carried by one diagnostics block. */
export const KEYMAP_CHUNK_DATA_LEN = 21;
/** Maximum number of diagnostics blocks for the fixed keymap payload. */
export const KEYMAP_CHUNK_COUNT_MAX = 7;
/** Version of the keymap diagnostics block. */
export const KEYMAP_REPORT_VERSION = 1;
/** Bytes of source-slot keymap payload carried by one diagnostics block. */
export const SOURCE_KEYMAP_CHUNK_DATA_LEN = 20;
/** Maximum number of source-slot keymap diagnostics blocks. */
export const SOURCE_KEYMAP_CHUNK_COUNT_MAX = 7;
/** Version of a source-slot keymap diagnostics block. */
export const SOURCE_KEYMAP_REPORT_VERSION = 2;
/** Maximum source-slot keymap payload length. */
export const SOURCE_KEYMAP_PAYLOAD_MAX_LEN = 3 + 32 * 4;
/** Version of the source record carried inside a source block. */
export const SOURCE_RECORD_VERSION = 1;
/** Maximum fixed-width source name bytes. */
export const SOURCE_NAME_LEN = 13;

/** Source transport values used by the firmware source block. */
export const SOURCE_TRANSPORT: Readonly<Record<number, string>> = {
  0: 'Unregistered',
  1: 'USB',
  2: 'BLE',
  3: 'Virtual',
};

/** Source-local lifecycle state values used by the firmware source block. */
export const SOURCE_STATE: Readonly<Record<number, string>> = {
  0: 'Unregistered',
  1: 'Disconnected',
  2: 'Connecting',
  3: 'Securing',
  4: 'Discovering',
  5: '接続済み',
  6: 'Failed',
};

/** Source-local lifecycle state that means the input path is ready. */
export const SOURCE_CONNECTED_STATE = 5;

/** BLE central lifecycle, as the firmware's `BridgeState` numbers it. */
export const BRIDGE_STATE: Readonly<Record<number, string>> = {
  0: '起動中',
  1: 'スキャン中',
  2: '接続中',
  3: '暗号化中',
  4: '探索中',
  5: '購読済み',
  6: '失敗',
};

/** High nibble of an error byte: which phase produced it. */
const ERROR_PHASE: Readonly<Record<number, string>> = {
  0x10: 'スキャン',
  0x20: '接続',
  0x30: '暗号化',
  0x40: '探索',
  0x50: '購読中',
};

/**
 * Low nibble of an error byte.
 *
 * Mostly Bluetooth HCI status codes, passed through unchanged. `0x08` and
 * `0x09` are the firmware's own: it overloads them for failures the host layer
 * reports without an HCI status.
 */
const ERROR_REASON: Readonly<Record<number, string>> = {
  0x01: '不明なHCIコマンド',
  0x02: '不明な接続識別子',
  0x03: '見つからない（サービス/特性/記述子）',
  0x04: 'ページタイムアウト',
  0x05: 'ATTエラー（相手が属性へのアクセスを拒否）',
  0x06: 'PIN/鍵が無い',
  0x07: 'メモリ/バッファ不足',
  0x08: 'タイムアウト',
  0x09: 'リンク切断',
  0x0a: '未対応の操作',
  0x0b: '状態が不正',
  0x0c: '応答が想定外',
  0x0d: 'コントローラ側の失敗',
  0x0e: '相手が拒否/切断',
  0x0f: 'ホスト側の失敗',
};

/** Which of the nine discovery steps a failure stopped at. */
const DISCOVERY_STEP: Readonly<Record<number, string>> = {
  0: '未実行',
  1: 'HIDサービス検索',
  2: 'HIDサービスの選択',
  3: 'レポートマップ特性の取得',
  4: 'レポートマップの読み出し',
  5: '特性一覧の取得',
  6: 'レポート参照記述子の読み出し',
  7: '購読対象の決定',
  8: '購読対象の特性を一覧から特定',
  9: '通知の購読',
};

const BOND_FLAGS: ReadonlyArray<[number, string]> = [
  [1 << 0, '起動時に鍵を読み込んだ'],
  [1 << 1, 'この起動で鍵を保存した'],
  [1 << 2, '直近の相手はボンド済み'],
  [1 << 3, 'ペアリングしたが鍵が得られなかった'],
  [1 << 4, '鍵の書き込みに失敗した'],
];

const INJECT_REFUSAL: Readonly<Record<number, string>> = {
  0: 'なし',
  1: 'レポート長が8バイトでない',
  2: '源が切り離されている',
  3: '源の番号が範囲外',
  4: 'USB書き込みが拒否された',
};

/** Raised when a payload is not the block it was expected to be. */
export class DiagnosticsError extends Error {}

/** Renders the packed phase/reason byte as the sentence it stands for. */
export function describeError(code: number): string {
  if (code === 0) {
    return 'なし';
  }
  const phase = ERROR_PHASE[code & 0xf0];
  const reason = ERROR_REASON[code & 0x0f];
  if (phase === undefined && reason === undefined) {
    return `0x${code.toString(16).padStart(2, '0')}（未定義）`;
  }
  return `0x${code.toString(16).padStart(2, '0')} ${phase ?? '不明な段階'} / ${reason ?? '不明な理由'}`;
}

/** Renders how far GATT discovery got. */
export function describeDiscoveryStep(step: number): string {
  return `${step} ${DISCOVERY_STEP[step] ?? '未定義'}`;
}

function assertCrc(payload: Uint8Array): void {
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  const expected = view.getUint32(28, true);
  const actual = crc32Ieee(payload.subarray(0, 28));
  if (expected !== actual) {
    throw new DiagnosticsError(
      `CRCが一致しません（送信 0x${expected.toString(16)}、計算 0x${actual.toString(16)}）`,
    );
  }
}

function assertHeader(payload: Uint8Array, kind: number): void {
  if (payload.length !== CONFIG_REPORT_LEN) {
    throw new DiagnosticsError(`${CONFIG_REPORT_LEN}バイトではありません（${payload.length}）`);
  }
  if (payload[0] !== CONFIG_PROTOCOL_VERSION) {
    throw new DiagnosticsError(`対応していないプロトコルバージョン ${payload[0]}`);
  }
  if (payload[1] !== kind) {
    throw new DiagnosticsError(`種別が違います（${payload[1]}、期待 ${kind}）`);
  }
  assertCrc(payload);
}

/**
 * Picks the framing the device actually used.
 *
 * Whether the browser keeps the report ID in the returned buffer depends on the
 * platform. Guessing wrong misaligns every field while still producing a
 * plausible-looking report, so the CRC decides instead.
 */
export function selectFraming(raw: Uint8Array): Uint8Array {
  if (raw.length !== CONFIG_REPORT_LEN && raw.length !== CONFIG_REPORT_LEN + 1) {
    throw new DiagnosticsError(
      `unexpected diagnostics report length ${raw.length} (expected ${CONFIG_REPORT_LEN} or ${CONFIG_REPORT_LEN + 1})`,
    );
  }
  if (raw.length === CONFIG_REPORT_LEN + 1 && raw[0] !== CONFIG_REPORT_ID) {
    throw new DiagnosticsError(
      `unexpected report ID prefix ${raw[0]} (expected ${CONFIG_REPORT_ID})`,
    );
  }
  const candidates = [raw.subarray(0, CONFIG_REPORT_LEN), raw.subarray(1, 1 + CONFIG_REPORT_LEN)];
  for (const candidate of candidates) {
    if (candidate.length !== CONFIG_REPORT_LEN) {
      continue;
    }
    try {
      assertCrc(candidate);
      return candidate;
    } catch {
      // Try the other framing before giving up.
    }
  }
  throw new DiagnosticsError('どの解釈でもCRCが一致しませんでした');
}

/** The bridge's lifecycle and counters. */
export interface StatusBlock {
  state: number;
  stateLabel: string;
  lastError: number;
  lastErrorLabel: string;
  advertisementsSeen: number;
  hidAdvertisements: number;
  lastAddress: string;
  lastRssi: number;
  connectionCount: number;
  inputReportsReceived: number;
  reportsForwarded: number;
  panicLine: number;
  panicCount: number;
  discoveryStep: number;
  discoveryStepLabel: string;
}

/** Decodes and validates the status block. */
export function decodeStatus(payload: Uint8Array): StatusBlock {
  assertHeader(payload, STATUS_KIND);
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  const address = Array.from(payload.subarray(8, 14))
    .reverse()
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join(':');
  const lastError = payload[3];
  const discoveryStep = payload[27];
  return {
    state: payload[2],
    stateLabel: BRIDGE_STATE[payload[2]] ?? `未知(${payload[2]})`,
    lastError,
    lastErrorLabel: describeError(lastError),
    advertisementsSeen: view.getUint16(4, true),
    hidAdvertisements: view.getUint16(6, true),
    lastAddress: address,
    lastRssi: view.getInt8(14),
    connectionCount: payload[15],
    inputReportsReceived: view.getUint32(16, true),
    reportsForwarded: view.getUint32(20, true),
    panicLine: view.getUint16(24, true),
    panicCount: payload[26],
    discoveryStep,
    discoveryStepLabel: describeDiscoveryStep(discoveryStep),
  };
}

/** Pairing state and the report path's refusals. */
export interface SecurityBlock {
  disconnectReason: number;
  pairingFailure: number;
  passkey: number;
  passkeySerial: number;
  bondFlags: number;
  bondSummary: string;
  hogpRefusal: number;
  notifyRefusal: number;
  notifyHandle: number;
  notifyExpected: number;
  bondFlashError: number;
  bondFlashErrno: number;
  usbHogpRefusal: number;
  linkSetupFailed: boolean;
  stuckKeyReleases: number;
  livenessProbeFailures: number;
  livenessProbeError: number;
  stuckKeySummary: string;
  injectRefusal: number;
  injectRefusalLabel: string;
}

/** Renders the bond bit field as the sentences it stands for. */
export function describeBondFlags(flags: number): string {
  if (flags === 0) {
    return '保存された鍵は無い';
  }
  return BOND_FLAGS.filter(([bit]) => (flags & bit) !== 0)
    .map(([, text]) => text)
    .join('／');
}

/**
 * Renders the stuck-key watchdog's record.
 *
 * Silence is the expected state. A non-zero release count means the keyboard
 * stopped answering while the host was holding a key and the bridge let go on
 * its behalf — the failure this watchdog exists to end, now visible instead of
 * only felt.
 */
export function describeStuckKey(releases: number, probeFailures: number, probeError: number): string {
  if (releases === 0 && probeFailures === 0) {
    return '発動なし';
  }
  return `全解放${releases}回 / 生存確認の無応答${probeFailures}回 / 直近の失敗理由 ${describeError(probeError)}`;
}

/** Decodes and validates the security block. */
export function decodeSecurity(payload: Uint8Array): SecurityBlock {
  assertHeader(payload, SECURITY_KIND);
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  const bondFlags = payload[12];
  const stuckKeyReleases = payload[24];
  const livenessProbeFailures = payload[25];
  const livenessProbeError = payload[26];
  const injectRefusal = payload[27];
  return {
    disconnectReason: payload[2],
    pairingFailure: payload[3],
    passkey: view.getUint32(4, true),
    passkeySerial: view.getUint32(8, true),
    bondFlags,
    bondSummary: describeBondFlags(bondFlags),
    hogpRefusal: payload[13],
    notifyRefusal: payload[14],
    notifyHandle: view.getUint16(16, true),
    notifyExpected: view.getUint16(18, true),
    bondFlashError: payload[15],
    bondFlashErrno: view.getUint16(20, true),
    usbHogpRefusal: payload[22],
    linkSetupFailed: payload[23] !== 0,
    stuckKeyReleases,
    livenessProbeFailures,
    livenessProbeError,
    stuckKeySummary: describeStuckKey(stuckKeyReleases, livenessProbeFailures, livenessProbeError),
    injectRefusal,
    injectRefusalLabel: INJECT_REFUSAL[injectRefusal] ?? `未定義(${injectRefusal})`,
  };
}

/** Builds the report that tells the firmware which block to return next. */
export function buildSelectBlock(selector: number): Uint8Array {
  const bytes = new Uint8Array(CONFIG_REPORT_LEN);
  bytes[0] = CONFIG_PROTOCOL_VERSION;
  bytes[1] = UKF_SELECT_PANIC_CHUNK;
  bytes[2] = selector;
  new DataView(bytes.buffer).setUint32(28, crc32Ieee(bytes.subarray(0, 28)), true);
  return bytes;
}

/** Builds the explicit slot selector used before a source block read. */
export function buildSelectSource(slot: number): Uint8Array {
  if (!Number.isInteger(slot) || slot < 0 || slot >= 5) {
    throw new RangeError(`source slot must be an integer from 0 through 4: ${slot}`);
  }
  const bytes = new Uint8Array(CONFIG_REPORT_LEN);
  bytes[0] = CONFIG_PROTOCOL_VERSION;
  bytes[1] = UKF_SELECT_SOURCE;
  bytes[2] = slot;
  new DataView(bytes.buffer).setUint32(28, crc32Ieee(bytes.subarray(0, 28)), true);
  return bytes;
}

/** Builds the explicit keymap-chunk selector used before a payload read. */
export function buildSelectKeymapChunk(chunk: number): Uint8Array {
  if (!Number.isInteger(chunk) || chunk < 0 || chunk >= KEYMAP_CHUNK_COUNT_MAX) {
    throw new RangeError(`keymap chunk must be an integer from 0 through ${KEYMAP_CHUNK_COUNT_MAX - 1}: ${chunk}`);
  }
  const bytes = new Uint8Array(CONFIG_REPORT_LEN);
  bytes[0] = CONFIG_PROTOCOL_VERSION;
  bytes[1] = UKF_SELECT_KEYMAP;
  bytes[2] = chunk;
  new DataView(bytes.buffer).setUint32(28, crc32Ieee(bytes.subarray(0, 28)), true);
  return bytes;
}

/** Builds the selector for one source slot's keymap chunk. */
export function buildSelectSourceKeymapChunk(slot: number, chunk: number): Uint8Array {
  if (!Number.isInteger(slot) || slot < 0 || slot >= 5) {
    throw new RangeError(`source keymap slot must be an integer from 0 through 4: ${slot}`);
  }
  if (!Number.isInteger(chunk) || chunk < 0 || chunk >= SOURCE_KEYMAP_CHUNK_COUNT_MAX) {
    throw new RangeError(
      `source keymap chunk must be an integer from 0 through ${SOURCE_KEYMAP_CHUNK_COUNT_MAX - 1}: ${chunk}`,
    );
  }
  const bytes = new Uint8Array(CONFIG_REPORT_LEN);
  bytes[0] = CONFIG_PROTOCOL_VERSION;
  bytes[1] = UKF_SELECT_SOURCE_KEYMAP;
  bytes[2] = slot;
  bytes[3] = chunk;
  new DataView(bytes.buffer).setUint32(28, crc32Ieee(bytes.subarray(0, 28)), true);
  return bytes;
}

/** One chunk of the active keymap payload. */
export interface KeymapChunkBlock {
  kind: number;
  version: number;
  chunkIndex: number;
  chunkCount: number;
  payloadLength: number;
  data: Uint8Array;
  sourceSlot?: number;
}

/** Decodes and strictly validates one keymap payload chunk. */
export function decodeKeymapChunk(payload: Uint8Array): KeymapChunkBlock {
  assertHeader(payload, KEYMAP_KIND);
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  const version = payload[2];
  if (version !== KEYMAP_REPORT_VERSION) {
    throw new DiagnosticsError(`unsupported keymap report version ${version}`);
  }
  const chunkIndex = payload[3];
  const chunkCount = payload[4];
  if (chunkCount === 0 || chunkCount > KEYMAP_CHUNK_COUNT_MAX || chunkIndex >= chunkCount) {
    throw new DiagnosticsError(`invalid keymap chunk ${chunkIndex}/${chunkCount}`);
  }
  const payloadLength = view.getUint16(5, true);
  const maxLength = 2 + 32 * 4;
  if (payloadLength < 2 || payloadLength > maxLength) {
    throw new DiagnosticsError(`invalid keymap payload length ${payloadLength}`);
  }
  const expectedChunkCount = Math.ceil(payloadLength / KEYMAP_CHUNK_DATA_LEN);
  if (chunkCount !== expectedChunkCount) {
    throw new DiagnosticsError(
      `keymap chunk count ${chunkCount} does not match payload length ${payloadLength}`,
    );
  }
  const start = chunkIndex * KEYMAP_CHUNK_DATA_LEN;
  if (start >= payloadLength) {
    throw new DiagnosticsError(`keymap chunk starts beyond payload: ${start}/${payloadLength}`);
  }
  const meaningful = Math.min(KEYMAP_CHUNK_DATA_LEN, payloadLength - start);
  if (payload.subarray(7 + meaningful, 28).some((byte) => byte !== 0)) {
    throw new DiagnosticsError('keymap chunk padding is not zero');
  }
  return {
    kind: payload[1],
    version,
    chunkIndex,
    chunkCount,
    payloadLength,
    data: payload.slice(7, 7 + meaningful),
  };
}

/** Decodes one source-slot keymap payload chunk. */
export function decodeSourceKeymapChunk(payload: Uint8Array): KeymapChunkBlock {
  assertHeader(payload, KEYMAP_KIND);
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  const version = payload[2];
  if (version !== SOURCE_KEYMAP_REPORT_VERSION) {
    throw new DiagnosticsError(`unsupported source keymap report version ${version}`);
  }
  const chunkIndex = payload[3];
  const chunkCount = payload[4];
  if (
    chunkCount === 0 ||
    chunkCount > SOURCE_KEYMAP_CHUNK_COUNT_MAX ||
    chunkIndex >= chunkCount
  ) {
    throw new DiagnosticsError(`invalid source keymap chunk ${chunkIndex}/${chunkCount}`);
  }
  const payloadLength = view.getUint16(5, true);
  if (payloadLength < 3 || payloadLength > SOURCE_KEYMAP_PAYLOAD_MAX_LEN) {
    throw new DiagnosticsError(`invalid source keymap payload length ${payloadLength}`);
  }
  const sourceSlot = payload[7];
  if (sourceSlot >= 5) {
    throw new DiagnosticsError(`unknown source keymap slot ${sourceSlot}`);
  }
  const expectedChunkCount = Math.ceil(payloadLength / SOURCE_KEYMAP_CHUNK_DATA_LEN);
  if (chunkCount !== expectedChunkCount) {
    throw new DiagnosticsError(
      `source keymap chunk count ${chunkCount} does not match payload length ${payloadLength}`,
    );
  }
  const start = chunkIndex * SOURCE_KEYMAP_CHUNK_DATA_LEN;
  if (start >= payloadLength) {
    throw new DiagnosticsError(`source keymap chunk starts beyond payload: ${start}/${payloadLength}`);
  }
  const meaningful = Math.min(SOURCE_KEYMAP_CHUNK_DATA_LEN, payloadLength - start);
  if (payload.subarray(8 + meaningful, 28).some((byte) => byte !== 0)) {
    throw new DiagnosticsError('source keymap chunk padding is not zero');
  }
  return {
    kind: payload[1],
    version,
    chunkIndex,
    chunkCount,
    payloadLength,
    sourceSlot,
    data: payload.slice(8, 8 + meaningful),
  };
}

/** One decoded 32-byte source diagnostics block. */
export interface SourceBlock {
  kind: number;
  slot: number;
  transport: number;
  transportLabel: string;
  state: number;
  stateLabel: string;
  identityAddress: string | null;
  irkPresent: boolean;
  profileFlags: number;
  name: string;
}

function sourceAddress(bytes: Uint8Array): string {
  return Array.from(bytes)
    .reverse()
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join(':');
}

/** Decodes and strictly validates one source diagnostics block. */
export function decodeSource(payload: Uint8Array): SourceBlock {
  assertHeader(payload, SOURCE_KIND);
  const slot = payload[3];
  if (slot >= 5) {
    throw new DiagnosticsError(`unknown source slot ${slot}`);
  }
  if (payload[2] !== SOURCE_RECORD_VERSION) {
    throw new DiagnosticsError(`unsupported source record version ${payload[2]}`);
  }

  const transport = payload[4];
  if (SOURCE_TRANSPORT[transport] === undefined) {
    throw new DiagnosticsError(`unknown source transport ${transport}`);
  }
  const state = payload[5];
  if (SOURCE_STATE[state] === undefined) {
    throw new DiagnosticsError(`unknown source state ${state}`);
  }
  const flags = payload[6];
  if ((flags & ~0x03) !== 0) {
    throw new DiagnosticsError(`reserved source flags are set: 0x${flags.toString(16)}`);
  }
  const profileFlags = payload[7];
  if ((profileFlags & ~0x07) !== 0) {
    throw new DiagnosticsError(`reserved profile flags are set: 0x${profileFlags.toString(16)}`);
  }

  const addressBytes = payload.subarray(8, 14);
  const hasAddress = (flags & 0x01) !== 0;
  if (!hasAddress && addressBytes.some((byte) => byte !== 0)) {
    throw new DiagnosticsError('an absent identity address must be zero-filled');
  }
  if (!hasAddress && (flags & 0x02) !== 0) {
    throw new DiagnosticsError('an IRK cannot be present without an identity address');
  }
  if ((transport === 0) !== (state === 0)) {
    throw new DiagnosticsError('source transport and lifecycle state disagree');
  }

  const nameLength = payload[14];
  if (nameLength > SOURCE_NAME_LEN) {
    throw new DiagnosticsError(`source name is too long: ${nameLength}`);
  }
  if (payload.subarray(15 + nameLength, 28).some((byte) => byte !== 0)) {
    throw new DiagnosticsError('source name padding is not zero-filled');
  }
  if (state === 0 && (flags !== 0 || profileFlags !== 0 || nameLength !== 0)) {
    throw new DiagnosticsError('unregistered source carries registered fields');
  }

  let name: string;
  try {
    name = new TextDecoder('utf-8', { fatal: true }).decode(
      payload.subarray(15, 15 + nameLength),
    );
  } catch {
    throw new DiagnosticsError('source name is not valid UTF-8');
  }

  return {
    kind: payload[1],
    slot,
    transport,
    transportLabel: SOURCE_TRANSPORT[transport],
    state,
    stateLabel: SOURCE_STATE[state],
    identityAddress: hasAddress ? sourceAddress(addressBytes) : null,
    irkPresent: (flags & 0x02) !== 0,
    profileFlags,
    name,
  };
}

/** How far a configuration transfer has got. */
export const TRANSFER_STATE: Readonly<Record<number, string>> = {
  0: '待機中',
  1: '受信中',
  2: '完了',
  3: '失敗',
};

/**
 * Why a transfer was refused.
 *
 * Each value names one refusal. Collapsing them would leave "the board said no"
 * and nothing to act on, which is the shape every hard bug in this project has
 * taken so far.
 */
export const TRANSFER_ERROR: Readonly<Record<number, string>> = {
  0: 'なし',
  1: '開始せずに断片が来た',
  2: '断片の番号が期待と違う',
  3: '断片の長さが範囲外',
  4: '宣言された長さが容量を超える',
  5: '全部受け取る前に確定が来た',
  6: '確定時のCRCが受信内容と一致しない',
  7: '確定の内容が開始時の宣言と違う',
  8: '未知の書き込み先',
};

/** State of the configuration transfer staging area. */
export interface TransferBlock {
  state: number;
  stateLabel: string;
  lastError: number;
  lastErrorLabel: string;
  target: number;
  expectedLen: number;
  receivedLen: number;
  nextIndex: number;
  declaredCrc: number;
  computedCrc: number;
  summary: string;
}

/** Renders the transfer state the way a person reads it during a write. */
export function describeTransfer(block: Omit<TransferBlock, 'summary' | 'stateLabel' | 'lastErrorLabel'>): string {
  switch (block.state) {
    case 0:
      return '待機中';
    case 1:
      return `受信中 ${block.receivedLen}/${block.expectedLen}バイト、次の断片 ${block.nextIndex}`;
    case 2:
      return `完了 ${block.receivedLen}バイト`;
    default:
      return `失敗 ${block.lastError} ${TRANSFER_ERROR[block.lastError] ?? '未定義'}`;
  }
}

/** Decodes and validates the transfer block. */
export function decodeTransfer(payload: Uint8Array): TransferBlock {
  assertHeader(payload, TRANSFER_KIND);
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  const fields = {
    state: payload[2],
    lastError: payload[3],
    target: payload[4],
    expectedLen: view.getUint16(6, true),
    receivedLen: view.getUint16(8, true),
    nextIndex: view.getUint16(10, true),
    declaredCrc: view.getUint32(12, true),
    computedCrc: view.getUint32(16, true),
  };
  return {
    ...fields,
    stateLabel: TRANSFER_STATE[fields.state] ?? `未知(${fields.state})`,
    lastErrorLabel: TRANSFER_ERROR[fields.lastError] ?? `未定義(${fields.lastError})`,
    summary: describeTransfer(fields),
  };
}
