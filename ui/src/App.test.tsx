import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from './App';
import type { BridgeDevice, BridgeSnapshot } from './device';
import { MemoryBridgeDevice, SerialDeviceTransport } from './device';
import type { KeymapRule } from './keymap';
import { SOURCE_KIND, describeBondFlags, describeError, describeStuckKey } from './diagnostics';

/** Whether the next connection attempt is refused, as a cancelled picker is. */
let connectFails = false;
let writeFails = false;
let writeAppliesThenFails = false;
let keymapWriteFails = false;
let staleKeymapReads = 0;
let pendingKeymapDiagnostics: KeymapRule[] | null = null;
const sourceWrites: Array<{ slot: number; profile: { usToJis: boolean; capsToCtrl: boolean; swapAltGui: boolean } }> = [];
const bondWrites: Array<{ operation: string; slot: number; name?: string }> = [];
const keymapWrites: KeymapRule[][] = [];
let keymapDiagnostics: KeymapRule[] = [];
let sourceDiagnostics = [
  { kind: SOURCE_KIND, slot: 0, transport: 2, transportLabel: 'BLE', state: 5, stateLabel: '接続済み', identityAddress: 'db:e9:25:13:55:25', irkPresent: true, profileFlags: 0b001, name: 'Physical BLE' },
  { kind: SOURCE_KIND, slot: 1, transport: 0, transportLabel: 'Unregistered', state: 0, stateLabel: 'Unregistered', identityAddress: null, irkPresent: false, profileFlags: 0, name: '' },
  { kind: SOURCE_KIND, slot: 2, transport: 0, transportLabel: 'Unregistered', state: 0, stateLabel: 'Unregistered', identityAddress: null, irkPresent: false, profileFlags: 0, name: '' },
  { kind: SOURCE_KIND, slot: 3, transport: 0, transportLabel: 'Unregistered', state: 0, stateLabel: 'Unregistered', identityAddress: null, irkPresent: false, profileFlags: 0, name: '' },
  { kind: SOURCE_KIND, slot: 4, transport: 3, transportLabel: 'Virtual', state: 5, stateLabel: '接続済み', identityAddress: null, irkPresent: false, profileFlags: 0b001, name: 'Physical Virtual' },
];

