/**
 * WebHID access to the bridge's vendor configuration interface.
 *
 * The board exposes two HID interfaces. Only the one whose usage page is
 * `0xFF00` and usage is `0x20` accepts configuration feature reports; the other
 * is the keyboard the host is already using. Selecting the wrong one would send
 * configuration bytes to a keyboard endpoint.
 */

import {
  SELECT_SECURITY,
  buildSelectSource,
  SELECT_TRANSFER,
  type SourceBlock,
  type TransferBlock,
  decodeTransfer,
  type SecurityBlock,
  type StatusBlock,
  buildSelectBlock,
  buildSelectKeymapChunk,
  buildSelectSourceKeymapChunk,
  decodeKeymapChunk,
  decodeSourceKeymapChunk,
  decodeSecurity,
  decodeSource,
  decodeStatus,
  KEYMAP_CHUNK_COUNT_MAX,
  KEYMAP_CHUNK_DATA_LEN,
  SOURCE_KEYMAP_CHUNK_COUNT_MAX,
  SOURCE_KEYMAP_CHUNK_DATA_LEN,
  type KeymapChunkBlock,
  selectFraming,
} from './diagnostics';
import { buildPairingMethodReport, buildPairingModeReport } from './commands';
import {
  TARGET_SCRATCH,
  TARGET_KEYMAP,
  TARGET_SOURCE_KEYMAP,
  TARGET_SOURCE_PROFILE,
  buildWriteBegin,
  buildWriteChunk,
  buildWriteCommit,
  splitPayload,
} from './transfer';
import {
  buildDeletePayload,
  buildRenamePayload,
  TARGET_BOND_MANAGEMENT,
} from './bondManagement';
import { CONFIG_REPORT_ID, buildInjectionPayload, buildSourceReport } from './injection';
import { encodeSourceProfilePayload, type Profile } from './profile';
import {
  decodeKeymapPayload,
  decodeSourceKeymapPayload,
  encodeKeymapPayload,
  encodeSourceKeymapPayload,
  type KeymapRule,
} from './keymap';

/** Approved application identity for the independent bridge prototype. */
export const USB_VENDOR_ID = 0x1209;
/** Approved application product identity. */
export const USB_PRODUCT_ID = 0x0001;
/** Legacy factory identity, still accepted by the reset and injection tools. */
export const LEGACY_VENDOR_ID = 0xcafe;
/** Legacy factory product identity. */
export const LEGACY_PRODUCT_ID = 0xbaf2;

const CONFIG_USAGE_PAGE = 0xff00;
const CONFIG_USAGE = 0x20;

/** Minimal shape of the WebHID device this module needs. */
export interface ConfigHidDevice {
  readonly opened: boolean;
  readonly vendorId?: number;
  readonly productId?: number;
  readonly productName?: string;
  readonly collections: ReadonlyArray<{ usagePage?: number; usage?: number }>;
  open(): Promise<void>;
  sendFeatureReport(reportId: number, data: BufferSource): Promise<void>;
  receiveFeatureReport(reportId: number): Promise<DataView>;
}

/** Returns whether a device exposes the vendor configuration collection. */
export function isConfigInterface(device: ConfigHidDevice): boolean {
  return device.collections.some(
    (collection) =>
      collection.usagePage === CONFIG_USAGE_PAGE && collection.usage === CONFIG_USAGE,
  );
}

/** Returns whether a vendor configuration collection belongs to this bridge. */
export function isApprovedConfigInterface(device: ConfigHidDevice): boolean {
  if (!isConfigInterface(device)) {
    return false;
  }
  return (
    (device.vendorId === USB_VENDOR_ID && device.productId === USB_PRODUCT_ID) ||
    (device.vendorId === LEGACY_VENDOR_ID && device.productId === LEGACY_PRODUCT_ID)
  );
}

/**
 * Picks the single configuration interface from an enumeration.
 *
 * Ambiguity is rejected rather than guessed. Two matching devices means two
 * boards are attached, and picking one silently would send keystrokes to
 * whichever happened to sort first.
 */
