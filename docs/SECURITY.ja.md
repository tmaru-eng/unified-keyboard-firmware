# セキュリティとデバイス境界

- PC側に常駐プロセス、入力フック、キーボードドライバを要求しません。
- WebHIDは設定用usageに加えて、許可されたbridge vendor/product identityだけを受け入れます。
- 複数候補、未知のdevice identity、identity欠落は推測で選択しません。
- pairing modeは明示的な操作で開き、通常の再接続では既存のbondだけを対象にします。
- 設定HIDのACKはUSB transportの受理を示すだけで、無線側のflash永続化完了を示しません。
- BLE動作中のflash erase/writeは行わず、設定は安全な静穏境界へ保留します。

これは公開配布物の設計説明であり、特定の脅威モデルに対する認証やセキュリティ保証では
ありません。問題を見つけた場合は、公開issueへ秘密情報やpairing keyを貼らず、プロジェクト
の報告窓口へ連絡してください。
