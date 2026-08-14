/** Keys shown by the first visual editor slice. Labels are keycaps; usages are wire values. */
export interface EditorKey {
  usage: number;
  label: string;
}

export interface EditorKeyRow {
  label: string;
  keys: EditorKey[];
}

function key(usage: number, label: string): EditorKey {
  return { usage, label };
}

const letters = (labels: string): EditorKey[] => [...labels].map((label) =>
  key(0x04 + label.charCodeAt(0) - 'A'.charCodeAt(0), label),
);

export const KEYMAP_EDITOR_ROWS: EditorKeyRow[] = [
  {
    label: '機能',
    keys: [
      key(0x29, 'Esc'),
      ...Array.from({ length: 12 }, (_, index) => key(0x3a + index, `F${index + 1}`)),
    ],
  },
  {
    label: '数字',
    keys: [
      ...[...'1234567890'].map((label, index) => key(0x1e + index, label)),
      key(0x2d, '-'), key(0x2e, '='), key(0x2a, 'Backspace'),
    ],
  },
  {
    label: '上段',
    keys: [key(0x2b, 'Tab'), ...letters('QWERTYUIOP'), key(0x2f, '['), key(0x30, ']'), key(0x31, '\\')],
  },
  {
    label: '中段',
    keys: [key(0x39, 'Caps'), ...letters('ASDFGHJKL'), key(0x33, ';'), key(0x34, "'"), key(0x28, 'Enter')],
  },
  {
    label: '下段',
    keys: [key(0xe1, 'LShift'), ...letters('ZXCVBNM'), key(0x36, ','), key(0x37, '.'), key(0x38, '/'), key(0xe5, 'RShift')],
  },
  {
    label: '修飾',
    keys: [key(0xe0, 'LCtrl'), key(0xe3, 'LGUI'), key(0xe2, 'LAlt'), key(0x2c, 'Space'), key(0xe6, 'RAlt'), key(0xe7, 'RGUI'), key(0xe4, 'Menu'), key(0xe8, 'RCtrl')],
  },
];

export const KEYMAP_EDITOR_KEYS = KEYMAP_EDITOR_ROWS.flatMap((row) => row.keys);

export function editorKeyLabel(usage: number): string {
  return KEYMAP_EDITOR_KEYS.find((item) => item.usage === usage)?.label ?? `usage 0x${usage.toString(16).padStart(2, '0')}`;
}

export function editorKeyUsageLabel(usage: number): string {
  return `${editorKeyLabel(usage)} (0x${usage.toString(16).padStart(2, '0')})`;
}
