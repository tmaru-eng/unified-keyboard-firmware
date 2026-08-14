import { describe, expect, it, vi } from 'vitest';

import {
  KEYMAP_KIND,
  KEYMAP_REPORT_VERSION,
  SECURITY_KIND,
  STATUS_KIND,
  UKF_SELECT_KEYMAP,
  UKF_SELECT_PANIC_CHUNK,
} from './diagnostics';
import {
  CONFIG_PROTOCOL_VERSION,
  CONFIG_REPORT_ID,
  CONFIG_REPORT_LEN,
  buildInjectionPayload,
  buildSourceReport,
  crc32Ieee,
} from './injection';
import {
  type ConfigHidDevice,
  injectSourceReport,
  isApprovedConfigInterface,
  isConfigInterface,
  readSecurity,
  readKeymap,
  readStatus,
  selectConfigInterface,
} from './webhid';

/** Builds a block the way the firmware's encoders do, CRC included. */
function block(kind: number, fill: (bytes: Uint8Array, view: DataView) => void): Uint8Array {
  const bytes = new Uint8Array(CONFIG_REPORT_LEN);
  const view = new DataView(bytes.buffer);
  bytes[0] = CONFIG_PROTOCOL_VERSION;
  bytes[1] = kind;
  fill(bytes, view);
  view.setUint32(28, crc32Ieee(bytes.subarray(0, 28)), true);
  return bytes;
}

type FakeDevice = ConfigHidDevice & {
  sent: Array<{ reportId: number; data: Uint8Array }>;
  reply: Uint8Array;
};

const makeDevice = (
  collections: Array<{ usagePage?: number; usage?: number }>,
  opened = true,
  identity: { vendorId?: number; productId?: number } = { vendorId: 0x1209, productId: 0x0001 },
): FakeDevice => {
  const sent: Array<{ reportId: number; data: Uint8Array }> = [];
  const device = {
    opened,
    collections,
    ...identity,
    reply: new Uint8Array(CONFIG_REPORT_LEN),
    open: vi.fn(async () => undefined),
    sendFeatureReport: vi.fn(async (reportId: number, data: BufferSource) => {
      sent.push({ reportId, data: new Uint8Array(data as ArrayBufferView['buffer']) });
    }),
    receiveFeatureReport: vi.fn(async () => {
      const reply = (device as FakeDevice).reply;
      return new DataView(reply.buffer, reply.byteOffset, reply.byteLength);
    }),
    sent,
  } as unknown as FakeDevice;
  return device;
};

const CONFIG_COLLECTION = { usagePage: 0xff00, usage: 0x20 };
const KEYBOARD_COLLECTION = { usagePage: 0x01, usage: 0x06 };

describe('isConfigInterface', () => {
  it('accepts only the vendor configuration collection', () => {
    expect(isConfigInterface(makeDevice([CONFIG_COLLECTION]))).toBe(true);
    expect(isConfigInterface(makeDevice([KEYBOARD_COLLECTION]))).toBe(false);
  });

  it('requires both the usage page and the usage to match', () => {
    expect(isConfigInterface(makeDevice([{ usagePage: 0xff00, usage: 0x21 }]))).toBe(false);
    expect(isConfigInterface(makeDevice([{ usagePage: 0xff01, usage: 0x20 }]))).toBe(false);
  });
});

describe('isApprovedConfigInterface', () => {
  it('accepts the current and legacy bridge identities', () => {
    expect(isApprovedConfigInterface(makeDevice([CONFIG_COLLECTION]))).toBe(true);
    expect(isApprovedConfigInterface(makeDevice([CONFIG_COLLECTION], true, { vendorId: 0xcafe, productId: 0xbaf2 }))).toBe(true);
  });

  it('rejects an unknown or incomplete identity even with the config usage', () => {
    expect(isApprovedConfigInterface(makeDevice([CONFIG_COLLECTION], true, { vendorId: 0x1209, productId: 0x0002 }))).toBe(false);
    expect(isApprovedConfigInterface(makeDevice([CONFIG_COLLECTION], true, {}))).toBe(false);
  });
});

describe('selectConfigInterface', () => {
  it('picks the configuration interface out of a composite device', () => {
    const keyboard = makeDevice([KEYBOARD_COLLECTION]);
    const config = makeDevice([CONFIG_COLLECTION]);

    expect(selectConfigInterface([keyboard, config])).toBe(config);
  });

  it('refuses to guess when two boards are attached', () => {
    // Guessing here would send keystrokes to whichever device happened to sort
    // first, on a board the operator was not looking at.
    expect(() => selectConfigInterface([makeDevice([CONFIG_COLLECTION]), makeDevice([CONFIG_COLLECTION])])).toThrow();
  });

  it('refuses when nothing matches', () => {
    expect(() => selectConfigInterface([makeDevice([KEYBOARD_COLLECTION])])).toThrow();
  });

  it('refuses a different vendor product even when its usage looks like the bridge', () => {
    const unknown = makeDevice([CONFIG_COLLECTION], true, { vendorId: 0x1209, productId: 0x0002 });

    expect(() => selectConfigInterface([unknown])).toThrow();
  });
});

