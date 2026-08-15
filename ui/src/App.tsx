import { useCallback, useEffect, useRef, useState } from 'react';
import type { SecurityBlock, SourceBlock } from './diagnostics';
import { SOURCE_CONNECTED_STATE } from './diagnostics';
import { KEYMAP_RULE_CAPACITY, type KeymapRule } from './keymap';
import { KeymapEditor } from './KeymapEditor';
import type { BridgeDevice, BridgeSnapshot, DemoScenario, SourceProfile } from './device';
import type { BridgeHardware } from './useBridgeHardware';
import { useBridgeHardware } from './useBridgeHardware';
import { VirtualKeyboard } from './VirtualKeyboard';
import './styles.css';

type Page = 'home' | 'keymap' | 'connections' | 'demo' | 'virtual' | 'diagnostics';

const pages: { id: Page; label: string }[] = [
  { id: 'home', label: 'ホーム' },
  { id: 'keymap', label: 'キーマップ' },
  { id: 'connections', label: 'キーボード登録' },
  { id: 'demo', label: 'デモ' },
  { id: 'virtual', label: '実機テスト' },
  { id: 'diagnostics', label: '診断' },
];

export function App({ device }: { device: BridgeDevice }) {
  const [page, setPage] = useState<Page>('home');
  const [snapshot, setSnapshot] = useState<BridgeSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [showSimulation, setShowSimulation] = useState(false);
  const [running, setRunning] = useState(false);
  const [savingProfile, setSavingProfile] = useState<string | null>(null);
  const [selectedKeymapSlot, setSelectedKeymapSlot] = useState(0);
  const [physicalOverrides, setPhysicalOverrides] = useState<Record<number, SourceProfile>>({});
  const readGeneration = useRef(0);
  // The management dashboard and diagnostics both show live connection state.
  // The configuration interface is the same one that carries the reset command,
  // so other screens leave it quiet while nobody is looking at live values.
  const hardware = useBridgeHardware(page === 'diagnostics' || page === 'home');

  const loadSnapshot = useCallback(async () => {
    const generation = ++readGeneration.current;
    setLoading(true);
    setError(null);
    setSnapshot(null);
    try {
      const nextSnapshot = await device.readSnapshot();
      if (generation === readGeneration.current) {
        setSnapshot(nextSnapshot);
      }
    } catch {
      if (generation === readGeneration.current) {
        setError('デバイスの状態を読み込めませんでした。');
      }
    } finally {
      if (generation === readGeneration.current) {
        setLoading(false);
      }
    }
  }, [device]);

  useEffect(() => {
    void loadSnapshot();
    return () => {
      readGeneration.current += 1;
    };
  }, [loadSnapshot]);

  useEffect(() => {
    // An optimistic physical write belongs to one WebHID session. Do not let
    // it reappear if that board is disconnected and a different one is
    // attached later.
    if (!hardware.device || hardware.simulated) {
      setPhysicalOverrides({});
    }
  }, [hardware.device, hardware.simulated]);

  async function updateProfile(profile: SourceProfile, field: keyof Pick<SourceProfile, 'usToJis' | 'capsToCtrl' | 'swapAltGui'>) {
    if (!snapshot || savingProfile) {
      return;
    }
    const previousSnapshot = snapshot;
    const next = { ...profile, [field]: !profile[field] };
    setError(null);
    setSavingProfile(profile.slot === undefined ? profile.id : `slot-${profile.slot}`);
    const physical = hardware.device && !hardware.simulated &&
      (profile.slot === 4 || profile.transport === 'Bluetooth');
    if (physical) {
      setPhysicalOverrides((current) => ({ ...current, [profile.slot as number]: next }));
    } else {
      setSnapshot({ ...previousSnapshot, sources: previousSnapshot.sources.map((item) => sameSource(item, profile) ? next : item) });
    }
    try {
      if (physical) {
        await hardware.writeSourceProfile(profile.slot as number, {
          usToJis: next.usToJis,
          capsToCtrl: next.capsToCtrl,
          swapAltGui: next.swapAltGui,
        });
        setPhysicalOverrides((current) => {
          const remaining = { ...current };
          delete remaining[profile.slot as number];
          return remaining;
        });
      } else {
        await device.saveProfile(next);
        setSnapshot(await device.readSnapshot());
      }
    } catch {
      if (physical) {
        // A source-profile write is accepted by USB before the radio task
        // applies it. If readback timed out, the original event may still be
        // queued. Put the old profile behind it so the final device state is
        // the state this screen is about to show. If that compensating write
        // also fails, refresh once and show whatever the device reports.
        try {
          await hardware.writeSourceProfile(profile.slot as number, {
            usToJis: profile.usToJis,
            capsToCtrl: profile.capsToCtrl,
            swapAltGui: profile.swapAltGui,
          });
        } catch {
          await hardware.refresh();
        }
        setPhysicalOverrides((current) => {
          const remaining = { ...current };
          delete remaining[profile.slot as number];
          return remaining;
        });
      } else {
        setSnapshot(previousSnapshot);
      }
      setError('保存に失敗しました。接続を確認して再試行してください。');
    } finally {
      setSavingProfile(null);
    }
  }

  async function runScenario(scenario: DemoScenario) {
    setRunning(true);
    setError(null);
    try {
      setSnapshot(await device.runDemoScenario(scenario));
      setShowSimulation(true);
    } catch {
      setError('シミュレーションを実行できませんでした。');
    } finally {
      setRunning(false);
    }
  }

  const busy = running || savingProfile !== null || hardware.reading;
  const physical = hardware.device && !hardware.simulated;
  const keymapSources = hardware.device
    ? hardware.sources
      .filter((source) => source.transportLabel === 'BLE' || source.slot === 4)
      .map(sourceProfileFromDiagnostics)
      .map((profile) => physical
        ? physicalOverrides[profile.slot as number] ?? profile
        : profile)
    : [];
  const profileSources = physical
    ? keymapSources
    : hardware.simulated ? snapshot?.sources ?? [] : [];
  const availableKeymapSlots = keymapSources
    .map((source) => source.slot)
    .filter((slot): slot is number => slot !== undefined);

  useEffect(() => {
    if (availableKeymapSlots.length > 0 && !availableKeymapSlots.includes(selectedKeymapSlot)) {
      setSelectedKeymapSlot(availableKeymapSlots[0]);
    }
  }, [availableKeymapSlots.join(','), selectedKeymapSlot]);

  return <main className="app-shell">
    <header>
      <div><p className="eyebrow">PRIVATE PROTOTYPE</p><h1>Unified Keyboard Firmware</h1></div>
      <HardwareBadge hardware={hardware} />
    </header>
    {hardware.error && <div role="alert" className="error"><p>{hardware.error}</p></div>}
    <nav aria-label="設定画面">{pages.map((item) => <button key={item.id} className={page === item.id ? 'active' : ''} onClick={() => setPage(item.id)}>{item.label}</button>)}</nav>
    {error && <div role="alert" className="error"><p>{error}</p>{!snapshot && !loading && <button type="button" onClick={() => void loadSnapshot()}>再試行</button>}</div>}
    {loading && <p role="status" aria-live="polite">デバイスを読み込んでいます…</p>}
    {!loading && !snapshot && <p>デバイスの状態を取得できませんでした。再試行してください。</p>}
    {snapshot && <section>
      {page === 'home' && <Home snapshot={snapshot} hardware={hardware} onNavigate={setPage} />}
      {page === 'keymap' && <Keymap sources={profileSources} keymapSources={keymapSources} selectedSlot={selectedKeymapSlot} onSelectSlot={setSelectedKeymapSlot} disabled={busy} saving={savingProfile !== null} onToggle={updateProfile} keymap={hardware.keymaps[selectedKeymapSlot] ?? null} keymapAvailable={hardware.device !== null} keymapDestination={hardware.device ? hardware.simulated ? 'シミュレーション' : '実機' : '未接続'} keymapBusy={hardware.reading} onReadKeymap={() => hardware.readSourceKeymap(selectedKeymapSlot)} onWriteKeymap={(rules) => hardware.writeSourceKeymap(selectedKeymapSlot, rules)} />}
      {page === 'connections' && (hardware.device ? <Pairing hardware={hardware} /> : <DisconnectedConnections />)}
      {page === 'demo' && <Demo disabled={busy} running={running} onRun={runScenario} />}
      {page === 'virtual' && <VirtualKeyboard hardware={hardware} />}
      {page === 'diagnostics' && <Diagnostics snapshot={snapshot} hardware={hardware} showSimulation={showSimulation} />}
    </section>}
  </main>;
}

