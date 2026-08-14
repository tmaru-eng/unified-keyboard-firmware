import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { ConfigHidDevice } from './webhid';
import type { BridgeHardware } from './useBridgeHardware';
import { VirtualKeyboard } from './VirtualKeyboard';

const { injectSourceReport } = vi.hoisted(() => ({
  injectSourceReport: vi.fn(async () => undefined),
}));

vi.mock('./webhid', () => ({
  injectSourceReport,
}));

afterEach(() => {
  cleanup();
  injectSourceReport.mockClear();
});

function hardware(device: ConfigHidDevice | null): BridgeHardware {
  return {
    device,
    status: null,
    security: null,
    sources: [],
    connecting: false,
    reading: false,
    error: null,
    keymap: null,
    connect: vi.fn(async () => undefined),
    connectSimulated: vi.fn(async () => undefined),
    disconnect: vi.fn(),
    refresh: vi.fn(async () => undefined),
    writeSourceProfile: vi.fn(async () => undefined),
    readKeymap: vi.fn(async () => []),
    writeKeymap: vi.fn(async () => undefined),
    renameBond: vi.fn(async () => undefined),
    deleteBond: vi.fn(async () => undefined),
    simulated: false,
    setPairingOpen: vi.fn(async () => undefined),
    setRequirePasskey: vi.fn(async () => undefined),
  };
}

describe('仮想キーボード', () => {
  it('sends a virtual-source press followed by a release', async () => {
    const device = {
      opened: true,
      collections: [],
      open: vi.fn(async () => undefined),
      sendFeatureReport: vi.fn(async () => undefined),
      receiveFeatureReport: vi.fn(),
    } as unknown as ConfigHidDevice;
    render(<VirtualKeyboard hardware={hardware(device)} />);

    fireEvent.click(screen.getByRole('button', { name: /^2$/ }));

    await waitFor(() => expect(injectSourceReport).toHaveBeenCalledTimes(2));
    expect(injectSourceReport).toHaveBeenNthCalledWith(1, device, 0, [0x1f]);
    expect(injectSourceReport).toHaveBeenNthCalledWith(2, device, 0, []);
  });

  it('reports that a bridge is needed instead of injecting without one', async () => {
    render(<VirtualKeyboard hardware={hardware(null)} />);

    fireEvent.click(screen.getByRole('button', { name: /^2$/ }));

    expect(await screen.findByRole('alert')).toHaveTextContent('先にブリッジへ接続してください');
    expect(injectSourceReport).not.toHaveBeenCalled();
  });
});
