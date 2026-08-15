# Unified Keyboard Firmware

ANSI US配列のBLEキーボードを、JIS配列設定のWindowsで使うための、独立したRust製
キーボードファームウェア基盤です。現行の最小製品はSeeed XIAO nRF52840 Senseを使う
BLE-to-USBブリッジで、変換はボード側で行い、PC側に常駐ソフトやドライバを置きません。

このプロジェクトの正規の設定モデルと実行モデルは、UKFが定義する独自規格です。
ZMK/QMKのファームウェアを実行する互換層ではありません。将来、ZMK/QMKなどの既存設定を
UKFの設定モデルへ取り込むアダプタを提供する可能性はありますが、対応範囲を明示し、
未対応の意味を黙って捨てない方針です。

ファームウェアを乗り換えるたびに設定資産や操作方法が分断される問題を減らし、
ハードウェア・ファームウェア・Web UIをまたいで設定を持ち運べる共通基盤を目指します。

この公開ツリーは、コア・ファームウェア・Web UI・テストを確認できる配布用snapshotです。
現行ブリッジでは、ソース別プロファイル、登録slot、固定32ルール・単一レイヤーのキーマップ、
診断、Web UIを扱えます。多層アクション、USB Host、NKRO、マウス、トラックボール、
ZMK/QMK設定取り込みは、現行リリースの対応範囲に含めません。

## 利用者向けクイックスタート

まず [Getting started](docs/GETTING-STARTED.md) の手順で、対象UF2の確認、バックアップ、
書き込み、診断、キーボード接続、Web UI設定まで進めてください。実機を持っていない場合は、
同じページのシミュレーション手順でUIと設定モデルを確認できます。

公開UI: https://tmaru-eng.github.io/unified-keyboard-firmware-public/

## 開発者向け確認

Rust stable、Node.js 24、npmを使います。ホスト側の確認は次の通りです。

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
Set-Location ui
npm ci
npm test
npm run build
```

実機なしでUIだけを確認する場合:

```powershell
npm run dev
```

画面上の「シミュレーションに接続」から、実機と同じ設定プロトコルを使うシミュレーションを
起動できます。実機WebHIDはChromeまたはEdgeのユーザー操作が必要です。

## Documents

- [Getting started](docs/GETTING-STARTED.md)
- [Configuration model](docs/CONFIGURATION.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Security and device boundaries](docs/SECURITY.md)

## Project policies

- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## License

MITまたはApache-2.0のデュアルライセンスです。詳細はリポジトリのライセンスファイルを
参照してください。