/**
 * States whether the page is looking at the board or at a simulation.
 *
 * Every screen but two shows simulated data, and before this the only way to
 * know was to notice a small grey label. Connection state belongs where it is
 * always visible.
 */
function HardwareBadge({ hardware }: { hardware: BridgeHardware }) {
  if (!hardware.device) {
    return <div className="hardware-badge">
      <span className="status">未接続</span>
      <button type="button" className="primary" disabled={hardware.connecting} onClick={() => void hardware.connect()}>
        {hardware.connecting ? '接続中…' : '実機に接続'}
      </button>
      {/* Same protocol, no board. Useful when there is no hardware to hand,
          and the only way to check this application without a person present
          to answer the browser's device picker. */}
      <button type="button" onClick={() => void hardware.connectSimulated()}>シミュレーションに接続</button>
    </div>;
  }
  return <div className="hardware-badge">
    <span className={hardware.simulated ? 'status simulated' : 'status ready'}>
      {hardware.simulated ? 'シミュレーション' : '実機'} 接続済み
      {hardware.device.productName ? `: ${hardware.device.productName}` : ''}
    </span>
    <button type="button" onClick={hardware.disconnect}>切断</button>
  </div>;
}

function profileFlags(profile: SourceProfile): number {
  return (profile.usToJis ? 1 : 0) | (profile.capsToCtrl ? 2 : 0) | (profile.swapAltGui ? 4 : 0);
}

