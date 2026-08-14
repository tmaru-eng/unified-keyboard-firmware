import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { KeymapEditor } from './KeymapEditor';
import type { KeymapRule } from './keymap';

afterEach(() => {
  cleanup();
});

describe('Vial風の固定キーマップエディタ', () => {
  it('物理キーを選ぶと現在の出力をパレットへ表示する', () => {
    const onChange = vi.fn();
    const rules: KeymapRule[] = [
      { inputUsage: 4, inputShifted: false, outputUsage: 5, outputShifted: false },
    ];

    render(<KeymapEditor rules={rules} disabled={false} onChange={onChange} />);

    const selectedKey = screen.getByRole('button', { name: /A.*0x04/ });
    fireEvent.click(selectedKey);

    expect(screen.getByRole('combobox', { name: '出力キー' })).toHaveValue('5');
    expect(selectedKey).toHaveTextContent('B');
  });

  it('Shift入力を選び、出力キーを変更すると対応するルールだけを更新する', () => {
    const onChange = vi.fn();
    const rules: KeymapRule[] = [
      { inputUsage: 4, inputShifted: false, outputUsage: 5, outputShifted: false },
    ];

    render(<KeymapEditor rules={rules} disabled={false} onChange={onChange} />);

    fireEvent.click(screen.getByRole('button', { name: 'Shift入力' }));
    fireEvent.click(screen.getByRole('button', { name: /A.*0x04/ }));
    fireEvent.change(screen.getByRole('combobox', { name: '出力キー' }), { target: { value: '6' } });

    expect(onChange).toHaveBeenLastCalledWith([
      rules[0],
      { inputUsage: 4, inputShifted: true, outputUsage: 6, outputShifted: false },
    ]);
  });

  it('既存ルールを解除すると素通しに戻る', () => {
    const onChange = vi.fn();
    const rules: KeymapRule[] = [
      { inputUsage: 4, inputShifted: false, outputUsage: 5, outputShifted: false },
    ];

    render(<KeymapEditor rules={rules} disabled={false} onChange={onChange} />);
    fireEvent.click(screen.getByRole('button', { name: /A.*0x04/ }));
    fireEvent.click(screen.getByRole('button', { name: 'この入力を素通し' }));

    expect(onChange).toHaveBeenLastCalledWith([]);
  });

  it('32ルールを超える追加を拒否し、理由を表示する', () => {
    const onChange = vi.fn();
    const rules: KeymapRule[] = Array.from({ length: 32 }, (_, index) => ({
      inputUsage: 4 + index,
      inputShifted: false,
      outputUsage: 4 + index,
      outputShifted: false,
    }));

    render(<KeymapEditor rules={rules} disabled={false} onChange={onChange} />);
    fireEvent.click(screen.getByRole('button', { name: 'RCtrl (0xe8)' }));
    fireEvent.change(screen.getByRole('combobox', { name: '出力キー' }), { target: { value: '5' } });

    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toHaveTextContent('32件まで');
  });
});