export function selectConfigInterface<T extends ConfigHidDevice>(devices: readonly T[]): T {
  const matches = devices.filter(isApprovedConfigInterface);
  if (matches.length !== 1) {
    throw new Error(
      `設定インターフェースがちょうど1つ見つかりません（${matches.length}個）。` +
        'ブリッジを1台だけ接続してください。',
    );
  }
  return matches[0];
}

/** Filters passed to the browser's device picker. */
export const HID_REQUEST_FILTERS = [
  { vendorId: USB_VENDOR_ID, productId: USB_PRODUCT_ID, usagePage: CONFIG_USAGE_PAGE, usage: CONFIG_USAGE },
  { vendorId: LEGACY_VENDOR_ID, productId: LEGACY_PRODUCT_ID, usagePage: CONFIG_USAGE_PAGE, usage: CONFIG_USAGE },
];

/** The subset of WebHID this application uses. */
interface HidApi {
  getDevices(): Promise<ConfigHidDevice[]>;
  requestDevice(options: unknown): Promise<ConfigHidDevice[]>;
}

function hidApi(): HidApi {
  const hid = (navigator as Navigator & { hid?: HidApi }).hid;
  if (!hid) {
    throw new Error(
      'このブラウザは WebHID に対応していません。Chrome か Edge で開いてください。' +
        'アプリ内のブラウザでは選択画面が出ません。',
    );
  }
  return hid;
}

/**
 * Obtains the configuration interface, prompting only when it has to.
 *
 * Permission survives a reload, so asking again every time turns a working
 * connection into a dialog the operator has to dismiss. The bridge's own
 * predecessor does the same: it looks at already-granted devices first and only
 * opens the picker when none of them is the configuration interface.
 */
export async function requestConfigDevice(): Promise<ConfigHidDevice> {
  const hid = hidApi();
  const granted = (await hid.getDevices()).filter(isApprovedConfigInterface);
  const devices =
    granted.length > 0 ? granted : await hid.requestDevice({ filters: HID_REQUEST_FILTERS });
  if (devices.length === 0) {
    throw new Error(
      'デバイスが選択されませんでした。選択画面でブリッジの設定インターフェースを選んでください。',
    );
  }
  const selected = selectConfigInterface(devices);
  if (!selected.opened) {
    await selected.open();
  }
  return selected;
}

/**
 * Reads one diagnostics block from the device.
 *
 * The configuration interface has a single report ID, so which block comes back
 * is decided by what was selected before the read. The status block is the
 * default and the firmware resets the selector after every read, which is why
 * the security block needs its selector sent each time.
 */
async function readBlock(device: ConfigHidDevice): Promise<Uint8Array> {
  if (!device.opened) {
    await device.open();
  }
  const raw = await device.receiveFeatureReport(CONFIG_REPORT_ID);
  return selectFraming(new Uint8Array(raw.buffer, raw.byteOffset, raw.byteLength));
}

/** Reads the bridge's lifecycle and counters. */
export async function readStatus(device: ConfigHidDevice): Promise<StatusBlock> {
  return decodeStatus(await readBlock(device));
}

/** Reads pairing state and the report path's refusals. */
export async function readSecurity(device: ConfigHidDevice): Promise<SecurityBlock> {
  if (!device.opened) {
    await device.open();
  }
  await device.sendFeatureReport(CONFIG_REPORT_ID, buildSelectBlock(SELECT_SECURITY));
  return decodeSecurity(await readBlock(device));
}

/** Reads one source slot after selecting it explicitly. */
export async function readSource(device: ConfigHidDevice, slot: number): Promise<SourceBlock> {
  if (!device.opened) {
    await device.open();
  }
  await device.sendFeatureReport(CONFIG_REPORT_ID, buildSelectSource(slot));
  return decodeSource(await readBlock(device));
}