function sameSource(left: SourceProfile, right: SourceProfile): boolean {
  if (left.slot !== undefined && right.slot !== undefined) {
    return left.slot === right.slot;
  }
  return left.id === right.id;
}

function sourceProfileFromDiagnostics(source: SourceBlock): SourceProfile {
  return {
    id: `source-${source.slot}`,
    slot: source.slot,
    name: source.name || `slot ${source.slot}`,
    transport: source.transportLabel === 'BLE' ? 'Bluetooth' : source.transportLabel === 'Virtual' ? 'Virtual' : 'Unregistered',
    connected: source.state === SOURCE_CONNECTED_STATE,
    state: source.stateLabel,
    identityAddress: source.identityAddress,
    irkPresent: source.irkPresent,
    usToJis: (source.profileFlags & 1) !== 0,
    capsToCtrl: (source.profileFlags & 2) !== 0,
    swapAltGui: (source.profileFlags & 4) !== 0,
  };
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

function keymapValidationMessage(rules: readonly KeymapRule[]): string | null {
  if (rules.length > KEYMAP_RULE_CAPACITY) {
    return `キーマップは${KEYMAP_RULE_CAPACITY}ルールまでです。`;
  }
  for (const [index, rule] of rules.entries()) {
    if (!Number.isInteger(rule.inputUsage) || rule.inputUsage < 1 || rule.inputUsage > 255) {
      return `入力usage ${index + 1}は1〜255の整数にしてください。`;
    }
    if (!Number.isInteger(rule.outputUsage) || rule.outputUsage < 0 || rule.outputUsage > 255) {
      return `出力usage ${index + 1}は0〜255の整数にしてください。`;
    }
    const duplicate = rules.slice(0, index).some((other) =>
      other.inputUsage === rule.inputUsage && other.inputShifted === rule.inputShifted,
    );
    if (duplicate) {
      return `入力usage ${index + 1} とShiftの組み合わせが重複しています。`;
    }
  }
  return null;
}

function Home({ snapshot, hardware, onNavigate }: { snapshot: BridgeSnapshot; hardware: BridgeHardware; onNavigate: (page: Page) => void }) {
  const sources = hardware.device
    ? (hardware.simulated ? snapshot.sources : hardware.sources.map(sourceProfileFromDiagnostics))
      .filter((source) => source.transport !== 'Unregistered')
    : [];
  const registered = sources.filter((source) => source.slot !== undefined && source.slot < 4 && source.transport === 'Bluetooth');
  const connected = sources.filter((source) => source.connected);
  const availableSlots = Math.max(0, 4 - registered.length);
  const sourcesLoading = hardware.device !== null && hardware.reading && hardware.sources.length === 0;
  const outputLabel = hardware.device
    ? hardware.status?.stateLabel ?? '読み込み中'
    : '未接続';
  const sourceContext = hardware.device
    ? hardware.simulated ? 'シミュレーション' : '実機'
    : '未接続';

  return <>
    <div className="page-heading">
      <div>
        <p className="eyebrow">CONTROL CENTER</p>
        <h2>ブリッジ管理</h2>
        <p className="lede">接続中のキーボード、登録スロット、変換設定をここから確認できます。</p>
      </div>
      <span className={hardware.device && !hardware.simulated ? 'context-badge live' : 'context-badge'}>
        {sourceContext}
      </span>
    </div>

    <section className="dashboard-summary" aria-label="概要">
      <SummaryCard label="接続状況" value={sourcesLoading ? '読み込み中' : `接続中のソース${connected.length}`} detail={sourcesLoading ? 'ソース情報を取得中' : `${sources.length}ソースを管理`} />
      <SummaryCard label="登録状況" value={sourcesLoading ? '読み込み中' : `登録済み${registered.length} / 4`} detail={sourcesLoading ? '登録スロットを確認中' : availableSlots > 0 ? `空きスロット ${availableSlots}` : '空きスロットなし'} />
      <SummaryCard label="USB出力" value={outputLabel} detail="変換後の入力をホストへ送信" />
    </section>

    <section className="dashboard-section" aria-labelledby="dashboard-sources-title">
      <div className="section-heading">
        <div>
          <h3 id="dashboard-sources-title">キーボード</h3>
          <p className="section-hint">接続が切れている登録済みキーボードも、設定を保持したまま表示します。</p>
        </div>
        <button type="button" onClick={() => onNavigate('connections')}>登録を管理</button>
      </div>
      {sourcesLoading
        ? <p className="empty-state" role="status" aria-live="polite">実機からソース情報を読み込んでいます…</p>
        : sources.length === 0
        ? <p className="empty-state">キーボードはまだ登録されていません。</p>
        : <div className="dashboard-source-grid">{sources.map((source) => <DashboardSourceCard key={source.id} source={source} onOpen={() => onNavigate('keymap')} />)}</div>}
    </section>

    <section className="dashboard-section" aria-labelledby="dashboard-actions-title">
      <div className="section-heading">
        <div>
          <h3 id="dashboard-actions-title">次にできること</h3>
          <p className="section-hint">よく使う操作へ直接移動できます。</p>
        </div>
      </div>
      <div className="dashboard-actions">
        <ActionCard title="キーマップを編集" detail="キーの入力・出力とShiftを変更" button="キーマップを開く" onClick={() => onNavigate('keymap')} />
        <ActionCard title="キーボードを登録" detail="新しいBLEキーボードを空きスロットへ追加" button="キーボードを登録" onClick={() => onNavigate('connections')} />
        <ActionCard title="状態を確認" detail="接続、打鍵転送、押しっぱなし対策を確認" button="診断を開く" onClick={() => onNavigate('diagnostics')} />
      </div>
    </section>
  </>;
}

function SummaryCard({ label, value, detail }: { label: string; value: string; detail: string }) {
  return <section className="summary-card" aria-label={label}>
    <span className="summary-label">{label}</span>
    <strong>{value}</strong>
    <span className="summary-detail">{detail}</span>
  </section>;
}

function DashboardSourceCard({ source, onOpen }: { source: SourceProfile; onOpen: () => void }) {
  const state = source.connected ? '接続中' : '未接続';
  const profile = [
    source.usToJis ? 'US → JIS' : 'US → JIS off',
    source.capsToCtrl ? 'Caps → Ctrl' : null,
    source.swapAltGui ? 'Alt ⇄ GUI' : null,
  ].filter((item): item is string => item !== null);

  return <article className={source.connected ? 'dashboard-source-card connected' : 'dashboard-source-card'}>
    <div className="source-card-topline">
      <span className="slot-label">slot {source.slot ?? source.id}</span>
      <span className={source.connected ? 'source-state connected' : 'source-state'}>{state}</span>
    </div>
    <h4>{source.name}</h4>
    <p className="source-card-transport">{source.transport}{source.state ? ` · ${source.state}` : ''}</p>
    <p className="source-card-profile">{profile.length > 0 ? profile.join(' · ') : '変換なし'}</p>
    <button type="button" onClick={onOpen}>{source.name} の設定</button>
  </article>;
}

function ActionCard({ title, detail, button, onClick }: { title: string; detail: string; button: string; onClick: () => void }) {
  return <article className="action-card">
    <h4>{title}</h4>
    <p>{detail}</p>
    <button type="button" onClick={onClick}>{button}</button>
  </article>;
}

function Keymap({ sources, keymapSources, selectedSlot, onSelectSlot, disabled, saving, onToggle, keymap, keymapAvailable, keymapDestination, keymapBusy, onReadKeymap, onWriteKeymap }: {
  sources: SourceProfile[];
  keymapSources: SourceProfile[];
  selectedSlot: number;
  onSelectSlot: (slot: number) => void;
  disabled: boolean;
  saving: boolean;
  onToggle: (profile: SourceProfile, field: keyof Pick<SourceProfile, 'usToJis' | 'capsToCtrl' | 'swapAltGui'>) => Promise<void>;
  keymap: KeymapRule[] | null;
  keymapAvailable: boolean;
  keymapDestination: '実機' | 'シミュレーション' | '未接続';
  keymapBusy: boolean;
  onReadKeymap: () => Promise<KeymapRule[]>;
  onWriteKeymap: (rules: readonly KeymapRule[]) => Promise<void>;
}) {
  const [draft, setDraft] = useState<KeymapRule[] | null>(keymap);
  const [saveError, setSaveError] = useState<string | null>(null);
  useEffect(() => {
    setDraft(keymap);
    setSaveError(null);
  }, [keymap]);
  const loaded = keymap !== null && draft !== null;
  const dirty = loaded && !keymapsEqual(draft, keymap);
  const validationError = draft ? keymapValidationMessage(draft) : null;
  const saveDisabled = disabled || keymapBusy || !loaded || !dirty || validationError !== null;

  function updateRule(index: number, field: keyof KeymapRule, value: number | boolean): void {
    setSaveError(null);
    setDraft((current) => current?.map((rule, ruleIndex) => ruleIndex === index ? { ...rule, [field]: value } : rule) ?? null);
  }

  async function saveKeymap(): Promise<void> {
    if (!draft) {
      return;
    }
    setSaveError(null);
    try {
      await onWriteKeymap(draft);
    } catch {
      setSaveError('キーマップの保存に失敗しました。接続を確認して再試行してください。');
    }
  }

  return <>
    <h2>互換プリセット</h2>
    <p className="lede">一般的なUS/JIS変換をキーボードごとに調整し、実機接続時は固定容量キーマップも読み書きできます。</p>
    {saving && <p role="status" aria-live="polite">設定を保存しています…</p>}
    {sources.map((profile) => <article className="profile" key={profile.id} aria-busy={saving}><h3>{profile.name} <small>slot {profile.slot ?? profile.id} · {profile.transport} · {profile.state ?? (profile.connected ? 'Connected' : 'Disconnected')}</small></h3><p>profile 0b{profileFlags(profile).toString(2).padStart(3, '0')}</p><Toggle label="US → JIS" value={profile.usToJis} disabled={disabled} onChange={() => void onToggle(profile, 'usToJis')} /><Toggle label="Caps Lock → Control" value={profile.capsToCtrl} disabled={disabled} onChange={() => void onToggle(profile, 'capsToCtrl')} /><Toggle label="Alt ⇄ GUI" value={profile.swapAltGui} disabled={disabled} onChange={() => void onToggle(profile, 'swapAltGui')} /></article>)}
    <section className="diagnostic-panel" aria-labelledby="keymap-editor-title">
      <h3 id="keymap-editor-title">固定キーマップ</h3>
      <p>ソースごとに現在のルールを読み込み、編集して保存できます。</p>
      <label htmlFor="keymap-source-select">編集対象ソース</label>
       <select
         id="keymap-source-select"
         value={selectedSlot}
         disabled={disabled || keymapBusy || keymapSources.length === 0}
         onChange={(event) => onSelectSlot(Number(event.target.value))}
       >
         {keymapSources.length === 0 && <option value={selectedSlot}>ソース未接続</option>}
         {keymapSources.map((source) => <option key={source.id} value={source.slot ?? selectedSlot}>slot {source.slot ?? source.id} · {source.name}</option>)}
      </select>
      {!keymapAvailable && <p>キーマップの読み書きにはブリッジ接続が必要です。</p>}
      {keymapAvailable && <p className="keymap-destination">保存先: {keymapDestination}</p>}
      {keymapAvailable && <button type="button" disabled={disabled || keymapBusy} onClick={() => void onReadKeymap()}>キーマップを読み込む</button>}
      {keymapBusy && <p role="status" aria-live="polite">キーマップを処理しています…</p>}
      {keymapAvailable && !loaded && !keymapBusy && <p className="keymap-state" role="status">キーマップを読み込んでから編集できます。</p>}
      {draft && <>
        <p className={dirty ? 'keymap-state dirty' : 'keymap-state'} role="status" aria-live="polite">
          {draft.length}/{KEYMAP_RULE_CAPACITY}ルール・{dirty ? '未保存の変更' : '保存済み'}
        </p>
        {validationError && <p className="keymap-validation" role="alert">{validationError}</p>}
        {saveError && <p className="keymap-error" role="alert">{saveError}</p>}
        <KeymapEditor rules={draft} disabled={disabled || keymapBusy} onChange={(rules) => { setSaveError(null); setDraft(rules); }} />
        <table><thead><tr><th>入力</th><th>入力Shift</th><th>出力</th><th>出力Shift</th><th>操作</th></tr></thead><tbody>{draft.map((rule, index) => <tr key={`${index}-${rule.inputUsage}-${rule.inputShifted}`}>
          <td><input aria-label={`入力usage ${index + 1}`} type="number" min="1" max="255" value={rule.inputUsage} onChange={(event) => updateRule(index, 'inputUsage', Number(event.target.value))} /></td>
          <td><input aria-label={`入力Shift ${index + 1}`} type="checkbox" checked={rule.inputShifted} onChange={(event) => updateRule(index, 'inputShifted', event.target.checked)} /></td>
          <td><input aria-label={`出力usage ${index + 1}`} type="number" min="0" max="255" value={rule.outputUsage} onChange={(event) => updateRule(index, 'outputUsage', Number(event.target.value))} /></td>
          <td><input aria-label={`出力Shift ${index + 1}`} type="checkbox" checked={rule.outputShifted} onChange={(event) => updateRule(index, 'outputShifted', event.target.checked)} /></td>
          <td><button type="button" disabled={disabled || keymapBusy} onClick={() => { setSaveError(null); setDraft((current) => current?.filter((_, ruleIndex) => ruleIndex !== index) ?? null); }}>削除</button></td>
        </tr>)}</tbody></table>
        <button type="button" disabled={disabled || keymapBusy || draft.length >= KEYMAP_RULE_CAPACITY} onClick={() => { setSaveError(null); setDraft((current) => [...(current ?? []), { inputUsage: 4, inputShifted: false, outputUsage: 4, outputShifted: false }]); }}>ルールを追加</button>
        <button type="button" className="primary" disabled={saveDisabled} onClick={() => void saveKeymap()}>キーマップを保存</button>
      </>}
    </section>
  </>;
}

function Demo({ disabled, running, onRun }: { disabled: boolean; running: boolean; onRun: (scenario: DemoScenario) => Promise<void> }) {
  return <>
    <h2>ブリッジ動作デモ</h2>
    <p className="simulation-notice"><strong>これはブラウザ内のシミュレーションです。</strong> 物理USB/BLEハードウェアへの接続やキー送信は行いません。</p>
    {running && <p role="status" aria-live="polite">シミュレーションを実行しています…</p>}
    <div className="demo-grid" aria-busy={running}>
      <Scenario title="US → JIS変換" detail="USB入力の Shift + 2 をJISの @ usageへ変換します。" button="USB: Shift + 2 (@)" disabled={disabled} onClick={() => void onRun('usb-us-at')} />
      <Scenario title="複数ソース統合" detail="USBのAとBluetoothのBを一つの8-byteレポートへまとめます。" button="USB A + BLE B" disabled={disabled} onClick={() => void onRun('merge-usb-ble')} />
      <Scenario title="解放・切断" detail="Bluetooth側のキーを解放し、その入力ソースだけを切断します。" button="BLEを解放・切断" disabled={disabled} onClick={() => void onRun('detach-ble')} />
    </div>
    <p className="demo-hint">実行結果は「診断」で確認できます。</p>
  </>;
}

function Scenario({ title, detail, button, disabled, onClick }: { title: string; detail: string; button: string; disabled: boolean; onClick: () => void }) {
  return <article className="scenario"><h3>{title}</h3><p>{detail}</p><button className="primary" disabled={disabled} onClick={onClick}>{button}</button></article>;
}

function Toggle({ label, value, disabled, onChange }: { label: string; value: boolean; disabled: boolean; onChange: () => void }) {
  return <label className="toggle"><input type="checkbox" checked={value} disabled={disabled} onChange={onChange} />{label}</label>;
}

/**
 * Welcomes a keyboard, on a machine that has none of this project's tooling.
 *
 * `pairing_mode.py` needs Python and `uv`. Carrying the bridge to another
 * computer and pairing a keyboard there only works from a page, which is what
 * makes this screen worth having before the rest of device management.
 */
function DisconnectedConnections() {
  return <>
    <h2>キーボード登録</h2>
    <p className="empty-state">ブリッジ未接続です。ヘッダーの「実機に接続」または「シミュレーションに接続」を選ぶと、登録スロットを表示できます。</p>
  </>;
}

function SourceCards({ sources }: { sources: SourceBlock[] }) {
  if (sources.length === 0) {
    return null;
  }
  return <section className="diagnostic-panel" aria-labelledby="source-slots-title">
    <h3 id="source-slots-title">Source slots</h3>
    <div className="cards">{sources.map((source) => <article key={source.slot}>
      <span>slot {source.slot} · {source.transportLabel} · {source.stateLabel}</span>
      <h3>{source.name || `slot ${source.slot}`}</h3>
      <p>profile 0b{source.profileFlags.toString(2).padStart(3, '0')}</p>
      <p>{source.identityAddress ?? 'unregistered identity'} · IRK {source.irkPresent ? 'present' : 'absent'}</p>
    </article>)}</div>
  </section>;
}

function Pairing({ hardware }: { hardware: BridgeHardware }) {
  const { status, security, sources } = hardware;
  if (!status || !security) {
    return <p role="status" aria-live="polite">ブリッジから読み込んでいます…</p>;
  }
  const bonded = (security.bondFlags & 0x01) !== 0 || (security.bondFlags & 0x02) !== 0;
  return <>
    <h2>キーボードの登録</h2>
    <section className="diagnostic-panel">
      <h3>いまの状態</h3>
      <dl>
        <Row label="接続" value={status.stateLabel} />
        <Row label="相手" value={status.connectionCount === 0 ? 'まだ接続していない' : `${status.lastAddress}（${status.lastRssi} dBm）`} />
        <Row label="登録済みの鍵" value={security.bondSummary} />
      </dl>
    </section>

    <section className="diagnostic-panel">
      <h3>新しいキーボードを迎える</h3>
      <p className="lede">
        受け入れ窓は2分で自分から閉じ、キーボードが登録された時点でも閉じます。
        開けたままのブリッジは近くで広告している機器を拾うので、必要なときだけ開けてください。
        {bonded && '登録済みの鍵はスロットごとに保持され、新しいキーボードは空きスロットへ入ります。'}
      </p>
      <div className="pairing-actions">
        <button type="button" className="primary" onClick={() => void hardware.setPairingOpen(true)}>
          受け入れを開く
        </button>
        <button type="button" onClick={() => void hardware.setPairingOpen(false)}>
          閉じる
        </button>
      </div>
    </section>
    <BondSlots hardware={hardware} sources={sources} />

    <section className="diagnostic-panel">
      <h3>登録の方式</h3>
      <p className="lede">
        Just Works は中間者攻撃を防ぎません。登録の瞬間に電波の届く範囲にいる相手が、
        キーボードに成り代われます。パスキーはそれを塞ぎますが、規格が登録の完了を
        30秒に制限しているため、番号は下に出たらすぐキーボードで打つ必要があります。
      </p>
      <div className="pairing-actions">
        <button type="button" onClick={() => void hardware.setRequirePasskey(false)}>Just Works にする</button>
        <button type="button" onClick={() => void hardware.setRequirePasskey(true)}>パスキーを要求する</button>
      </div>
      <Passkey security={security} />
    </section>
  </>;
}

/** Lists the four registration slots and exposes the two 0.6 mutations. */
function BondSlots({ hardware, sources }: { hardware: BridgeHardware; sources: SourceBlock[] }) {
  const [drafts, setDrafts] = useState<Record<number, string>>({});
  const [busySlot, setBusySlot] = useState<number | null>(null);
  const registered = sources.filter((source) => source.slot < 4 && source.transportLabel === 'BLE');
  const emptySlots = sources
    .filter((source) => source.slot < 4 && source.transportLabel === 'Unregistered')
    .map((source) => source.slot);

  async function rename(source: SourceBlock) {
    const name = drafts[source.slot] ?? source.name;
    const bytes = new TextEncoder().encode(name);
    if (bytes.length > 13) {
      return;
    }
    setBusySlot(source.slot);
    try {
      await hardware.renameBond(source.slot, name);
      setDrafts((current) => ({ ...current, [source.slot]: name }));
    } catch {
      // The shared hardware error area contains the transport failure.
    } finally {
      setBusySlot(null);
    }
  }

  async function remove(source: SourceBlock) {
    if (!window.confirm(`slot ${source.slot} の「${source.name}」を削除しますか？`)) {
      return;
    }
    setBusySlot(source.slot);
    try {
      await hardware.deleteBond(source.slot);
    } catch {
      // The shared hardware error area contains the transport failure.
    } finally {
      setBusySlot(null);
    }
  }

  return <section className="diagnostic-panel" aria-labelledby="bond-slots-title">
    <h3 id="bond-slots-title">登録スロット</h3>
    <p className="lede">
      登録は4スロットまでです。名前変更と削除は無線が切れた静かな時点で保存されるため、
      接続中のキーボードは切断後に表示が更新されます。
    </p>
    {registered.length === 0 && <p>登録済みのBLEキーボードはありません。</p>}
    {registered.map((source) => <article className="profile" key={source.slot}>
      <h4>slot {source.slot} · {source.name}</h4>
      <p>{source.identityAddress ?? 'identity unavailable'} · {source.stateLabel}</p>
      <label>
        表示名
        <input
          aria-label={`slot ${source.slot} の表示名`}
          maxLength={13}
          value={drafts[source.slot] ?? source.name}
          onChange={(event) => setDrafts((current) => ({ ...current, [source.slot]: event.target.value }))}
        />
      </label>
      <div className="pairing-actions">
        <button type="button" className="primary" disabled={busySlot !== null} onClick={() => void rename(source)}>名前を保存</button>
        <button type="button" disabled={busySlot !== null} onClick={() => void remove(source)}>登録を削除</button>
      </div>
    </article>)}
    <p>空きスロット: {emptySlots.length > 0 ? emptySlots.join(', ') : 'なし'}</p>
  </section>;
}

/**
 * Shows the number the keyboard has to be told.
 *
 * The serial is what says a passkey is current: the value alone cannot
 * distinguish "this pairing's number" from one left over from a pairing that
 * already failed, and typing a stale number wastes the thirty seconds the
 * specification allows.
 */
function Passkey({ security }: { security: SecurityBlock }) {
  if (security.passkeySerial === 0) {
    return <p className="passkey-idle">パスキーはまだ生成されていません。</p>;
  }
  return <div className="passkey" role="status" aria-live="assertive">
    <span className="passkey-label">キーボードで打つ番号</span>
    <output className="passkey-value">{String(security.passkey).padStart(6, '0')}</output>
    <span className="passkey-serial">{security.passkeySerial}回目</span>
  </div>;
}

function Row({ label, value, warn = false }: { label: string; value: string; warn?: boolean }) {
  return <div className={warn ? 'diagnostic-row warn' : 'diagnostic-row'}>
    <dt>{label}</dt>
    <dd>{value}</dd>
  </div>;
}

/**
 * The board's own account of itself.
 *
 * Grouped the way a person reads it during bring-up: what the bridge is doing,
 * then whether keystrokes are getting through, then the two things that have
 * actually gone wrong on hardware — a pairing that would not stick and a key
 * left held on the host.
 */
function HardwareDiagnostics({ hardware }: { hardware: BridgeHardware }) {
  const { status, security, sources } = hardware;
  if (!status) {
    return <p role="status" aria-live="polite">実機から読み込んでいます…</p>;
  }
  const lost = status.inputReportsReceived - status.reportsForwarded;
  return <>
    {/* Which one is being shown has to be unmistakable. A number that came from
        a simulation and one that came from the board look identical, and acting
        on the wrong one is how a bring-up goes sideways. */}
    <p className={hardware.simulated ? 'simulation-label' : 'live-label'}>
      {hardware.simulated ? 'シミュレーションの値 — 実機ではありません' : '実機の値'}
      {' — '}
      {hardware.reading ? '更新中' : '1秒ごとに更新'}
    </p>
    <section className="diagnostic-panel">
      <h3>ブリッジの状態</h3>
      <dl>
        <Row label="状態" value={status.stateLabel} />
        <Row label="直近エラー" value={status.lastErrorLabel} warn={status.lastError !== 0} />
        <Row label="探索の到達点" value={status.discoveryStepLabel} />
        <Row label="接続回数" value={String(status.connectionCount)} />
        <Row label="直近の相手" value={`${status.lastAddress}（${status.lastRssi} dBm）`} />
        <Row label="広告観測数" value={`${status.advertisementsSeen}（うちHID ${status.hidAdvertisements}）`} />
      </dl>
    </section>
    <SourceCards sources={sources} />
    <section className="diagnostic-panel">
      <h3>打鍵</h3>
      <dl>
        <Row label="受信レポート数" value={String(status.inputReportsReceived)} />
        <Row label="USB転送数" value={String(status.reportsForwarded)} />
        <Row label="取りこぼし" value={lost === 0 ? 'なし' : `${lost}件`} warn={lost !== 0} />
        <Row label="直近パニック" value={status.panicCount === 0 ? 'なし' : `行${status.panicLine}（${status.panicCount}回）`} warn={status.panicCount !== 0} />
      </dl>
    </section>
    {security && <section className="diagnostic-panel">
      <h3>ペアリングと安全性</h3>
      <dl>
        <Row label="ボンド状態" value={security.bondSummary} />
        <Row label="押しっぱなし対策" value={security.stuckKeySummary} warn={security.stuckKeyReleases !== 0} />
        <Row label="注入の拒否理由" value={security.injectRefusalLabel} warn={security.injectRefusal !== 0} />
        <Row label="購読の拒否理由" value={`無線側 ${security.hogpRefusal} / USB側 ${security.usbHogpRefusal}`} warn={security.hogpRefusal !== 0 || security.usbHogpRefusal !== 0} />
        <Row label="通知の拒否理由" value={`${security.notifyRefusal}（着信 ${security.notifyHandle} / 期待 ${security.notifyExpected}）`} warn={security.notifyRefusal !== 0} />
      </dl>
    </section>}
  </>;
}

function Diagnostics({ snapshot, hardware, showSimulation }: { snapshot: BridgeSnapshot; hardware: BridgeHardware; showSimulation: boolean }) {
  if (hardware.device) {
    return <>
      <h2>診断</h2>
      <HardwareDiagnostics hardware={hardware} />
    </>;
  }
  if (!showSimulation) {
    return <>
      <h2>診断</h2>
      <p className="empty-state">ブリッジ未接続です。実機またはシミュレーションに接続すると、診断値を表示します。</p>
    </>;
  }
  const formattedReport = snapshot.hidOutput.map((byte) => byte.toString(16).padStart(2, '0').toUpperCase()).join(' ');
  return <>
    <h2>診断</h2>
    <p className="simulation-label">SIMULATED OUTPUT — デモで生成した値です。実機から取得した値ではありません。</p>
    <section className="diagnostic-panel" aria-labelledby="hid-output-title">
      <h3 id="hid-output-title">8-byte HID output</h3>
      <output className="hid-report">{formattedReport}</output>
    </section>
    <section className="diagnostic-panel" aria-labelledby="event-log-title">
      <h3 id="event-log-title">イベントログ</h3>
      <ol className="event-log">{snapshot.eventLog.map((event, index) => <li key={`${index}-${event}`}>{event}</li>)}</ol>
    </section>
  </>;
}
