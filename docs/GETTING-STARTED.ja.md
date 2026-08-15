# はじめに

このページは、公開リリースをXIAO nRF52840 Senseへ書き込み、BLEキーボードを接続し、
Web UIから設定するまでの利用者向け手順です。実機を使わずにUIだけ確認する場合は、最後の
「シミュレーション」を使ってください。

## 1. 用意するもの

- Seeed XIAO nRF52840 Sense
- 対象リリースのUF2ファイル
- ANSI US配列のBLEキーボード
- WebHIDを使えるChromeまたはEdge
- UF2書き込み用のデータ通信対応USBケーブル

書き込み対象のボード、ファームウェア版、ビルド識別子は[リリースページ](https://github.com/tmaru-eng/unified-keyboard-firmware/releases)
と各リリースノートで確認してください。
別のボードや別のUF2を推測で書き込まないでください。

## 2. 現在のUF2を確認してから書き込む

キーボードを切断し、無線が動作していない状態で行います。無線動作中にflashのerase/writeを
行ってはいけません。

まず、対象UF2とバックアップ先を絶対パスで指定して安全確認だけを実行します。

```powershell
Set-Location firmware/nrf52840-ble-usb/tools
.\flash_xiao.ps1 `
  -Uf2Path 'C:\path\to\release.uf2' `
  -BackupDirectory 'C:\path\to\uf2-backups' `
  -WhatIf
```

`-WhatIf`の出力で、XIAO SenseのBoard-ID、S140、UF2範囲、バックアップ先が期待どおりである
ことを確認します。利用者が書き込みを承認した後だけ、同じコマンドから`-WhatIf`を外し、
`-ConfirmFlash`を追加して実行します。

```powershell
.\flash_xiao.ps1 `
  -Uf2Path 'C:\path\to\release.uf2' `
  -BackupDirectory 'C:\path\to\uf2-backups' `
  -ConfirmFlash
```

書き込み後はボードが再列挙するまで待ちます。`CURRENT.UF2`のバックアップは、復旧が必要に
なった場合のために保管してください。

## 3. 診断を読む

```powershell
Set-Location firmware/nrf52840-ble-usb/tools
uv run read_diagnostics.py
```

ファームウェア版、ビルド識別子、USB状態、ボンド状態、ソースslot、プロファイルを確認します。
起動直後のボンド状態は鍵の読み出し前の一時状態の場合があるため、数秒後に再読出しします。

## 4. キーボードを接続する

既にボンド済みのキーボードは、通常は自動再接続を待ちます。新しいキーボードを迎える場合
だけ、次を実行してからキーボード側をペアリング待機にします。

```powershell
uv run pairing_mode.py on
```

接続後、`read_diagnostics.py`で対象slotがConnectedになり、入力元が意図したslotとして表示
されることを確認します。ペアリング済みのキーボードを使うためだけに、ペアリングモードを
有効にする必要はありません。

## 5. Web UIでプロファイルを設定する

公開[Pages](https://tmaru-eng.github.io/unified-keyboard-firmware/)を開き、ChromeまたはEdgeで「実機に接続」を選びます。ブラウザのデバイス選択では、
ブリッジの設定用HID interfaceを選択してください。UIは設定usageだけでなく、許可済みの
vendor/product identityも検証します。

現行版で設定できるものは、入力元ごとのUS→JIS、Caps→Ctrl、Alt/GUI、登録slot管理、診断です。
設定を書き込んだ後、接続中は無線が静穏になるまで保存待ちになることがあります。保存結果は
診断とUIの状態表示で確認し、保存中にflashやリセットを行わないでください。

登録済みsourceの「切断」はlinkだけを切り、ボンドと設定を保持します。「再接続」は探索を
再開しますが、advertisementとGATT設定が完了するまでは接続済みと表示しません。「ペアリング
解除」は必要に応じて先に切断し、ボンド、名前、source設定を削除します。操作対象のslotと
影響範囲を確認してから実行してください。

## 6. キーマップを編集する

現行のキーマップはsourceごとの固定32ルール・単一レイヤーです。Web UIのキーマップ画面で
「編集対象ソース」から登録BLE slotまたは仮想slotを選び、読み込み、キー編集、保存を行います。
未対応のレイヤーやアクションを表示しないため、画面に出ない機能はまだ保存できません。
Pythonでも次のようにslotを明示して確認できます。

```powershell
uv run read_keymap.py --slot 4
uv run write_keymap.py --slot 4 --us-jis
```

保存は接続中にRAMへ適用され、無線が静穏になるまで保留されます。保存待ちの間にflashや
リセットを行わず、UIまたは`read_diagnostics.py`で結果を確認します。

## シミュレーション

実機なしで確認する場合は、リポジトリの`ui`で次を実行します。

```powershell
Set-Location ui
npm ci
npm run dev
```

表示されたURLを開き、「シミュレーションに接続」を押します。シミュレーションはBLE、USB、
flashを開かず、実機と同じ設定プロトコルで診断、プロファイル、固定キーマップのUI導線を
確認します。

## 困ったとき

- 実機が見えない場合は、USBケーブル、HID interface、Chrome/Edgeの選択対象を確認する。
- ボンド済みなのに接続しない場合は、OS側のペアリング状態を確認してから診断を再読出しする。
- 書き込みが必要な場合は、キーボードを切断し、`-WhatIf`、バックアップ、明示承認の順で進める。
- 未対応機能や設定取り込みの要否は、[設定モデル](CONFIGURATION.ja.md)を確認する。