describe('injectSourceReport', () => {
  it('sends the payload under the configuration report ID', async () => {
    const device = makeDevice([CONFIG_COLLECTION]);

    await injectSourceReport(device, 0x02, [0x1f]);

    expect(device.sent).toHaveLength(1);
    expect(device.sent[0].reportId).toBe(CONFIG_REPORT_ID);
    expect(Array.from(device.sent[0].data)).toEqual(
      Array.from(buildInjectionPayload(buildSourceReport(0x02, [0x1f]))),
    );
  });

  it('opens a device that is not open yet', async () => {
    const device = makeDevice([CONFIG_COLLECTION], false);

    await injectSourceReport(device, 0, []);

    expect(device.open).toHaveBeenCalledOnce();
  });
});

describe('readStatus', () => {
  it('returns the block the device sent', async () => {
    const device = makeDevice([CONFIG_COLLECTION]);
    device.reply = block(STATUS_KIND, (bytes) => {
      bytes[2] = 5;
      bytes[15] = 3;
    });

    const status = await readStatus(device);

    expect(status.stateLabel).toBe('購読済み');
    expect(status.connectionCount).toBe(3);
  });

  it('reads the status without selecting anything first', async () => {
    // The status block is what the firmware returns by default, and it resets
    // its selector after every read. Sending a selector here would cost a round
    // trip on the interface that also carries the reset command.
    const device = makeDevice([CONFIG_COLLECTION]);
    device.reply = block(STATUS_KIND, () => {});

    await readStatus(device);

    expect(device.sent).toHaveLength(0);
  });

  it('accepts a reply that keeps the report ID in front', async () => {
    const device = makeDevice([CONFIG_COLLECTION]);
    const payload = block(STATUS_KIND, (bytes) => {
      bytes[15] = 9;
    });
    const prefixed = new Uint8Array(CONFIG_REPORT_LEN + 1);
    prefixed[0] = CONFIG_REPORT_ID;
    prefixed.set(payload, 1);
    device.reply = prefixed;

    expect((await readStatus(device)).connectionCount).toBe(9);
  });
});

describe('readSecurity', () => {
  it('selects the security block before reading it', async () => {
    const device = makeDevice([CONFIG_COLLECTION]);
    device.reply = block(SECURITY_KIND, (bytes) => {
      bytes[24] = 2;
    });

    const security = await readSecurity(device);

    expect(device.sent).toHaveLength(1);
    expect(device.sent[0].reportId).toBe(CONFIG_REPORT_ID);
    expect(device.sent[0].data[1]).toBe(UKF_SELECT_PANIC_CHUNK);
    expect(device.sent[0].data[2]).toBe(0xfe);
    expect(security.stuckKeyReleases).toBe(2);
  });

  it('refuses a status block returned in place of the security block', async () => {
    // Selecting and reading are two separate round trips, so the wrong block
    // can come back. Decoding it anyway would report the status counters as
    // pairing state.
    const device = makeDevice([CONFIG_COLLECTION]);
    device.reply = block(STATUS_KIND, () => {});

    await expect(readSecurity(device)).rejects.toThrow(/種別/);
  });
});

describe('readKeymap', () => {
  it('selects and reassembles every keymap chunk before decoding rules', async () => {
    const device = makeDevice([CONFIG_COLLECTION]);
    const payload = Uint8Array.from([
      1, 6,
      0x04, 0, 0x05, 0,
      0x06, 0, 0x07, 0,
      0x08, 0, 0x09, 0,
      0x0a, 0, 0x0b, 0,
      0x0c, 0, 0x0d, 0,
      0x0e, 0, 0x0f, 0,
    ]);
    const blocks = [0, 1].map((chunkIndex) =>
      block(KEYMAP_KIND, (bytes, view) => {
        bytes[2] = KEYMAP_REPORT_VERSION;
        bytes[3] = chunkIndex;
        bytes[4] = 2;
        view.setUint16(5, payload.length, true);
        bytes.set(payload.subarray(chunkIndex * 21, (chunkIndex + 1) * 21), 7);
      }),
    );
    device.sendFeatureReport = vi.fn(async (reportId: number, data: BufferSource) => {
      const bytes = new Uint8Array(data as ArrayBufferView['buffer']);
      device.sent.push({ reportId, data: bytes });
      if (bytes[1] === UKF_SELECT_KEYMAP) {
        device.reply = blocks[bytes[2]];
      }
    });

    const rules = await readKeymap(device);

    expect(rules).toHaveLength(6);
    expect(rules[0]).toEqual({ inputUsage: 0x04, inputShifted: false, outputUsage: 0x05, outputShifted: false });
    expect(device.sent.map(({ data }) => data[2])).toEqual([0, 1]);
  });
});