/** Reads all four registration slots and the virtual slot in fixed order. */
export async function readSources(device: ConfigHidDevice): Promise<SourceBlock[]> {
  const sources: SourceBlock[] = [];
  for (let slot = 0; slot < 5; slot += 1) {
    sources.push(await readSource(device, slot));
  }
  return sources;
}

/** Reads and validates the complete active keymap payload. */
export async function readKeymap(device: ConfigHidDevice): Promise<KeymapRule[]> {
  if (!device.opened) {
    await device.open();
  }
  const chunks: KeymapChunkBlock[] = [];
  let payloadLength: number | undefined;
  let chunkCount: number | undefined;
  for (let index = 0; index < KEYMAP_CHUNK_COUNT_MAX; index += 1) {
    await device.sendFeatureReport(CONFIG_REPORT_ID, buildSelectKeymapChunk(index));
    const chunk = decodeKeymapChunk(await readBlock(device));
    if (chunk.chunkIndex !== index) {
      throw new Error(`keymap chunk order changed: expected ${index}, got ${chunk.chunkIndex}`);
    }
    if (payloadLength === undefined) {
      payloadLength = chunk.payloadLength;
      chunkCount = chunk.chunkCount;
    } else if (chunk.payloadLength !== payloadLength || chunk.chunkCount !== chunkCount) {
      throw new Error('keymap chunk metadata changed during read');
    }
    chunks.push(chunk);
    if (chunks.length === chunk.chunkCount) {
      break;
    }
  }
  if (payloadLength === undefined || chunkCount === undefined || chunks.length !== chunkCount) {
    throw new Error('keymap read ended before all chunks arrived');
  }
  const payload = new Uint8Array(payloadLength);
  for (const chunk of chunks) {
    payload.set(chunk.data, chunk.chunkIndex * KEYMAP_CHUNK_DATA_LEN);
  }
  return decodeKeymapPayload(payload);
}

/** Reads and validates one source slot's complete keymap payload. */
export async function readSourceKeymap(
  device: ConfigHidDevice,
  slot: number,
): Promise<KeymapRule[]> {
  if (!device.opened) {
    await device.open();
  }
  const chunks: KeymapChunkBlock[] = [];
  let payloadLength: number | undefined;
  let chunkCount: number | undefined;
  for (let index = 0; index < SOURCE_KEYMAP_CHUNK_COUNT_MAX; index += 1) {
    await device.sendFeatureReport(CONFIG_REPORT_ID, buildSelectSourceKeymapChunk(slot, index));
    const chunk = decodeSourceKeymapChunk(await readBlock(device));
    if (chunk.sourceSlot !== slot || chunk.chunkIndex !== index) {
      throw new Error(`source keymap chunk order changed for slot ${slot}`);
    }
    if (payloadLength === undefined) {
      payloadLength = chunk.payloadLength;
      chunkCount = chunk.chunkCount;
    } else if (chunk.payloadLength !== payloadLength || chunk.chunkCount !== chunkCount) {
      throw new Error('source keymap chunk metadata changed during read');
    }
    chunks.push(chunk);
    if (chunks.length === chunk.chunkCount) {
      break;
    }
  }
  if (payloadLength === undefined || chunkCount === undefined || chunks.length !== chunkCount) {
    throw new Error('source keymap read ended before all chunks arrived');
  }
  const payload = new Uint8Array(payloadLength);
  for (const chunk of chunks) {
    payload.set(chunk.data, chunk.chunkIndex * SOURCE_KEYMAP_CHUNK_DATA_LEN);
  }
  const decoded = decodeSourceKeymapPayload(payload);
  if (decoded.slot !== slot) {
    throw new Error(`source keymap payload belongs to slot ${decoded.slot}, not ${slot}`);
  }
  return decoded.rules;
}

/**
 * Opens or closes the bridge to keyboards it has not bonded with.
 *
 * Worth doing deliberately: a bridge left open adopts whatever advertises
 * nearby, and for a keyboard bridge that means someone else's device becoming a
 * trusted source of keystrokes.
 */
