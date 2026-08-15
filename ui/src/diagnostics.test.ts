import { describe, expect, it } from 'vitest';
import {
  CONFIG_REPORT_ID,
  CONFIG_REPORT_LEN,
  CONFIG_PROTOCOL_VERSION,
  crc32Ieee,
} from './injection';
import {
  SECURITY_KIND,
  KEYMAP_KIND,
  KEYMAP_REPORT_VERSION,
  SOURCE_KIND,
  STATUS_KIND,
  buildSelectBlock,
  buildSelectKeymapChunk,
  buildSelectSource,
  buildSelectSourceKeymapChunk,
  decodeKeymapChunk,
  decodeSourceKeymapChunk,
  decodeSecurity,
  decodeSource,
  decodeStatus,
  selectFraming,
} from './diagnostics';

/** Builds a block the way the firmware's encoders do, CRC included. */
function block(kind: number, fill: (view: DataView, bytes: Uint8Array) => void): Uint8Array {
  const bytes = new Uint8Array(CONFIG_REPORT_LEN);
  const view = new DataView(bytes.buffer);
  bytes[0] = CONFIG_PROTOCOL_VERSION;
  bytes[1] = kind;
  fill(view, bytes);
  view.setUint32(28, crc32Ieee(bytes.subarray(0, 28)), true);
  return bytes;
}

describe('status block', () => {
  it('decodes every field from the offsets the firmware writes', () => {
    // Offsets are asserted rather than trusted because both sides write them by
    // hand. A field added on one side and not the other does not fail loudly —
    // it silently reports a neighbouring byte.
    const bytes = block(STATUS_KIND, (view, raw) => {
      raw[2] = 5; // subscribed
      raw[3] = 0x49;
      view.setUint16(4, 8550, true);
      view.setUint16(6, 1, true);
      raw.set([0x25, 0x55, 0x13, 0x25, 0xe9, 0xdb], 8);
      raw[14] = 0xc6; // -58 as a signed byte
      raw[15] = 1;
      view.setUint32(16, 131, true);
      view.setUint32(20, 131, true);
      view.setUint16(24, 0, true);
      raw[26] = 0;
      raw[27] = 9;
    });

    const status = decodeStatus(bytes);

    expect(status.state).toBe(5);
    expect(status.lastError).toBe(0x49);
    expect(status.advertisementsSeen).toBe(8550);
    expect(status.hidAdvertisements).toBe(1);
    expect(status.lastAddress).toBe('db:e9:25:13:55:25');
    expect(status.lastRssi).toBe(-58);
    expect(status.connectionCount).toBe(1);
    expect(status.inputReportsReceived).toBe(131);
    expect(status.reportsForwarded).toBe(131);
    expect(status.panicCount).toBe(0);
    expect(status.discoveryStep).toBe(9);
  });

  it('reads the address in the order a person writes it', () => {
    // The wire order is least significant byte first. Printing it unreversed
    // produces an address that matches nothing the keyboard or any scanner
    // shows, which during bring-up reads as the wrong peer.
    const bytes = block(STATUS_KIND, (_view, raw) => {
      raw.set([1, 2, 3, 4, 5, 6], 8);
    });

    expect(decodeStatus(bytes).lastAddress).toBe('06:05:04:03:02:01');
  });

  it('refuses a block whose CRC does not cover its contents', () => {
    const bytes = block(STATUS_KIND, (_view, raw) => {
      raw[15] = 3;
    });
    bytes[15] ^= 1;

    expect(() => decodeStatus(bytes)).toThrow(/CRC/);
  });

  it('refuses another kind rather than reading its bytes as a status', () => {
    expect(() => decodeStatus(block(SECURITY_KIND, () => {}))).toThrow(/種別/);
  });

  it('refuses an unsupported protocol version', () => {
    const bytes = block(STATUS_KIND, () => {});
    bytes[0] = CONFIG_PROTOCOL_VERSION + 1;
    const view = new DataView(bytes.buffer);
    view.setUint32(28, crc32Ieee(bytes.subarray(0, 28)), true);

    expect(() => decodeStatus(bytes)).toThrow(/バージョン/);
  });
});

