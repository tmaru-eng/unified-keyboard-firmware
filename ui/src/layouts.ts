/**
 * Physical key layouts for the on-screen keyboard.
 *
 * A key carries the HID usage its physical switch reports, not the character it
 * produces. The whole point of the bridge is that the same usage means a
 * different character depending on the host's layout, so encoding characters
 * here would beg the question the test is meant to answer.
 */

/** A single key on the modelled keyboard. */
export interface VirtualKey {
  /** HID usage the switch reports. */
  usage: number;
  /** Legend printed on the physical keycap, unshifted. */
  cap: string;
  /** Legend printed on the keycap when shifted, when the two differ. */
  shiftedCap?: string;
}

/** Which physical arrangement the on-screen keyboard is imitating. */
export type LayoutName = 'ansi-us' | 'jis';

const DIGIT_ROW_US: VirtualKey[] = [
  { usage: 0x1e, cap: '1', shiftedCap: '!' },
  { usage: 0x1f, cap: '2', shiftedCap: '@' },
  { usage: 0x20, cap: '3', shiftedCap: '#' },
  { usage: 0x21, cap: '4', shiftedCap: '$' },
  { usage: 0x22, cap: '5', shiftedCap: '%' },
  { usage: 0x23, cap: '6', shiftedCap: '^' },
  { usage: 0x24, cap: '7', shiftedCap: '&' },
  { usage: 0x25, cap: '8', shiftedCap: '*' },
  { usage: 0x26, cap: '9', shiftedCap: '(' },
  { usage: 0x27, cap: '0', shiftedCap: ')' },
];

const DIGIT_ROW_JIS: VirtualKey[] = [
  { usage: 0x1e, cap: '1', shiftedCap: '!' },
  { usage: 0x1f, cap: '2', shiftedCap: '"' },
  { usage: 0x20, cap: '3', shiftedCap: '#' },
  { usage: 0x21, cap: '4', shiftedCap: '$' },
  { usage: 0x22, cap: '5', shiftedCap: '%' },
  { usage: 0x23, cap: '6', shiftedCap: '&' },
  { usage: 0x24, cap: '7', shiftedCap: "'" },
  { usage: 0x25, cap: '8', shiftedCap: '(' },
  { usage: 0x26, cap: '9', shiftedCap: ')' },
  { usage: 0x27, cap: '0', shiftedCap: '' },
];

const SYMBOLS_US: VirtualKey[] = [
  { usage: 0x2d, cap: '-', shiftedCap: '_' },
  { usage: 0x2e, cap: '=', shiftedCap: '+' },
  { usage: 0x2f, cap: '[', shiftedCap: '{' },
  { usage: 0x30, cap: ']', shiftedCap: '}' },
  { usage: 0x31, cap: '\\', shiftedCap: '|' },
  { usage: 0x33, cap: ';', shiftedCap: ':' },
  { usage: 0x34, cap: "'", shiftedCap: '"' },
  { usage: 0x35, cap: '`', shiftedCap: '~' },
  { usage: 0x36, cap: ',', shiftedCap: '<' },
  { usage: 0x37, cap: '.', shiftedCap: '>' },
  { usage: 0x38, cap: '/', shiftedCap: '?' },
];

const SYMBOLS_JIS: VirtualKey[] = [
  { usage: 0x2d, cap: '-', shiftedCap: '=' },
  { usage: 0x2e, cap: '^', shiftedCap: '~' },
  { usage: 0x2f, cap: '@', shiftedCap: '`' },
  { usage: 0x30, cap: '[', shiftedCap: '{' },
  { usage: 0x31, cap: ']', shiftedCap: '}' },
  { usage: 0x33, cap: ';', shiftedCap: '+' },
  { usage: 0x34, cap: ':', shiftedCap: '*' },
  { usage: 0x36, cap: ',', shiftedCap: '<' },
  { usage: 0x37, cap: '.', shiftedCap: '>' },
  { usage: 0x38, cap: '/', shiftedCap: '?' },
  { usage: 0x87, cap: 'ろ', shiftedCap: '_' },
  { usage: 0x89, cap: '¥', shiftedCap: '|' },
];

const LETTERS: VirtualKey[] = [
  { usage: 0x04, cap: 'A' },
  { usage: 0x05, cap: 'B' },
  { usage: 0x06, cap: 'C' },
];

const JIS_ONLY: VirtualKey[] = [
  { usage: 0x8a, cap: '変換' },
  { usage: 0x8b, cap: '無変換' },
  { usage: 0x90, cap: 'かな' },
  { usage: 0x91, cap: '英数' },
];

/** One labelled group of keys shown as a row. */
export interface KeyRow {
  label: string;
  keys: VirtualKey[];
}

/** Returns the rows to display for a layout. */
export function layoutRows(layout: LayoutName): KeyRow[] {
  if (layout === 'jis') {
    return [
      { label: '数字段', keys: DIGIT_ROW_JIS },
      { label: '記号', keys: SYMBOLS_JIS },
      { label: '日本語キー', keys: JIS_ONLY },
      { label: '文字', keys: LETTERS },
    ];
  }
  return [
    { label: '数字段', keys: DIGIT_ROW_US },
    { label: '記号', keys: SYMBOLS_US },
    { label: '文字', keys: LETTERS },
  ];
}

/** Human-readable name of a layout. */
export function layoutLabel(layout: LayoutName): string {
  return layout === 'jis' ? 'JIS' : 'ANSI US';
}
