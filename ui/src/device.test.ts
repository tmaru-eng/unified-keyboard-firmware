import { describe, expect, it } from 'vitest';
import type { BridgeDevice, BridgeSnapshot, SourceProfile } from './device';
import { SerialDeviceTransport } from './device';

const profile: SourceProfile = {
  id: 'usb-0',
  name: 'USB Keyboard',
  transport: 'USB',
  connected: true,
  usToJis: true,
  capsToCtrl: false,
  swapAltGui: false,
};

const snapshot: BridgeSnapshot = {
  outputReady: true,
  hidOutput: [0, 0, 0, 0, 0, 0, 0, 0],
  eventLog: [],
  sources: [profile],
};

describe('SerialDeviceTransport', () => {
  it('serializes profile saves and scenario requests at the device boundary', async () => {
    const events: string[] = [];
    let releaseSave: (() => void) | undefined;
    const device: BridgeDevice = {
      readSnapshot: async () => structuredClone(snapshot),
      saveProfile: () => new Promise<void>((resolve) => {
        events.push('save:start');
        releaseSave = () => {
          events.push('save:end');
          resolve();
        };
      }),
      runDemoScenario: async () => {
        events.push('scenario');
        return structuredClone(snapshot);
      },
    };
    const transport = new SerialDeviceTransport(device);

    const save = transport.saveProfile(profile);
    const scenario = transport.runDemoScenario('usb-us-at');
    await Promise.resolve();

    expect(events).toEqual(['save:start']);
    releaseSave?.();
    await Promise.all([save, scenario]);
    expect(events).toEqual(['save:start', 'save:end', 'scenario']);
  });

  it('allows the next request to proceed after a failed device operation', async () => {
    const events: string[] = [];
    const device: BridgeDevice = {
      readSnapshot: async () => {
        events.push('read');
        return structuredClone(snapshot);
      },
      saveProfile: async () => {
        events.push('save');
        throw new Error('device unavailable');
      },
      runDemoScenario: async () => structuredClone(snapshot),
    };
    const transport = new SerialDeviceTransport(device);

    const failedSave = transport.saveProfile(profile);
    const laterRead = transport.readSnapshot();

    await expect(failedSave).rejects.toThrow('device unavailable');
    await expect(laterRead).resolves.toEqual(snapshot);
    expect(events).toEqual(['save', 'read']);
  });
});
