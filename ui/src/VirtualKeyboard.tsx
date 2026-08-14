import { useCallback, useEffect, useRef, useState } from 'react';

import { MODIFIER } from './injection';
import { type LayoutName, type VirtualKey, layoutLabel, layoutRows } from './layouts';
import type { BridgeHardware } from './useBridgeHardware';
import { injectSourceReport } from './webhid';

/** One observed round trip: what was pressed, and what the host received. */
interface Observation {
  id: number;
  pressed: string;
  usage: number;
  shifted: boolean;
  received: string | null;
}

const MAX_OBSERVATIONS = 12;

/**
 * Drives the bridge from an on-screen keyboard and shows what the host got back.
 *
 * Pressing a key sends the source report the modelled keyboard would have sent.
 * The firmware converts it and emits a real USB keystroke, which the operating
 * system then delivers to whatever has focus — this page. Capturing that
 * `keydown` closes the loop through the hardware and the host's own layout
 * setting, which is the one thing no host test can stand in for.
 */
export function VirtualKeyboard({ hardware }: { hardware: BridgeHardware }) {
  const device = hardware.device;
  const [layout, setLayout] = useState<LayoutName>('ansi-us');
  const [shifted, setShifted] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [observations, setObservations] = useState<Observation[]>([]);
  const captureRef = useRef<HTMLDivElement>(null);
  const pendingRef = useRef<number | null>(null);
  const nextId = useRef(0);

  // The connection is owned by the application shell now, so the same board
  // serves this screen and the diagnostics. Connecting twice from two screens
  // produced two handles to one interface and interleaved their reports.
  const connect = useCallback(async () => {
    setError(null);
    await hardware.connect();
    captureRef.current?.focus();
  }, [hardware]);

  const press = useCallback(
    async (key: VirtualKey) => {
      if (!device) {
        setError('先にブリッジへ接続してください。');
        return;
      }
      setError(null);
      captureRef.current?.focus();

      const id = nextId.current;
      nextId.current += 1;
      pendingRef.current = id;
      setObservations((current) =>
        [
          {
            id,
            pressed: shifted && key.shiftedCap ? key.shiftedCap : key.cap,
            usage: key.usage,
            shifted,
            received: null,
          },
          ...current,
        ].slice(0, MAX_OBSERVATIONS),
      );

      const modifiers = shifted ? MODIFIER.leftShift : 0;
      try {
        await injectSourceReport(device, modifiers, [key.usage]);
        // A press with no release leaves the key held on the host.
        await injectSourceReport(device, 0, []);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      }
    },
    [device, shifted],
  );

  useEffect(() => {
    const node = captureRef.current;
    if (!node) {
      return undefined;
    }
    const onKeyDown = (event: KeyboardEvent) => {
      // The keystroke arrives from the board, not from the operator, so it must
      // not also reach the page as ordinary text input.
      event.preventDefault();
      const id = pendingRef.current;
      if (id === null) {
        return;
      }
      pendingRef.current = null;
      const received = event.key === ' ' ? '␣' : event.key;
      setObservations((current) =>
        current.map((item) => (item.id === id ? { ...item, received } : item)),
      );
    };
    node.addEventListener('keydown', onKeyDown);
    return () => node.removeEventListener('keydown', onKeyDown);
  }, []);

  return (
    <section className="virtual-keyboard">
      <header>
        <h2>バーチャルキーボード</h2>
        <p>
          画面のキーを押すと、その物理キーのHID usage をブリッジへ送ります。ブリッジが変換し、
          実際のUSBキーストロークとしてこのページに返ってきたものを下に表示します。
        </p>
      </header>

      <div className="virtual-keyboard__controls">
        <button type="button" onClick={connect} disabled={device !== null}>
          {device ? '接続済み' : 'ブリッジに接続'}
        </button>
        <label>
          配列
          <select value={layout} onChange={(event) => setLayout(event.target.value as LayoutName)}>
            <option value="ansi-us">{layoutLabel('ansi-us')}</option>
            <option value="jis">{layoutLabel('jis')}</option>
          </select>
        </label>
        <label>
          <input type="checkbox" checked={shifted} onChange={(event) => setShifted(event.target.checked)} />
          Shift
        </label>
      </div>

      {error ? <p role="alert">{error}</p> : null}

      {layoutRows(layout).map((row) => (
        <div key={row.label} className="virtual-keyboard__row">
          <span className="virtual-keyboard__row-label">{row.label}</span>
          {row.keys.map((key) => (
            <button
              key={key.usage}
              type="button"
              title={`usage 0x${key.usage.toString(16).padStart(2, '0')}`}
              onClick={() => void press(key)}
            >
              {shifted && key.shiftedCap ? key.shiftedCap : key.cap}
            </button>
          ))}
        </div>
      ))}

      <div
        ref={captureRef}
        className="virtual-keyboard__capture"
        tabIndex={0}
        role="log"
        aria-label="ホストが受信したキー"
      >
        フォーカスをここに置いてください。ブリッジからのキーストロークを捕捉します。
      </div>

      <table className="virtual-keyboard__observations">
        <thead>
          <tr>
            <th>押した物理キー</th>
            <th>usage</th>
            <th>ホストが受信</th>
          </tr>
        </thead>
        <tbody>
          {observations.map((item) => (
            <tr key={item.id}>
              <td>
                {item.shifted ? 'Shift + ' : ''}
                {item.pressed}
              </td>
              <td>0x{item.usage.toString(16).padStart(2, '0')}</td>
              <td>{item.received ?? '待機中'}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
