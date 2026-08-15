# Unified Keyboard Firmware

- [English README](README.md)

Unified Keyboard Firmware（UKF）は、Rustで作る独立した入力デバイス用ファームウェア基盤です。
最初の製品はSeeed XIAO nRF52840 Senseを使うBLE-to-USB bridgeで、JIS設定のWindowsに接続した
外付けANSI USキーボードを、PC側の常駐processやkeyboard driverなしで変換します。

UKFの設定・実行モデルは独自のversion付き規格です。ZMK/QMKのruntime互換layerではありません。
将来のimporterは、既存設定の文書化されたsubsetをUKF modelへ変換し、未対応fieldを黙って捨てずに
報告します。

現行bridgeはsource別profile、登録BLE slot 4個、仮想source 1個、固定32-rule・単一layer
keymap、diagnostics、browser管理UIに対応します。mouse、trackball、その他のpointing device、
多層action、USB Host、NKRO、ZMK/QMK importは現行releaseの対象外です。

## クイックスタート

[はじめに](docs/GETTING-STARTED.ja.md)で、対象UF2の確認、バックアップ、書き込み、診断、
キーボード接続、bridge設定まで確認できます。実機がない場合も同じページのsimulationでUIと
設定protocolを確認できます。

公開UI: https://tmaru-eng.github.io/unified-keyboard-firmware/

## ドキュメント

- [はじめに](docs/GETTING-STARTED.ja.md)
- [設定モデル](docs/CONFIGURATION.ja.md)
- [アーキテクチャ](docs/ARCHITECTURE.ja.md)
- [セキュリティとデバイス境界](docs/SECURITY.ja.md)
- [English documentation](docs/README.md)

## 開発者向け確認

Rust stable、Node.js 24、npmを使います。主なhost側確認は次の通りです。

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
Set-Location ui
npm ci
npm test
npm run build
```

実機なしでUIを確認する場合は`npm run dev`を実行し、「シミュレーションに接続」を選びます。
simulationは同じ設定protocolを話しますが、BLE、USB、flashへ接続せず、実機へkeystrokeも送りません。
実機WebHIDにはChromeまたはEdgeで利用者の操作が必要です。

## ライセンス

MITまたはApache-2.0のデュアルライセンスです。詳細はrepositoryのlicense fileを参照してください。