describe('keymap chunks', () => {
  it('builds the selector and decodes a padded payload chunk', () => {
    const selector = buildSelectKeymapChunk(2);
    expect(selector[1]).toBe(108);
    expect(selector[2]).toBe(2);

    const payload = Uint8Array.from([1, 1, 0x04, 0, 0x05, 0]);
    const chunk = decodeKeymapChunk(
      block(KEYMAP_KIND, (view, raw) => {
        raw[2] = KEYMAP_REPORT_VERSION;
        raw[3] = 0;
        raw[4] = 1;
        view.setUint16(5, payload.length, true);
        raw.set(payload, 7);
      }),
    );

    expect(chunk.chunkIndex).toBe(0);
    expect(chunk.chunkCount).toBe(1);
    expect(chunk.payloadLength).toBe(payload.length);
    expect(Array.from(chunk.data)).toEqual(Array.from(payload));
  });

  it('rejects an invalid chunk index and non-zero padding', () => {
    expect(() => decodeKeymapChunk(block(KEYMAP_KIND, (_view, raw) => {
      raw[2] = KEYMAP_REPORT_VERSION;
      raw[3] = 1;
      raw[4] = 1;
      new DataView(raw.buffer).setUint16(5, 2, true);
    }))).toThrow(/invalid keymap chunk/);

    expect(() => decodeKeymapChunk(block(KEYMAP_KIND, (_view, raw) => {
      raw[2] = KEYMAP_REPORT_VERSION;
      raw[3] = 0;
      raw[4] = 1;
      new DataView(raw.buffer).setUint16(5, 2, true);
      raw[9] = 1;
    }))).toThrow(/padding/);

    expect(() => decodeKeymapChunk(block(KEYMAP_KIND, (_view, raw) => {
      raw[2] = KEYMAP_REPORT_VERSION;
      raw[3] = 0;
      raw[4] = 1;
      new DataView(raw.buffer).setUint16(5, 22, true);
    }))).toThrow(/chunk count/);
  });
});

describe('security block', () => {
  it('decodes the pairing and stuck-key fields', () => {
    const bytes = block(SECURITY_KIND, (view, raw) => {
      raw[2] = 0x13;
      raw[3] = 0;
      view.setUint32(4, 111111, true);
      view.setUint32(8, 2, true);
      raw[12] = 0x05;
      raw[13] = 0;
      raw[14] = 0;
      raw[15] = 0;
      view.setUint16(16, 24, true);
      view.setUint16(18, 24, true);
      view.setUint16(20, 0, true);
      raw[22] = 0;
      raw[23] = 0;
      raw[24] = 2; // stuck-key releases
      raw[25] = 0; // liveness probe failures
      raw[26] = 0; // last probe failure
      raw[27] = 0; // injection refusal
    });

    const security = decodeSecurity(bytes);

    expect(security.passkey).toBe(111111);
    expect(security.passkeySerial).toBe(2);
    expect(security.bondFlags).toBe(0x05);
    expect(security.stuckKeyReleases).toBe(2);
    expect(security.livenessProbeFailures).toBe(0);
    expect(security.injectRefusal).toBe(0);
    expect(security.notifyHandle).toBe(24);
    expect(security.notifyExpected).toBe(24);
  });

  it('names the bond flags rather than showing a number', () => {
    const bytes = block(SECURITY_KIND, (_view, raw) => {
      raw[12] = 0x05;
    });

    const security = decodeSecurity(bytes);

    expect(security.bondSummary).toContain('起動時に鍵を読み込んだ');
    expect(security.bondSummary).toContain('直近の相手はボンド済み');
    expect(security.bondSummary).not.toContain('鍵の書き込みに失敗した');
  });

  it('says a quiet watchdog is quiet instead of printing zeros', () => {
    const security = decodeSecurity(block(SECURITY_KIND, () => {}));

    expect(security.stuckKeySummary).toBe('発動なし');
  });

  it('reports a fired watchdog with its reason', () => {
    const bytes = block(SECURITY_KIND, (_view, raw) => {
      raw[24] = 1;
      raw[25] = 1;
      raw[26] = 0x58; // subscribed phase, timeout
    });

    const summary = decodeSecurity(bytes).stuckKeySummary;

    expect(summary).toContain('全解放1回');
    expect(summary).toContain('購読中');
    expect(summary).toContain('タイムアウト');
  });
});