// The transport is replaced rather than the browser API, because WebHID cannot
// be exercised in a test environment at all: the device picker is native, and
// the one physical board is not something a test may open.
vi.mock('./webhid', () => ({
  requestConfigDevice: async () => {
    if (connectFails) {
      throw new Error('デバイスが選択されませんでした。');
    }
    return { opened: true, productName: 'XIAO BLE coexistence probe', collections: [] };
  },
  readStatus: async () => ({
    state: 5,
    stateLabel: '購読済み',
    lastError: 0,
    lastErrorLabel: describeError(0),
    advertisementsSeen: 8550,
    hidAdvertisements: 1,
    lastAddress: 'db:e9:25:13:55:25',
    lastRssi: -58,
    connectionCount: 1,
    inputReportsReceived: 131,
    reportsForwarded: 131,
    panicLine: 0,
    panicCount: 0,
    discoveryStep: 9,
    discoveryStepLabel: '9 通知の購読',
  }),
  readSecurity: async () => ({
    disconnectReason: 0,
    pairingFailure: 0,
    passkey: 0,
    passkeySerial: 0,
    bondFlags: 0x05,
    bondSummary: describeBondFlags(0x05),
    hogpRefusal: 0,
    notifyRefusal: 0,
    notifyHandle: 24,
    notifyExpected: 24,
    bondFlashError: 0,
    bondFlashErrno: 0,
    usbHogpRefusal: 0,
    linkSetupFailed: false,
    stuckKeyReleases: 2,
    livenessProbeFailures: 0,
    livenessProbeError: 0,
    stuckKeySummary: describeStuckKey(2, 0, 0),
    injectRefusal: 0,
    injectRefusalLabel: 'なし',
  }),
  readSources: async () => structuredClone(sourceDiagnostics),
  readSource: async (_device: unknown, slot: number) => structuredClone(sourceDiagnostics.find((source) => source.slot === slot)),
  readKeymap: async () => {
    if (staleKeymapReads > 0) {
      staleKeymapReads -= 1;
      return [];
    }
    if (pendingKeymapDiagnostics !== null) {
      keymapDiagnostics = pendingKeymapDiagnostics;
      pendingKeymapDiagnostics = null;
    }
    return structuredClone(keymapDiagnostics);
  },
  writeKeymap: async (_device: unknown, rules: readonly KeymapRule[]) => {
    if (keymapWriteFails) {
      throw new Error('keymap write failed');
    }
    const next = rules.map((rule) => ({ ...rule }));
    keymapWrites.push(next);
    staleKeymapReads = 1;
    pendingKeymapDiagnostics = next;
    return { state: 2, lastError: 0, summary: 'ok' };
  },
  writeSourceProfile: async (_device: unknown, slot: number, profile: { usToJis: boolean; capsToCtrl: boolean; swapAltGui: boolean }) => {
    if (writeFails) {
      throw new Error('write failed');
    }
    sourceWrites.push({ slot, profile });
    const flags = (profile.usToJis ? 1 : 0) | (profile.capsToCtrl ? 2 : 0) | (profile.swapAltGui ? 4 : 0);
    sourceDiagnostics = sourceDiagnostics.map((source) => source.slot === slot ? { ...source, profileFlags: flags } : source);
    if (writeAppliesThenFails) {
      writeAppliesThenFails = false;
      throw new Error('readback timed out after apply');
    }
    return { state: 2, lastError: 0, summary: '完了' };
  },
  renameBond: async (_device: unknown, slot: number, name: string) => {
    bondWrites.push({ operation: 'rename', slot, name });
    sourceDiagnostics = sourceDiagnostics.map((source) => source.slot === slot ? { ...source, name } : source);
    return { state: 2, lastError: 0, summary: '完了' };
  },
  deleteBond: async (_device: unknown, slot: number) => {
    bondWrites.push({ operation: 'delete', slot });
    sourceDiagnostics = sourceDiagnostics.map((source) => source.slot === slot
      ? { ...source, transport: 0, transportLabel: 'Unregistered', state: 0, stateLabel: 'Unregistered', identityAddress: null, irkPresent: false, profileFlags: 0, name: '' }
      : source);
    return { state: 2, lastError: 0, summary: '完了' };
  },
  injectSourceReport: async () => undefined,
  setPairingMode: async (_device: unknown, open: boolean) => {
    pairingCalls.push(['mode', open]);
  },
  setPairingMethod: async (_device: unknown, required: boolean) => {
    pairingCalls.push(['method', required]);
  },
}));

/** What the screen asked the bridge to do, in order. */
const pairingCalls: Array<[string, boolean]> = [];

beforeEach(() => {
  connectFails = false;
  writeFails = false;
  writeAppliesThenFails = false;
  keymapWriteFails = false;
  pairingCalls.length = 0;
  sourceWrites.length = 0;
  bondWrites.length = 0;
  keymapWrites.length = 0;
  keymapDiagnostics = [];
  staleKeymapReads = 0;
  pendingKeymapDiagnostics = null;
  sourceDiagnostics = [
    { kind: SOURCE_KIND, slot: 0, transport: 2, transportLabel: 'BLE', state: 5, stateLabel: '接続済み', identityAddress: 'db:e9:25:13:55:25', irkPresent: true, profileFlags: 0b001, name: 'Physical BLE' },
    { kind: SOURCE_KIND, slot: 1, transport: 0, transportLabel: 'Unregistered', state: 0, stateLabel: 'Unregistered', identityAddress: null, irkPresent: false, profileFlags: 0, name: '' },
    { kind: SOURCE_KIND, slot: 2, transport: 0, transportLabel: 'Unregistered', state: 0, stateLabel: 'Unregistered', identityAddress: null, irkPresent: false, profileFlags: 0, name: '' },
    { kind: SOURCE_KIND, slot: 3, transport: 0, transportLabel: 'Unregistered', state: 0, stateLabel: 'Unregistered', identityAddress: null, irkPresent: false, profileFlags: 0, name: '' },
    { kind: SOURCE_KIND, slot: 4, transport: 3, transportLabel: 'Virtual', state: 5, stateLabel: '接続済み', identityAddress: null, irkPresent: false, profileFlags: 0b001, name: 'Physical Virtual' },
  ];
});