export async function setPairingMode(device: ConfigHidDevice, open: boolean): Promise<void> {
  if (!device.opened) {
    await device.open();
  }
  await device.sendFeatureReport(CONFIG_REPORT_ID, buildPairingModeReport(open));
}

/** Chooses passkey entry over Just Works for the next pairing. */
export async function setPairingMethod(
  device: ConfigHidDevice,
  requirePasskey: boolean,
): Promise<void> {
  if (!device.opened) {
    await device.open();
  }
  await device.sendFeatureReport(CONFIG_REPORT_ID, buildPairingMethodReport(requirePasskey));
}

/** Reads how far a configuration transfer has got. */
export async function readTransfer(device: ConfigHidDevice): Promise<TransferBlock> {
  if (!device.opened) {
    await device.open();
  }
  await device.sendFeatureReport(CONFIG_REPORT_ID, buildSelectBlock(SELECT_TRANSFER));
  return decodeTransfer(await readBlock(device));
}

/**
 * Writes a payload the board cannot receive in one report.
 *
 * Sends the declaration, then every slice in order, then the commit. The board
 * applies nothing until that commit proves the whole payload arrived, so a
 * transfer interrupted anywhere leaves it as it was.
 *
 * The transfer block is read back afterwards rather than trusting the writes to
 * have landed. Feature reports report a USB-level result, which says the report
 * reached the interface — not that the board accepted what was in it.
 */
export async function writeConfig(
  device: ConfigHidDevice,
  payload: Uint8Array,
  target: number = TARGET_SCRATCH,
): Promise<TransferBlock> {
  if (!device.opened) {
    await device.open();
  }
  await device.sendFeatureReport(CONFIG_REPORT_ID, buildWriteBegin(target, payload));
  let index = 0;
  for (const chunk of splitPayload(payload)) {
    await device.sendFeatureReport(CONFIG_REPORT_ID, buildWriteChunk(index, chunk));
    index += 1;
  }
  await device.sendFeatureReport(CONFIG_REPORT_ID, buildWriteCommit(target, payload));
  return readTransfer(device);
}

/** Writes one source-specific profile through the explicit target/payload path. */
export async function writeSourceProfile(
  device: ConfigHidDevice,
  slot: number,
  profile: Profile,
): Promise<TransferBlock> {
  return writeConfig(device, encodeSourceProfilePayload(slot, profile), TARGET_SOURCE_PROFILE);
}

/** Writes one volatile single-layer keymap through the generic transfer. */
export async function writeKeymap(
  device: ConfigHidDevice,
  rules: readonly KeymapRule[],
): Promise<TransferBlock> {
  return writeConfig(device, encodeKeymapPayload(rules), TARGET_KEYMAP);
}

/** Writes one source-specific keymap through target 6. */
export async function writeSourceKeymap(
  device: ConfigHidDevice,
  slot: number,
  rules: readonly KeymapRule[],
): Promise<TransferBlock> {
  return writeConfig(device, encodeSourceKeymapPayload(slot, rules), TARGET_SOURCE_KEYMAP);
}

/** Renames one registered BLE source and verifies transfer acceptance. */
export async function renameBond(
  device: ConfigHidDevice,
  slot: number,
  name: string,
): Promise<TransferBlock> {
  return writeConfig(device, buildRenamePayload(slot, name), TARGET_BOND_MANAGEMENT);
}

/** Deletes one registered BLE source and verifies transfer acceptance. */
export async function deleteBond(
  device: ConfigHidDevice,
  slot: number,
): Promise<TransferBlock> {
  return writeConfig(device, buildDeletePayload(slot), TARGET_BOND_MANAGEMENT);
}

/** Sends one source keyboard report for the firmware to convert and emit. */
export async function injectSourceReport(
  device: ConfigHidDevice,
  modifiers: number,
  usages: readonly number[],
): Promise<void> {
  if (!device.opened) {
    await device.open();
  }
  const payload = buildInjectionPayload(buildSourceReport(modifiers, usages));
  await device.sendFeatureReport(CONFIG_REPORT_ID, payload);
}