describe('blocks captured from the board', () => {
  // Captured on 2026-08-12 from the XIAO running
  // `ukf-xiao-ble-probe-watchdog2.uf2`, and decoded by `read_diagnostics.py`,
  // which is the implementation that has been run against hardware. A fixture
  // built by the same hand that wrote the decoder proves only self-consistency;
  // these bytes prove the decoder agrees with the firmware's encoder.
  const hex = (text: string) =>
    Uint8Array.from(text.match(/../g)!.map((pair) => parseInt(pair, 16)));

  it('decodes a real status block the way the Python tool does', () => {
    const status = decodeStatus(
      hex('120101490e43e500fcd74963e43dac057b0600007b060000000000097053f141'),
    );

    expect(status.stateLabel).toBe('スキャン中');
    expect(status.lastErrorLabel).toBe('0x49 探索 / リンク切断');
    expect(status.advertisementsSeen).toBe(17166);
    expect(status.hidAdvertisements).toBe(229);
    expect(status.lastAddress).toBe('3d:e4:63:49:d7:fc');
    expect(status.lastRssi).toBe(-84);
    expect(status.connectionCount).toBe(5);
    expect(status.inputReportsReceived).toBe(1659);
    expect(status.reportsForwarded).toBe(1659);
    expect(status.panicCount).toBe(0);
    expect(status.discoveryStepLabel).toBe('9 通知の購読');
  });

  it('decodes a real security block the way the Python tool does', () => {
    const security = decodeSecurity(
      hex('12030000000000000000000005000000180018000000000002000000433eedc5'),
    );

    expect(security.bondSummary).toBe('起動時に鍵を読み込んだ／直近の相手はボンド済み');
    expect(security.stuckKeySummary).toBe(
      '全解放2回 / 生存確認の無応答0回 / 直近の失敗理由 なし',
    );
    expect(security.notifyHandle).toBe(24);
    expect(security.notifyExpected).toBe(24);
    expect(security.injectRefusalLabel).toBe('なし');
    expect(security.linkSetupFailed).toBe(false);
  });
});

describe('framing', () => {
  it('accepts the payload whether or not the report ID is prefixed', () => {
    // Whether the browser hands back the report ID depends on the platform, so
    // the CRC decides rather than a guess. A wrong guess misaligns every field
    // while still looking like a plausible report.
    const payload = block(STATUS_KIND, (_view, raw) => {
      raw[15] = 7;
    });
    const prefixed = new Uint8Array(CONFIG_REPORT_LEN + 1);
    prefixed[0] = CONFIG_REPORT_ID;
    prefixed.set(payload, 1);

    expect(decodeStatus(selectFraming(payload)).connectionCount).toBe(7);
    expect(decodeStatus(selectFraming(prefixed)).connectionCount).toBe(7);
  });

  it('refuses a prefixed payload with the wrong report ID', () => {
    const prefixed = new Uint8Array(CONFIG_REPORT_LEN + 1);
    prefixed[0] = CONFIG_REPORT_ID + 1;
    prefixed.set(block(STATUS_KIND, () => undefined), 1);

    expect(() => selectFraming(prefixed)).toThrow(/report ID/);
  });

  it('refuses an oversized payload even when a candidate block has a valid CRC', () => {
    const oversized = new Uint8Array(CONFIG_REPORT_LEN + 2);
    oversized[0] = CONFIG_REPORT_ID;
    oversized.set(block(STATUS_KIND, () => undefined), 1);

    expect(() => selectFraming(oversized)).toThrow(/report length/);
  });

  it('refuses a payload that validates under no framing', () => {
    const noise = new Uint8Array(CONFIG_REPORT_LEN).fill(0x5a);

    expect(() => selectFraming(noise)).toThrow(/CRC/);
  });
});

describe('block selection', () => {
  it('builds a selector the firmware accepts', () => {
    const selector = buildSelectBlock(0xfe);

    expect(selector).toHaveLength(CONFIG_REPORT_LEN);
    expect(selector[0]).toBe(CONFIG_PROTOCOL_VERSION);
    expect(selector[1]).toBe(101); // UKF_SELECT_PANIC_CHUNK
    expect(selector[2]).toBe(0xfe);
    const view = new DataView(selector.buffer, selector.byteOffset);
    expect(view.getUint32(28, true)).toBe(crc32Ieee(selector.subarray(0, 28)));
  });

  it('builds a slot-specific source selector with explicit command bytes', () => {
    const selector = buildSelectSource(4);

    expect(selector[0]).toBe(CONFIG_PROTOCOL_VERSION);
    expect(selector[1]).toBe(107); // UKF_SELECT_SOURCE
    expect(selector[2]).toBe(4);
    expect(selector.slice(3, 28).every((byte) => byte === 0)).toBe(true);
    const view = new DataView(selector.buffer, selector.byteOffset);
    expect(view.getUint32(28, true)).toBe(crc32Ieee(selector.subarray(0, 28)));
  });

  it('builds a source-keymap selector with explicit slot and chunk bytes', () => {
    const selector = buildSelectSourceKeymapChunk(3, 2);

    expect(selector[0]).toBe(CONFIG_PROTOCOL_VERSION);
    expect(selector[1]).toBe(109); // UKF_SELECT_SOURCE_KEYMAP
    expect(selector[2]).toBe(3);
    expect(selector[3]).toBe(2);
    expect(selector.slice(4, 28).every((byte) => byte === 0)).toBe(true);
    const view = new DataView(selector.buffer, selector.byteOffset);
    expect(view.getUint32(28, true)).toBe(crc32Ieee(selector.subarray(0, 28)));
  });
});

