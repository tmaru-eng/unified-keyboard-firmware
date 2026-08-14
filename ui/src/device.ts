export type Transport = 'USB' | 'Bluetooth' | 'Virtual' | 'Unregistered';
export type DemoScenario = 'usb-us-at' | 'merge-usb-ble' | 'detach-ble';

export interface SourceProfile {
  id: string;
  /** Registration slot; optional for older callers that provide a legacy snapshot. */
  slot?: number;
  name: string;
  transport: Transport;
  connected: boolean;
  /** Source-local lifecycle label from the 32-byte source block. */
  state?: string;
  identityAddress?: string | null;
  irkPresent?: boolean;
  usToJis: boolean;
  capsToCtrl: boolean;
  swapAltGui: boolean;
}

export interface BridgeSnapshot {
  sources: SourceProfile[];
  outputReady: boolean;
  hidOutput: number[];
  eventLog: string[];
}

export interface BridgeDevice {
  readSnapshot(): Promise<BridgeSnapshot>;
  saveProfile(profile: SourceProfile): Promise<void>;
  runDemoScenario(scenario: DemoScenario): Promise<BridgeSnapshot>;
}

function sameSource(left: SourceProfile, right: SourceProfile): boolean {
  if (left.slot !== undefined && right.slot !== undefined) {
    return left.slot === right.slot;
  }
  return left.id === right.id;
}

/** Serializes device I/O so a scenario or profile save cannot race a status read. */
export class SerialDeviceTransport implements BridgeDevice {
  #tail: Promise<void> = Promise.resolve();

  constructor(private readonly device: BridgeDevice) {}

  readSnapshot(): Promise<BridgeSnapshot> {
    return this.run(() => this.device.readSnapshot());
  }

  saveProfile(profile: SourceProfile): Promise<void> {
    return this.run(() => this.device.saveProfile(profile));
  }

  runDemoScenario(scenario: DemoScenario): Promise<BridgeSnapshot> {
    return this.run(() => this.device.runDemoScenario(scenario));
  }

  private run<T>(operation: () => Promise<T>): Promise<T> {
    const result = this.#tail.then(operation, operation);
    this.#tail = result.then(() => undefined, () => undefined);
    return result;
  }
}

/**
 * In-browser simulation only. It produces deterministic boot-keyboard reports
 * without opening WebHID, USB, Bluetooth, or a serial port.
 */
export class MemoryBridgeDevice implements BridgeDevice {
  constructor(private snapshot: BridgeSnapshot = {
    outputReady: true,
    hidOutput: [0, 0, 0, 0, 0, 0, 0, 0],
    eventLog: ['シミュレーターを初期化しました。物理デバイスには接続していません。'],
    sources: [
      { id: 'ble-0', slot: 0, name: 'Bluetooth Keyboard', transport: 'Bluetooth', connected: true, state: 'Connected', identityAddress: 'db:e9:25:13:55:25', irkPresent: true, usToJis: true, capsToCtrl: false, swapAltGui: false },
      { id: 'virtual-4', slot: 4, name: 'Virtual Input', transport: 'Virtual', connected: true, state: 'Connected', usToJis: true, capsToCtrl: false, swapAltGui: false },
    ],
  }) {}

  async readSnapshot(): Promise<BridgeSnapshot> {
    return structuredClone(this.snapshot);
  }

  async saveProfile(profile: SourceProfile): Promise<void> {
    this.snapshot = {
      ...this.snapshot,
      sources: this.snapshot.sources.map((item) => sameSource(item, profile) ? profile : item),
      eventLog: this.appendLog(`${profile.name}: 互換プリセットを更新`),
    };
  }

  async runDemoScenario(scenario: DemoScenario): Promise<BridgeSnapshot> {
    switch (scenario) {
      case 'usb-us-at':
        this.updateSimulation(
          [0x00, 0x00, 0x2f, 0x00, 0x00, 0x00, 0x00, 0x00],
          'USB: Shift + 2 (@) → JIS @ (usage 0x2F)',
          { usbConnected: true },
        );
        break;
      case 'merge-usb-ble':
        this.updateSimulation(
          [0x00, 0x00, 0x04, 0x05, 0x00, 0x00, 0x00, 0x00],
          'USB A + BLE B → 1つのHIDレポートへ統合',
          { usbConnected: true, bleConnected: true },
        );
        break;
      case 'detach-ble':
        this.updateSimulation(
          [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
          'BLE: キーを解放してソースを切断',
          { bleConnected: false },
        );
        break;
    }
    return this.readSnapshot();
  }

  private updateSimulation(
    hidOutput: number[],
    event: string,
    connection: { usbConnected?: boolean; bleConnected?: boolean },
  ): void {
    this.snapshot = {
      ...this.snapshot,
      hidOutput,
      sources: this.snapshot.sources.map((source) => {
        if (source.transport === 'USB' && connection.usbConnected !== undefined) {
          return { ...source, connected: connection.usbConnected };
        }
        if (source.transport === 'Bluetooth' && connection.bleConnected !== undefined) {
          return { ...source, connected: connection.bleConnected };
        }
        return source;
      }),
      eventLog: this.appendLog(event),
    };
  }

  private appendLog(event: string): string[] {
    return [...this.snapshot.eventLog, event].slice(-8);
  }
}