const initialSnapshot: BridgeSnapshot = {
  outputReady: true,
  hidOutput: [0, 0, 0, 0, 0, 0, 0, 0],
  eventLog: [],
  sources: [
    { id: 'usb-0', name: 'USB Keyboard', transport: 'USB', connected: true, usToJis: true, capsToCtrl: true, swapAltGui: true },
    { id: 'ble-1', name: 'Bluetooth Keyboard', transport: 'Bluetooth', connected: true, usToJis: true, capsToCtrl: false, swapAltGui: false },
  ],
};

describe('bridge configuration', () => {
  it('opens on a management dashboard with source and action summaries', async () => {
    render(<App device={new SerialDeviceTransport(new MemoryBridgeDevice())} />);

    expect(await screen.findByRole('heading', { name: 'ブリッジ管理' })).toBeInTheDocument();
    expect(screen.getByRole('region', { name: '接続状況' })).toHaveTextContent('接続中のソース2');
    expect(screen.getByRole('region', { name: '登録状況' })).toHaveTextContent('登録済み1 / 4');
    expect(screen.getByRole('button', { name: 'Bluetooth Keyboard の設定' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'キーボードを登録' })).toBeInTheDocument();
  });

  it('opens a source settings screen from its dashboard card', async () => {
    render(<App device={new SerialDeviceTransport(new MemoryBridgeDevice())} />);

    fireEvent.click(await screen.findByRole('button', { name: 'Bluetooth Keyboard の設定' }));

    expect(await screen.findByRole('heading', { name: '互換プリセット' })).toBeInTheDocument();
  });

  it('opens keyboard registration from the management quick actions', async () => {
    render(<App device={new SerialDeviceTransport(new MemoryBridgeDevice())} />);

    fireEvent.click(await screen.findByRole('button', { name: 'キーボードを登録' }));

    expect(await screen.findByText(/SIMULATED/)).toBeInTheDocument();
  });

  it('announces the initial device load accessibly', () => {
    const device: BridgeDevice = {
      readSnapshot: () => new Promise<BridgeSnapshot>(() => undefined),
      saveProfile: async () => undefined,
      runDemoScenario: async () => initialSnapshot,
    };

    render(<App device={device} />);

    expect(screen.getByRole('status')).toHaveTextContent('デバイスを読み込んでいます…');
  });

  it('shows a readable load error and can retry without knowing the transport', async () => {
    let reads = 0;
    const device: BridgeDevice = {
      readSnapshot: async () => {
        reads += 1;
        if (reads === 1) {
          throw new Error('temporary connection failure');
        }
        return structuredClone(initialSnapshot);
      },
      saveProfile: async () => undefined,
      runDemoScenario: async () => structuredClone(initialSnapshot),
    };

    render(<App device={device} />);

    expect(await screen.findByRole('alert')).toHaveTextContent('デバイスの状態を読み込めませんでした。');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '再試行' }));
    expect(await screen.findByText('USB Keyboard')).toBeInTheDocument();
  });

  it('shows the BLE and virtual sources with their fixed slots', async () => {
    render(<App device={new SerialDeviceTransport(new MemoryBridgeDevice())} />);
    expect(await screen.findByText('Bluetooth Keyboard')).toBeInTheDocument();
    expect(screen.getByText('Virtual Input')).toBeInTheDocument();
    expect(screen.getByText(/slot 0/)).toBeInTheDocument();
    expect(screen.getByText(/slot 4/)).toBeInTheDocument();
  });

  it('persists an editable compatibility preset per keyboard', async () => {
    const device = new SerialDeviceTransport(new MemoryBridgeDevice());
    render(<App device={device} />);
    fireEvent.click(await screen.findByRole('button', { name: 'キーマップ' }));
    const checkbox = screen.getAllByLabelText('Caps Lock → Control')[1];
    expect(checkbox).not.toBeChecked();
    fireEvent.click(checkbox);
    await waitFor(async () => expect((await device.readSnapshot()).sources[1].capsToCtrl).toBe(true));
  });

  it('restores the displayed preset when saving it fails', async () => {
    const device: BridgeDevice = {
      readSnapshot: async () => structuredClone(initialSnapshot),
      saveProfile: async () => {
        throw new Error('device unavailable');
      },
      runDemoScenario: async () => structuredClone(initialSnapshot),
    };
    render(<App device={device} />);

    fireEvent.click(await screen.findByRole('button', { name: 'キーマップ' }));
    const checkbox = screen.getAllByLabelText('Caps Lock → Control')[1];
    fireEvent.click(checkbox);

    expect(await screen.findByRole('alert')).toHaveTextContent('保存に失敗しました。接続を確認して再試行してください。');
    expect(checkbox).not.toBeChecked();
  });

  it('simulates the USB US-at conversion and records its HID output', async () => {
    render(<App device={new SerialDeviceTransport(new MemoryBridgeDevice())} />);

    fireEvent.click(await screen.findByRole('button', { name: 'デモ' }));
    expect(screen.getByText(/シミュレーションです/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'USB: Shift + 2 (@)' }));

    fireEvent.click(screen.getByRole('button', { name: '診断' }));
    expect(await screen.findByText('00 00 2F 00 00 00 00 00')).toBeInTheDocument();
    expect(screen.getByText(/USB.*JIS @.*0x2F/)).toBeInTheDocument();
  });

  it('simulates merging USB A and Bluetooth B', async () => {
    render(<App device={new SerialDeviceTransport(new MemoryBridgeDevice())} />);

    fireEvent.click(await screen.findByRole('button', { name: 'デモ' }));
    fireEvent.click(screen.getByRole('button', { name: 'USB A + BLE B' }));
    fireEvent.click(screen.getByRole('button', { name: '診断' }));

    expect(await screen.findByText('00 00 04 05 00 00 00 00')).toBeInTheDocument();
    expect(screen.getByText(/USB A.*BLE B.*統合/)).toBeInTheDocument();
  });

  it('announces an in-flight simulation and prevents concurrent scenario actions', async () => {
    let finishScenario: ((snapshot: BridgeSnapshot) => void) | undefined;
    const device: BridgeDevice = {
      readSnapshot: async () => structuredClone(initialSnapshot),
      saveProfile: async () => undefined,
      runDemoScenario: () => new Promise<BridgeSnapshot>((resolve) => {
        finishScenario = resolve;
      }),
    };
    render(<App device={device} />);

    fireEvent.click(await screen.findByRole('button', { name: 'デモ' }));
    const scenario = screen.getByRole('button', { name: 'USB: Shift + 2 (@)' });
    fireEvent.click(scenario);

    expect(screen.getByRole('status')).toHaveTextContent('シミュレーションを実行しています…');
    expect(scenario).toBeDisabled();

    finishScenario?.(structuredClone(initialSnapshot));
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
  });
});

