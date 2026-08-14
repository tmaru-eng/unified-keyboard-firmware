import { useState } from 'react';

import { KEYMAP_RULE_CAPACITY, type KeymapRule } from './keymap';
import {
  editorKeyLabel,
  editorKeyUsageLabel,
  KEYMAP_EDITOR_KEYS,
  KEYMAP_EDITOR_ROWS,
} from './keymapEditorLayout';

interface KeymapEditorProps {
  rules: readonly KeymapRule[];
  disabled: boolean;
  onChange: (rules: KeymapRule[]) => void;
}

/**
 * Visual editor for the current single-layer wire contract.
 *
 * This deliberately does not pretend to support future Vial actions. The
 * palette exposes only the output usage and Shift bit that the firmware can
 * currently persist.
 */
export function KeymapEditor({ rules, disabled, onChange }: KeymapEditorProps) {
  const [selectedUsage, setSelectedUsage] = useState(0x04);
  const [inputShifted, setInputShifted] = useState(false);
  const [editorError, setEditorError] = useState<string | null>(null);
  const selectedRule = rules.find(
    (rule) => rule.inputUsage === selectedUsage && rule.inputShifted === inputShifted,
  );
  const outputUsage = selectedRule?.outputUsage ?? selectedUsage;
  const outputShifted = selectedRule?.outputShifted ?? false;
  const outputOptionExists = KEYMAP_EDITOR_KEYS.some((key) => key.usage === outputUsage);
  const selectedLabel = editorKeyLabel(selectedUsage);

  function changeRule(next: Pick<KeymapRule, 'outputUsage' | 'outputShifted'>): void {
    setEditorError(null);
    const index = rules.findIndex(
      (rule) => rule.inputUsage === selectedUsage && rule.inputShifted === inputShifted,
    );
    const replacement: KeymapRule = {
      inputUsage: selectedUsage,
      inputShifted,
      outputUsage: next.outputUsage,
      outputShifted: next.outputShifted,
    };
    if (index >= 0) {
      onChange(rules.map((rule, ruleIndex) => (ruleIndex === index ? replacement : rule)));
      return;
    }
    if (rules.length >= KEYMAP_RULE_CAPACITY) {
      setEditorError(`ルールは${KEYMAP_RULE_CAPACITY}件までです。先に不要なルールを解除してください。`);
      return;
    }
    onChange([...rules, replacement]);
  }

  function clearRule(): void {
    setEditorError(null);
    onChange(rules.filter(
      (rule) => !(rule.inputUsage === selectedUsage && rule.inputShifted === inputShifted),
    ));
  }

  return (
    <section className="keymap-visual-editor" aria-labelledby="visual-keymap-title">
      <div className="keymap-visual-editor__heading">
        <div>
          <h4 id="visual-keymap-title">ビジュアルキーマップ</h4>
          <p>キーを選択して、現在のファームウェアが保存できる出力を割り当てます。</p>
        </div>
        <span className="keymap-capacity">{rules.length}/{KEYMAP_RULE_CAPACITY}ルール</span>
      </div>

      <div className="keymap-input-modes" aria-label="入力状態">
        <button type="button" aria-pressed={!inputShifted} disabled={disabled} onClick={() => setInputShifted(false)}>
          通常入力
        </button>
        <button type="button" aria-pressed={inputShifted} disabled={disabled} onClick={() => setInputShifted(true)}>
          Shift入力
        </button>
      </div>

      <div className="keymap-visual-editor__keyboard" aria-label="キーボード配置">
        {KEYMAP_EDITOR_ROWS.map((row) => (
          <div className="keymap-editor-row" key={row.label}>
            <span className="keymap-editor-row__label">{row.label}</span>
            {row.keys.map((key) => {
              const rule = rules.find((item) => item.inputUsage === key.usage && item.inputShifted === inputShifted);
              const mapped = rule ? `${editorKeyLabel(rule.outputUsage)}${rule.outputShifted ? ' + Shift' : ''}` : '素通し';
              return (
                <button
                  className={key.usage === selectedUsage ? 'keymap-editor-key selected' : 'keymap-editor-key'}
                  key={key.usage}
                  type="button"
                  aria-label={editorKeyUsageLabel(key.usage)}
                  aria-pressed={key.usage === selectedUsage}
                  disabled={disabled}
                  onClick={() => { setEditorError(null); setSelectedUsage(key.usage); }}
                >
                  <strong>{key.label}</strong>
                  <small>{mapped}</small>
                </button>
              );
            })}
          </div>
        ))}
      </div>

      <div className="keymap-action-panel">
        <div>
          <span className="keymap-action-panel__eyebrow">選択中</span>
          <strong>{selectedLabel} / {inputShifted ? 'Shift入力' : '通常入力'}</strong>
        </div>
        <label>
          出力キー
          <select
            aria-label="出力キー"
            value={String(outputUsage)}
            disabled={disabled}
            onChange={(event) => changeRule({ outputUsage: Number(event.target.value), outputShifted })}
          >
            {!outputOptionExists && <option value={String(outputUsage)}>{editorKeyUsageLabel(outputUsage)}</option>}
            {KEYMAP_EDITOR_KEYS.map((key) => <option key={key.usage} value={String(key.usage)}>{editorKeyUsageLabel(key.usage)}</option>)}
          </select>
        </label>
        <label className="keymap-action-panel__checkbox">
          <input
            type="checkbox"
            aria-label="出力Shift"
            checked={outputShifted}
            disabled={disabled}
            onChange={(event) => changeRule({ outputUsage, outputShifted: event.target.checked })}
          />
          出力にShiftを付ける
        </label>
        <button type="button" disabled={disabled || selectedRule === undefined} onClick={clearRule}>
          この入力を素通し
        </button>
      </div>
      {editorError && <p className="keymap-validation" role="alert">{editorError}</p>}
    </section>
  );
}
