/**
 * Owns the one connection to the physical bridge.
 *
 * The connection used to live inside the on-screen keyboard, which meant the
 * only screen that could see real hardware was the one that injected keys.
 * Everything else showed a simulation, and there was no way to tell from the
 * page which was which. Hoisting it here lets any screen say plainly whether
 * what it is showing came from the board.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import type { KeymapRule } from './keymap';
import type { SecurityBlock, SourceBlock, StatusBlock } from './diagnostics';
import { SimulatedBridge } from './simulatedBridge';
import {
  type ConfigHidDevice,
  readKeymap as readPhysicalKeymap,
  readSourceKeymap as readPhysicalSourceKeymap,
  readSource,
  readSecurity,
  readSources,
  readStatus,
  requestConfigDevice,
  deleteBond as deletePhysicalBond,
  renameBond as renamePhysicalBond,
  setPairingMethod,
  setPairingMode,
  writeKeymap as writePhysicalKeymap,
  writeSourceKeymap as writePhysicalSourceKeymap,
  writeSourceProfile as writePhysicalSourceProfile,
} from './webhid';
import type { Profile } from './profile';

/** What the page knows about the physical bridge right now. */
export interface BridgeHardware {
  /** The open configuration interface, or null while nothing is connected. */
  device: ConfigHidDevice | null;
  /** Latest lifecycle and counters, or null before the first successful read. */
  status: StatusBlock | null;
  /** Latest pairing and report-path state. */
  security: SecurityBlock | null;
  /** Five source slots, read through selector then source-block diagnostics. */
  sources: SourceBlock[];
  /** Active fixed-capacity keymap after an explicit read. */
  keymap: KeymapRule[] | null;
  /** Source-specific keymaps after explicit reads, keyed by registration slot. */
  keymaps: Partial<Record<number, KeymapRule[]>>;
  /** Whether a connection attempt is in flight. */
  connecting: boolean;
  /** Whether a read is in flight. */
  reading: boolean;
  /** The last failure, kept until something succeeds. */
  error: string | null;
  connect(): Promise<void>;
  /** Attaches a bridge that speaks the protocol without a board behind it. */
  connectSimulated(): Promise<void>;
  disconnect(): void;
  refresh(): Promise<void>;
  /** Writes one live source profile and confirms its diagnostics readback. */
  writeSourceProfile(slot: number, profile: Profile): Promise<void>;
  /** Reads the active fixed-capacity keymap from the bridge. */
  readKeymap(): Promise<KeymapRule[]>;
  /** Writes a fixed-capacity keymap and reads it back. */
  writeKeymap(rules: readonly KeymapRule[]): Promise<void>;
  /** Reads one source-specific keymap from target 6. */
  readSourceKeymap(slot: number): Promise<KeymapRule[]>;
  /** Writes one source-specific keymap to target 6 and verifies readback. */
  writeSourceKeymap(slot: number, rules: readonly KeymapRule[]): Promise<void>;
  /** Queues a display-name change for one registered BLE source. */
  renameBond(slot: number, name: string): Promise<void>;
  /** Queues deletion of one registered BLE source. */
  deleteBond(slot: number): Promise<void>;
  /** Whether the attached bridge is the simulation rather than a board. */
  simulated: boolean;
  /** Opens or closes the bridge to keyboards it has not bonded with. */
  setPairingOpen(open: boolean): Promise<void>;
  /** Chooses passkey entry over Just Works for the next pairing. */
  setRequirePasskey(required: boolean): Promise<void>;
}

/** Interval between reads while a screen is watching live values. */
const LIVE_INTERVAL_MS = 1000;
const SOURCE_PROFILE_READBACK_ATTEMPTS = 20;
const SOURCE_PROFILE_READBACK_DELAY_MS = 25;
const BOND_READBACK_ATTEMPTS = 20;
const BOND_READBACK_DELAY_MS = 25;
const KEYMAP_READBACK_ATTEMPTS = 20;
const KEYMAP_READBACK_DELAY_MS = 25;

