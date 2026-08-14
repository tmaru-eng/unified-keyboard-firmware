# Unified Keyboard Firmware

Rustで実装した、ANSI US配列BLEキーボードをJIS配列設定のWindowsで使うための
BLE-to-USB bridge prototypeです。変換はSeeed XIAO nRF52840 Sense側で行い、
PC側に常駐ソフトやドライバを置きません。

この公開ツリーは、コア・ファームウェア・Web UI・テストを確認できる配布用snapshotです。
対応範囲と未対応範囲はリリースごとに明記します。現時点ではZMK/QMK形式との互換や、
多層アクション、USB Host、NKRO、マウス、トラックボールを約束しません。

## Quick start

前提はRust stable、Node.js 24、npmです。ホスト側の確認は次の通りです。

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
Set-Location ui
npm ci
npm test
npm run build
```

実機なしでUIを確認する場合:

```powershell
npm run dev
```

画面上の「シミュレーションに接続」から、実機と同じ設定プロトコルを使うシミュレーションを
起動できます。実機WebHIDはChromeまたはEdgeのユーザー操作が必要です。

## Documents

- [Getting started](docs/GETTING-STARTED.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Security and device boundaries](docs/SECURITY.md)

## Project policies

- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## License

MITまたはApache-2.0のデュアルライセンスです。詳細はリポジトリのライセンスファイルを
参照してください。