describe('physical bridge', () => {
  const simulator: BridgeDevice = {
    readSnapshot: async () => structuredClone(initialSnapshot),
    saveProfile: async () => undefined,
    runDemoScenario: async () => structuredClone(initialSnapshot),
  };

  it('says the diagnostics are simulated until a board is connected', async () => {
    render(<App device={simulator} />);

    fireEvent.click(await screen.findByRole('button', { name: '診断' }));

    expect(screen.getByText(/SIMULATED OUTPUT/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '実機に接続' })).toBeInTheDocument();
  });

  it('uses live source diagnostics on the management dashboard after physical connection', async () => {
    render(<App device={simulator} />);

    fireEvent.click(await screen.findByRole('button', { name: '実機に接続' }));

    expect(await screen.findByRole('heading', { name: 'ブリッジ管理' })).toBeInTheDocument();
    expect(await screen.findByText('Physical BLE')).toBeInTheDocument();
    expect(screen.getByText('Physical Virtual')).toBeInTheDocument();
    expect(screen.queryByText('Bluetooth Keyboard')).not.toBeInTheDocument();
    expect(screen.getByText('実機')).toBeInTheDocument();
    expect(screen.getByRole('region', { name: '登録状況' })).toHaveTextContent('登録済み1 / 4');
  });

  it('keeps a disconnected registered source out of the connected count and badge', async () => {
    sourceDiagnostics = sourceDiagnostics.map((source) => source.slot === 0
      ? { ...source, state: 1, stateLabel: 'Disconnected' }
      : source);
    render(<App device={simulator} />);

    fireEvent.click(await screen.findByRole('button', { name: '実機に接続' }));

    expect(await screen.findByText('Physical BLE')).toBeInTheDocument();
    expect(await screen.findByRole('region', { name: '接続状況' })).toHaveTextContent('接続中のソース1');
    const sourceCard = screen.getByText('Physical BLE').closest('article');
    expect(sourceCard).not.toBeNull();
    expect(sourceCard?.querySelector('.source-state')).toHaveTextContent('未接続');
  });

  it('shows the board’s own values once it is connected', async () => {
    // The whole point of the connection is that a number on this page can be
    // trusted. Showing simulated counters beside a connected board, or real
    // ones without saying so, is the failure this replaces.
    render(<App device={simulator} />);

    fireEvent.click(await screen.findByRole('button', { name: '診断' }));
    fireEvent.click(screen.getByRole('button', { name: '実機に接続' }));

    expect(await screen.findByText('購読済み')).toBeInTheDocument();
    expect(screen.getByText(/実機の値/)).toBeInTheDocument();
    expect(screen.queryByText(/SIMULATED OUTPUT/)).not.toBeInTheDocument();
    expect(screen.getByText('全解放2回 / 生存確認の無応答0回 / 直近の失敗理由 なし')).toBeInTheDocument();
  });

  it('reports a refused connection instead of pretending to be connected', async () => {
    connectFails = true;
    render(<App device={simulator} />);

    fireEvent.click(await screen.findByRole('button', { name: '診断' }));
    fireEvent.click(screen.getByRole('button', { name: '実機に接続' }));

    expect(await screen.findByText('デバイスが選択されませんでした。')).toBeInTheDocument();
    expect(screen.getByText(/SIMULATED OUTPUT/)).toBeInTheDocument();
  });

  it('uses physical source diagnostics for keymap and writes only registered BLE/virtual slots', async () => {
    const saveProfile = vi.fn(async () => undefined);
    render(<App device={{ ...simulator, saveProfile }} />);

    fireEvent.click(await screen.findByRole('button', { name: '実機に接続' }));
    fireEvent.click(await screen.findByRole('button', { name: 'キーマップ' }));

    expect(await screen.findByText('Physical BLE')).toBeInTheDocument();
    expect(screen.getByText('Physical Virtual')).toBeInTheDocument();
    expect(screen.queryByText(/slot 1/)).not.toBeInTheDocument();
    fireEvent.click(screen.getAllByLabelText('Caps Lock → Control')[0]);

    await waitFor(() => expect(sourceWrites).toContainEqual({ slot: 0, profile: { usToJis: true, capsToCtrl: true, swapAltGui: false } }));
    expect(saveProfile).not.toHaveBeenCalled();
    expect(await screen.findByText('profile 0b011')).toBeInTheDocument();
  });

  it('reads, edits, and saves the physical keymap through the editor', async () => {
    render(<App device={simulator} />);

    fireEvent.click(await screen.findByRole('button', { name: '実機に接続' }));
    fireEvent.click(await screen.findByRole('button', { name: 'キーマップ' }));
    fireEvent.click(await screen.findByRole('button', { name: 'キーマップを読み込む' }));

    fireEvent.click(await screen.findByRole('button', { name: 'ルールを追加' }));
    fireEvent.change(screen.getByRole('spinbutton', { name: '入力usage 1' }), { target: { value: '6' } });
    fireEvent.click(screen.getByRole('button', { name: 'キーマップを保存' }));

    await waitFor(() => expect(keymapWrites).toContainEqual([
      { inputUsage: 6, inputShifted: false, outputUsage: 4, outputShifted: false },
    ]));
    await waitFor(() => expect(keymapDiagnostics).toEqual([
      { inputUsage: 6, inputShifted: false, outputUsage: 4, outputShifted: false },
    ]));
    expect(await screen.findByRole('spinbutton', { name: '入力usage 1' })).toHaveValue(6);
  });

  it('shows the keymap load state, destination, and only enables save after a change', async () => {
    keymapDiagnostics = [{ inputUsage: 4, inputShifted: false, outputUsage: 5, outputShifted: false }];
    render(<App device={simulator} />);

    fireEvent.click(await screen.findByRole('button', { name: '実機に接続' }));
    fireEvent.click(await screen.findByRole('button', { name: 'キーマップ' }));

    expect(await screen.findByText('キーマップを読み込んでから編集できます。')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'キーマップを読み込む' }));

    expect(await screen.findByText('1/32ルール・保存済み')).toBeInTheDocument();
    expect(screen.getByText('保存先: 実機')).toBeInTheDocument();
    const save = screen.getByRole('button', { name: 'キーマップを保存' });
    expect(save).toBeDisabled();

    fireEvent.change(screen.getByRole('spinbutton', { name: '入力usage 1' }), { target: { value: '6' } });

    expect(await screen.findByText('1/32ルール・未保存の変更')).toBeInTheDocument();
    expect(save).toBeEnabled();
  });

  it('shows validation errors and blocks an invalid keymap save', async () => {
    keymapDiagnostics = [{ inputUsage: 4, inputShifted: false, outputUsage: 5, outputShifted: false }];
    render(<App device={simulator} />);

    fireEvent.click(await screen.findByRole('button', { name: '実機に接続' }));
    fireEvent.click(await screen.findByRole('button', { name: 'キーマップ' }));
    expect(await screen.findByText('キーマップを読み込んでから編集できます。')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'キーマップを読み込む' }));
    expect(await screen.findByText('1/32ルール・保存済み')).toBeInTheDocument();
    fireEvent.click(await screen.findByRole('button', { name: 'ルールを追加' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('入力usage 2 とShiftの組み合わせが重複しています');
    expect(screen.getByRole('button', { name: 'キーマップを保存' })).toBeDisabled();

    fireEvent.change(screen.getByRole('spinbutton', { name: '入力usage 2' }), { target: { value: '6' } });

    await waitFor(() => expect(screen.queryByText('入力usage 2 とShiftの組み合わせが重複しています')).not.toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'キーマップを保存' })).toBeEnabled();
  });

  it('blocks usage values outside the keymap wire contract', async () => {
    keymapDiagnostics = [{ inputUsage: 4, inputShifted: false, outputUsage: 5, outputShifted: false }];
    render(<App device={simulator} />);

    fireEvent.click(await screen.findByRole('button', { name: '実機に接続' }));
    fireEvent.click(await screen.findByRole('button', { name: 'キーマップ' }));
    expect(await screen.findByText('キーマップを読み込んでから編集できます。')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'キーマップを読み込む' }));
    expect(await screen.findByText('1/32ルール・保存済み')).toBeInTheDocument();

    fireEvent.change(screen.getByRole('spinbutton', { name: '入力usage 1' }), { target: { value: '0' } });
    expect(await screen.findByRole('alert')).toHaveTextContent('入力usage 1は1〜255の整数にしてください。');
    expect(screen.getByRole('button', { name: 'キーマップを保存' })).toBeDisabled();

    fireEvent.change(screen.getByRole('spinbutton', { name: '入力usage 1' }), { target: { value: '6' } });
    fireEvent.change(screen.getByRole('spinbutton', { name: '出力usage 1' }), { target: { value: '256' } });
    expect(await screen.findByRole('alert')).toHaveTextContent('出力usage 1は0〜255の整数にしてください。');
    expect(screen.getByRole('button', { name: 'キーマップを保存' })).toBeDisabled();
  });

  it('preserves keymap edits when saving fails', async () => {
    keymapWriteFails = true;
    keymapDiagnostics = [{ inputUsage: 4, inputShifted: false, outputUsage: 5, outputShifted: false }];
    render(<App device={simulator} />);

    fireEvent.click(await screen.findByRole('button', { name: '実機に接続' }));
    fireEvent.click(await screen.findByRole('button', { name: 'キーマップ' }));
    expect(await screen.findByText('キーマップを読み込んでから編集できます。')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'キーマップを読み込む' }));
    expect(await screen.findByText('1/32ルール・保存済み')).toBeInTheDocument();
    fireEvent.change(screen.getByRole('spinbutton', { name: '入力usage 1' }), { target: { value: '6' } });
    fireEvent.click(screen.getByRole('button', { name: 'キーマップを保存' }));

    expect(await screen.findByText('キーマップの保存に失敗しました。接続を確認して再試行してください。')).toBeInTheDocument();
    expect(screen.getByRole('spinbutton', { name: '入力usage 1' })).toHaveValue(6);
    expect(screen.getByText('1/32ルール・未保存の変更')).toBeInTheDocument();
  });

  it('rolls back a physical optimistic update when the write fails', async () => {
    writeFails = true;
    render(<App device={simulator} />);

    fireEvent.click(await screen.findByRole('button', { name: '実機に接続' }));
    fireEvent.click(await screen.findByRole('button', { name: 'キーマップ' }));
    const checkbox = await screen.findAllByLabelText('Caps Lock → Control');
    fireEvent.click(checkbox[0]);

    expect(await screen.findByRole('alert')).toHaveTextContent('保存に失敗しました');
    expect(checkbox[0]).not.toBeChecked();
  });

  it('writes the previous physical profile back when readback fails after apply', async () => {
    writeAppliesThenFails = true;
    render(<App device={simulator} />);

    fireEvent.click(await screen.findByRole('button', { name: '実機に接続' }));
    fireEvent.click(await screen.findByRole('button', { name: 'キーマップ' }));
    const checkbox = (await screen.findAllByLabelText('Caps Lock → Control'))[0];
    fireEvent.click(checkbox);

    expect(await screen.findByRole('alert')).toHaveTextContent('保存に失敗しました');
    await waitFor(() => expect(sourceWrites).toHaveLength(2));
    expect(sourceWrites[0]).toMatchObject({ slot: 0, profile: { usToJis: true, capsToCtrl: true, swapAltGui: false } });
    expect(sourceWrites[1]).toMatchObject({ slot: 0, profile: { usToJis: true, capsToCtrl: false, swapAltGui: false } });
    expect(checkbox).not.toBeChecked();
  });
});