describe('source-keymap block', () => {
  it('decodes the version-two slot and payload offset', () => {
    const payload = Uint8Array.from([2, 3, 1, 4, 2, 5, 0]);
    const bytes = block(KEYMAP_KIND, (view, raw) => {
      raw[2] = 2;
      raw[3] = 0;
      raw[4] = 1;
      view.setUint16(5, payload.length, true);
      raw[7] = 3;
      raw.set(payload, 8);
    });

    const decoded = decodeSourceKeymapChunk(bytes);

    expect(decoded.sourceSlot).toBe(3);
    expect(decoded.chunkIndex).toBe(0);
    expect(Array.from(decoded.data)).toEqual(Array.from(payload));
  });

  it('rejects a source-keymap block whose slot is outside the registration range', () => {
    const bytes = block(KEYMAP_KIND, (view, raw) => {
      raw[2] = 2;
      raw[4] = 1;
      view.setUint16(5, 3, true);
      raw[7] = 5;
      raw.set([2, 0, 0], 8);
    });

    expect(() => decodeSourceKeymapChunk(bytes)).toThrow(/source keymap slot/);
  });
});

describe('source block', () => {
  it('decodes the fixed virtual-slot wire bytes without exposing an IRK', () => {
    const bytes = Uint8Array.from([
      0x12, 0x07, 0x01, 0x04, 0x03, 0x05, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x0d, 0x56, 0x69, 0x72, 0x74, 0x75, 0x61, 0x6c, 0x20, 0x49, 0x6e, 0x70, 0x75, 0x74,
      0xcb, 0x7c, 0xa5, 0xd8,
    ]);

    const source = decodeSource(bytes);

    expect(source.kind).toBe(SOURCE_KIND);
    expect(source.slot).toBe(4);
    expect(source.transportLabel).toBe('Virtual');
    expect(source.stateLabel).toBe('接続済み');
    expect(source.identityAddress).toBeNull();
    expect(source.irkPresent).toBe(false);
    expect(source.profileFlags).toBe(0x05);
    expect(source.name).toBe('Virtual Input');
  });

  it('rejects an IRK flag without an identity address', () => {
    const bytes = Uint8Array.from([
      0x12, 0x07, 0x01, 0x04, 0x03, 0x05, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x0d, 0x56, 0x69, 0x72, 0x74, 0x75, 0x61, 0x6c, 0x20, 0x49, 0x6e, 0x70, 0x75, 0x74,
      0x00, 0x00, 0x00, 0x00,
    ]);
    const view = new DataView(bytes.buffer);
    view.setUint32(28, crc32Ieee(bytes.subarray(0, 28)), true);

    expect(() => decodeSource(bytes)).toThrow(/IRK/);
  });

  it('rejects unknown slots, reserved bytes, and CRC failures', () => {
    const valid = Uint8Array.from([
      0x12, 0x07, 0x01, 0x04, 0x03, 0x05, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x0d, 0x56, 0x69, 0x72, 0x74, 0x75, 0x61, 0x6c, 0x20, 0x49, 0x6e, 0x70, 0x75, 0x74,
      0xcb, 0x7c, 0xa5, 0xd8,
    ]);
    const unknown = valid.slice();
    unknown[3] = 5;
    unknown[28] = 0;
    expect(() => decodeSource(unknown)).toThrow();

    const reserved = valid.slice();
    reserved[27] = 1;
    reserved[28] = 0;
    expect(() => decodeSource(reserved)).toThrow();

    const corrupt = valid.slice();
    corrupt[7] ^= 1;
    expect(() => decodeSource(corrupt)).toThrow(/CRC/);
  });
});
