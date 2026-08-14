# Public architecture overview

入力・変換・出力の意味論は`crates/ukf-core`に集め、BLE、USB、保存、ブラウザはアダプタ
として分離します。

```text
BLE HOGP ─────┐
              ├─ source adapter ─> ukf-core ─> USB HID output
Virtual input ┘                 └> profile/keymap policy

Web UI ─> BridgeDevice ─> WebHID configuration adapter
                       └> in-browser simulation
```

現行のコアは`no_std`で、source identity、プロファイル、固定容量キーマップ、レポート集約、
切断時のキー解放を扱います。nRF52840アダプタはBLE HOGP central、USB HID device、設定HID、
診断、保存を接続します。UIはWebHIDのreport操作を直接扱わず、`BridgeDevice`境界を通します。

公開対象の現在の制約は次の通りです。

- BLE同時接続は現行実装の上限が2本。
- キーマップは固定32ルール・単一レイヤー。
- USB Host、マトリクススキャン、split keyboard、3本以上のBLE、NKRO、mouse、consumer、
  pointing deviceは未対応。
- WebHID設定は設定usageと許可済みvendor/product identityの両方を検証します。