describe('pairing a keyboard', () => {
  const simulator: BridgeDevice = {
    readSnapshot: async () => structuredClone(initialSnapshot),
    saveProfile: async () => undefined,
    runDemoScenario: async () => structuredClone(initialSnapshot),
  };

  async function connected() {
    render(<App device={simulator} />);
    fireEvent.click(await screen.findByRole('button', { name: '実機に接続' }));
    fireEvent.click(await screen.findByRole('button', { name: 'キーボード登録' }));
    return screen.findByRole('heading', { name: 'キーボードの登録' });
  }

  it('asks the bridge to open and close its pairing window', async () => {
    // This is the whole reason the screen exists: `pairing_mode.py` needs
    // Python and uv, which the machine the bridge is carried to will not have.
    await connected();

    fireEvent.click(screen.getByRole('button', { name: '受け入れを開く' }));
    await waitFor(() => expect(pairingCalls).toContainEqual(['mode', true]));

    fireEvent.click(screen.getByRole('button', { name: '閉じる' }));
    await waitFor(() => expect(pairingCalls).toContainEqual(['mode', false]));
  });

  it('switches the pairing method', async () => {
    await connected();

    fireEvent.click(screen.getByRole('button', { name: 'パスキーを要求する' }));

    await waitFor(() => expect(pairingCalls).toContainEqual(['method', true]));
  });

  it('says no passkey has been generated rather than showing a stale zero', async () => {
    // The fixture reports serial zero. A six-digit 000000 would be a number
    // someone might type, and it would waste the thirty seconds the
    // specification allows for the pairing.
    await connected();

    expect(screen.getByText('パスキーはまだ生成されていません。')).toBeInTheDocument();
  });

  it('shows that registrations use independent slots', async () => {
    await connected();

    expect(screen.getByText(/スロットごとに保持され/)).toBeInTheDocument();
    expect(screen.getByText(/空きスロット: 1, 2, 3/)).toBeInTheDocument();
  });

  it('renames and deletes a registered slot through the management controls', async () => {
    await connected();

    fireEvent.change(screen.getByLabelText('slot 0 の表示名'), { target: { value: 'Keyboard B' } });
    fireEvent.click(screen.getByRole('button', { name: '名前を保存' }));
    await waitFor(() => expect(bondWrites).toContainEqual({ operation: 'rename', slot: 0, name: 'Keyboard B' }));

    vi.spyOn(window, 'confirm').mockReturnValue(true);
    fireEvent.click(screen.getByRole('button', { name: '登録を削除' }));
    await waitFor(() => expect(bondWrites).toContainEqual({ operation: 'delete', slot: 0 }));
    expect(screen.getByText('登録済みのBLEキーボードはありません。')).toBeInTheDocument();
  });

  it('shows the simulator list until a bridge is connected', async () => {
    render(<App device={simulator} />);

    fireEvent.click(await screen.findByRole('button', { name: 'キーボード登録' }));

    expect(screen.getByText(/SIMULATED/)).toBeInTheDocument();
  });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});
