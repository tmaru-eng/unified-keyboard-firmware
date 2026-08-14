# Getting started

## Simulation

```powershell
Set-Location ui
npm ci
npm run dev
```

ブラウザで表示されたURLを開き、「シミュレーションに接続」を押します。シミュレーションは
BLEやUSBを開かず、設定・診断・固定32ルールキーマップのUI導線を確認するためのものです。

## Hardware

実機の設定画面は、ブリッジの設定用HID interfaceをWebHIDで開きます。ChromeまたはEdgeを
使い、ブラウザのデバイス選択でブリッジの設定interfaceを選びます。アプリは設定usageに加え、
許可済みのvendor/product identityを検証します。

実機へ書き込む前に、対象UF2のボードID、S140、Family ID、アドレス範囲、SHA-256、現在の
`CURRENT.UF2`バックアップを確認してください。無線が動作している間にflashをerase/write
してはいけません。実機の詳細な手順は公開配布物のリリースノートで版ごとに示します。

## Configuration

現行の公開UIは、ソースごとのUS→JIS、Caps→Ctrl、Alt/GUI互換設定、登録slot管理、診断、
固定32ルール・単一レイヤーのキーマップ編集を対象とします。未対応のレイヤーやアクションは
画面に表示しません。