function message(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function profileFlags(profile: Profile): number {
  return (
    (profile.usToJis ? 1 : 0) |
    (profile.capsToCtrl ? 2 : 0) |
    (profile.swapAltGui ? 4 : 0)
  );
}

function keymapsEqual(left: readonly KeymapRule[], right: readonly KeymapRule[]): boolean {
  return left.length === right.length && left.every((rule, index) => {
    const other = right[index];
    return rule.inputUsage === other.inputUsage &&
      rule.inputShifted === other.inputShifted &&
      rule.outputUsage === other.outputUsage &&
      rule.outputShifted === other.outputShifted;
  });
}

/**
 * @param live Whether to keep re-reading while connected. Screens that display
 * counters pass true; the rest leave the interface alone, because it is the
 * same one that carries the reset command.
 */
export function useBridgeHardware(live: boolean): BridgeHardware {
  const [device, setDevice] = useState<ConfigHidDevice | null>(null);
  const [status, setStatus] = useState<StatusBlock | null>(null);
  const [security, setSecurity] = useState<SecurityBlock | null>(null);
  const [sources, setSources] = useState<SourceBlock[]>([]);
  const [keymap, setKeymap] = useState<KeymapRule[] | null>(null);
  const [keymaps, setKeymaps] = useState<Partial<Record<number, KeymapRule[]>>>({});
  const [simulated, setSimulated] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const [reading, setReading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Reads are serialized. Two overlapping reads on one report ID interleave a
  // selector with someone else's read, and the wrong block comes back.
  const busy = useRef(false);

  const refresh = useCallback(async () => {
    if (!device || busy.current) {
      return;
    }
    busy.current = true;
    setReading(true);
    try {
      // The status block first: it is what the firmware returns by default, so
      // reading it needs no selector and cannot disturb the other reader.
      setStatus(await readStatus(device));
      setSecurity(await readSecurity(device));
      if (typeof readSources === 'function') {
        setSources(await readSources(device));
      }
      setError(null);
    } catch (cause) {
      setError(message(cause));
    } finally {
      busy.current = false;
      setReading(false);
    }
  }, [device]);

  const connect = useCallback(async () => {
    setConnecting(true);
    setError(null);
    try {
      setKeymap(null);
      setKeymaps({});
      setDevice(await requestConfigDevice());
      setSimulated(false);
    } catch (cause) {
      setError(message(cause));
    } finally {
      setConnecting(false);
    }
  }, []);

  const connectSimulated = useCallback(async () => {
    setError(null);
    const bridge = new SimulatedBridge();
    await bridge.open();
    setKeymap(null);
    setKeymaps({});
    setDevice(bridge);
    setSimulated(true);
  }, []);

  // The window is the security boundary, so the result of asking has to be
  // read back rather than assumed: the caller learns what the bridge now says,
  // not what it was told.
  const command = useCallback(
    async (send: (device: ConfigHidDevice) => Promise<void>) => {
      if (!device) {
        setError('先にブリッジへ接続してください。');
        return;
      }
      try {
        await send(device);
        setError(null);
      } catch (cause) {
        setError(message(cause));
      }
      await refresh();
    },
    [device, refresh],
  );

  const setPairingOpen = useCallback(
    (open: boolean) => command((target) => setPairingMode(target, open)),
    [command],
  );

  const setRequirePasskey = useCallback(
    (required: boolean) => command((target) => setPairingMethod(target, required)),
    [command],
  );

  const writeSourceProfile = useCallback(async (slot: number, profile: Profile) => {
    if (!device) {
      const cause = new Error('実機ブリッジへ接続してください');
      setError(cause.message);
      throw cause;
    }
    if (!Number.isInteger(slot) || slot < 0 || slot > 4) {
      const cause = new Error(`source slot ${slot} は0から4の範囲ではありません`);
      setError(cause.message);
      throw cause;
    }
    if (busy.current) {
      const cause = new Error('別のブリッジ操作が完了するまで待ってください。');
      setError(cause.message);
      throw cause;
    }
    busy.current = true;
    setReading(true);
    try {
      const transfer = await writePhysicalSourceProfile(device, slot, profile);
      if (transfer.state !== 2 || transfer.lastError !== 0) {
        throw new Error(`source profileの転送を確認できませんでした: ${transfer.summary}`);
      }
      const expectedFlags = profileFlags(profile);
      for (let attempt = 0; attempt < SOURCE_PROFILE_READBACK_ATTEMPTS; attempt += 1) {
        const source = await readSource(device, slot);
        if (source.profileFlags === expectedFlags) {
          setSources((current) => current.map((item) => item.slot === slot ? source : item));
          setError(null);
          return;
        }
        await new Promise<void>((resolve) => setTimeout(resolve, SOURCE_PROFILE_READBACK_DELAY_MS));
      }
      throw new Error(`source slot ${slot} のプロファイル反映を確認できませんでした`);
    } catch (cause) {
      throw cause;
    } finally {
      busy.current = false;
      setReading(false);
    }
  }, [device]);

  const renameBond = useCallback(async (slot: number, name: string) => {
    if (!device) {
      const cause = new Error('実機ブリッジへ接続してください');
      setError(cause.message);
      throw cause;
    }
    if (busy.current) {
      const cause = new Error('別のブリッジ操作が完了するまで待ってください。');
      setError(cause.message);
      throw cause;
    }
    busy.current = true;
    setReading(true);
    try {
      const transfer = await renamePhysicalBond(device, slot, name);
      if (transfer.state !== 2 || transfer.lastError !== 0) {
        throw new Error(`ボンド名の転送を確認できませんでした: ${transfer.summary}`);
      }
      for (let attempt = 0; attempt < BOND_READBACK_ATTEMPTS; attempt += 1) {
        const source = await readSource(device, slot);
        if (source.transportLabel === 'BLE' && source.name === name) {
          setSources((current) => current.map((item) => item.slot === slot ? source : item));
          setError(null);
          return;
        }
        await new Promise<void>((resolve) => setTimeout(resolve, BOND_READBACK_DELAY_MS));
      }
      // Target 4 is deliberately asynchronous: the radio task applies the
      // request only after the link becomes quiet. A timeout here is therefore
      // not a rejected transfer; a later refresh will show the applied name.
      setError(null);
    } catch (cause) {
      setError(message(cause));
      throw cause;
    } finally {
      busy.current = false;
      setReading(false);
    }
  }, [device]);

  const deleteBond = useCallback(async (slot: number) => {
    if (!device) {
      const cause = new Error('実機ブリッジへ接続してください');
      setError(cause.message);
      throw cause;
    }
    if (busy.current) {
      const cause = new Error('別のブリッジ操作が完了するまで待ってください。');
      setError(cause.message);
      throw cause;
    }
    busy.current = true;
    setReading(true);
    try {
      const transfer = await deletePhysicalBond(device, slot);
      if (transfer.state !== 2 || transfer.lastError !== 0) {
        throw new Error(`ボンド削除の転送を確認できませんでした: ${transfer.summary}`);
      }
      for (let attempt = 0; attempt < BOND_READBACK_ATTEMPTS; attempt += 1) {
        const source = await readSource(device, slot);
        if (source.transportLabel === 'Unregistered' && source.state === 0) {
          setSources((current) => current.map((item) => item.slot === slot ? source : item));
          setError(null);
          return;
        }
        await new Promise<void>((resolve) => setTimeout(resolve, BOND_READBACK_DELAY_MS));
      }
      // Deletion has the same quiet-boundary semantics as rename. The transfer
      // was accepted even when the connected source still appears briefly.
      setError(null);
    } catch (cause) {
      setError(message(cause));
      throw cause;
    } finally {
      busy.current = false;
      setReading(false);
    }
  }, [device]);

  const readKeymap = useCallback(async (): Promise<KeymapRule[]> => {
    if (!device) {
      const cause = new Error('実機ブリッジへ接続してください');
      setError(cause.message);
      throw cause;
    }
    if (busy.current) {
      const cause = new Error('別のブリッジ操作が完了するまで待ってください。');
      setError(cause.message);
      throw cause;
    }
    busy.current = true;
    setReading(true);
    try {
      const rules = await readPhysicalKeymap(device);
      setKeymap(rules);
      setError(null);
      return rules;
    } catch (cause) {
      setError(message(cause));
      throw cause;
    } finally {
      busy.current = false;
      setReading(false);
    }
  }, [device]);

  const writeKeymap = useCallback(async (rules: readonly KeymapRule[]): Promise<void> => {
    if (!device) {
      const cause = new Error('実機ブリッジへ接続してください');
      setError(cause.message);
      throw cause;
    }
    if (busy.current) {
      const cause = new Error('別のブリッジ操作が完了するまで待ってください。');
      setError(cause.message);
      throw cause;
    }
    busy.current = true;
    setReading(true);
    try {
      const transfer = await writePhysicalKeymap(device, rules);
      if (transfer.state !== 2 || transfer.lastError !== 0) {
        throw new Error(`キーマップの転送を確認できませんでした: ${transfer.summary}`);
      }
      for (let attempt = 0; attempt < KEYMAP_READBACK_ATTEMPTS; attempt += 1) {
        const actual = await readPhysicalKeymap(device);
        if (keymapsEqual(actual, rules)) {
          setKeymap(actual);
          setError(null);
          return;
        }
        await new Promise<void>((resolve) => setTimeout(resolve, KEYMAP_READBACK_DELAY_MS));
      }
      throw new Error('キーマップの適用を読み戻しで確認できませんでした');
    } catch (cause) {
      setError(message(cause));
      throw cause;
    } finally {
      busy.current = false;
      setReading(false);
    }
  }, [device]);


  const readSourceKeymap = useCallback(async (slot: number): Promise<KeymapRule[]> => {
    if (!device) {
      throw new Error('bridge is not connected');
    }
    if (busy.current) {
      throw new Error('bridge operation is already in progress');
    }
    busy.current = true;
    setReading(true);
    try {
      const rules = await readPhysicalSourceKeymap(device, slot);
      setKeymaps((current) => ({ ...current, [slot]: rules }));
      if (slot === 0) {
        setKeymap(rules);
      }
      setError(null);
      return rules;
    } catch (cause) {
      setError(message(cause));
      throw cause;
    } finally {
      busy.current = false;
      setReading(false);
    }
  }, [device]);

  const writeSourceKeymap = useCallback(async (
    slot: number,
    rules: readonly KeymapRule[],
  ): Promise<void> => {
    if (!device) {
      throw new Error('bridge is not connected');
    }
    if (busy.current) {
      throw new Error('bridge operation is already in progress');
    }
    busy.current = true;
    setReading(true);
    try {
      const transfer = await writePhysicalSourceKeymap(device, slot, rules);
      if (transfer.state !== 2 || transfer.lastError !== 0) {
        throw new Error(`source keymap transfer was not accepted: ${transfer.summary}`);
      }
      for (let attempt = 0; attempt < KEYMAP_READBACK_ATTEMPTS; attempt += 1) {
        const actual = await readPhysicalSourceKeymap(device, slot);
        if (keymapsEqual(actual, rules)) {
          setKeymaps((current) => ({ ...current, [slot]: actual }));
          if (slot === 0) {
            setKeymap(actual);
          }
          setError(null);
          return;
        }
        await new Promise<void>((resolve) => setTimeout(resolve, KEYMAP_READBACK_DELAY_MS));
      }
      throw new Error('source keymap readback did not converge');
    } catch (cause) {
      setError(message(cause));
      throw cause;
    } finally {
      busy.current = false;
      setReading(false);
    }
  }, [device]);

  const disconnect = useCallback(() => {
    setDevice(null);
    setStatus(null);
    setSecurity(null);
    setSources([]);
    setKeymap(null);
    setKeymaps({});
    setSimulated(false);
  }, []);

  useEffect(() => {
    if (!device) {
      return undefined;
    }
    void refresh();
    if (!live) {
      return undefined;
    }
    const timer = setInterval(() => void refresh(), LIVE_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [device, live, refresh]);

  return {
    device,
    status,
    security,
    sources,
    keymap,
    keymaps,
    connecting,
    reading,
    error,
    connect,
    connectSimulated,
    disconnect,
    refresh,
    writeSourceProfile,
    readKeymap,
    writeKeymap,
    readSourceKeymap,
    writeSourceKeymap,
    renameBond,
    deleteBond,
    simulated,
    setPairingOpen,
    setRequirePasskey,
  };
}
