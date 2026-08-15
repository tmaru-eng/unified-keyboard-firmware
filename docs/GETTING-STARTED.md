# Getting started

This guide covers flashing the public firmware, connecting a BLE keyboard, and
using the Web UI. If you only want to inspect the UI, use the simulation section
without a board.

## Hardware

Use a Seeed XIAO nRF52840 Sense, a USB data cable, an ANSI US BLE keyboard, and
Chrome or Edge with WebHID support. The bridge is the USB HID output device and
accepts BLE keyboard input. Keep the keyboard disconnected from the host while
changing firmware or persistent configuration.

## Flashing

Download the UF2 from the matching GitHub Release. Check the board model, firmware
version, and build identifier before writing. Do not flash an assumed board or
artifact.

Keep all BLE keyboards disconnected and make sure no radio link is active. Follow
the release-specific backup and preflight instructions. From the repository root,
run the preflight first:

```powershell
Set-Location firmware/nrf52840-ble-usb/tools
.\flash_xiao.ps1 `
  -Uf2Path 'C:\path\to\release.uf2' `
  -BackupDirectory 'C:\path\to\uf2-backups' `
  -WhatIf
```

Confirm the XIAO Sense Board-ID, S140, UF2 range, and backup path. Only after
explicit owner approval, remove `-WhatIf`, add `-ConfirmFlash`, put the board in
its UF2 bootloader, and run the command. Wait for the board to reboot and keep the
`CURRENT.UF2` backup for recovery.

Never erase or write flash while a BLE link is active. A physical write requires
explicit owner approval and a verified recovery path.

## Diagnostics

Use the host tool to read the firmware version, bond state, source slots, and
profile state:

```powershell
Set-Location firmware/nrf52840-ble-usb/tools
uv run read_diagnostics.py
```

Immediately after boot, bond loading may still be in progress. Read diagnostics
again after a few seconds before concluding that stored keys are absent.

## Connecting a keyboard

Bonded keyboards normally reconnect automatically. Open pairing mode only when
welcoming a new keyboard:

```powershell
uv run pairing_mode.py on
```

After pairing, confirm that the intended source slot is connected in diagnostics.
Do not enable pairing mode merely to reconnect an already bonded keyboard.

## Web UI

Open the published Pages site in Chrome or Edge. Choose **Connect to device** and
select the bridge's configuration HID interface. The UI validates the vendor usage,
report identity, and approved vendor/product identity; selecting an unrelated HID
device is rejected.

For a registered BLE source, **Disconnect** ends only the link and preserves its
bond and settings. **Reconnect** permits discovery again but does not claim that
the keyboard is connected until its advertisement and GATT setup complete.
**Remove pairing** disconnects first when necessary, then removes the bond, name,
and source settings.

The profile screen can change US-to-JIS, Caps-to-Control, and Alt/GUI behavior for
the selected source. Source slots, link state, and the pending-save state must be
visible before applying a change.

## First configuration

1. Read diagnostics and confirm the firmware version.
2. Enable the US-to-JIS profile for the relevant source.
3. Pair a keyboard only when the pairing window is explicitly open.
4. Confirm source state and output in diagnostics.

In the keymap screen, choose a registered BLE slot or the virtual slot, read its
keymap, edit the fixed 32-rule single layer, and save it. The host tool exposes
the same path with:

```powershell
uv run read_keymap.py --slot 4
uv run write_keymap.py --slot 4 --us-jis
```

Settings can be applied to RAM while a source is connected and may remain pending
until the radio reaches a quiet boundary. Confirm the result in the UI or with
`read_diagnostics.py`; do not reset or flash while a save is pending.

## Simulation

From the repository's `ui` directory, run:

```powershell
npm ci
npm run dev
```

Open the displayed URL and choose **Connect to simulation**. The simulation uses
the same 32-byte configuration protocol, but it does not open BLE, USB, or flash
and does not send keystrokes to physical hardware.

## Troubleshooting

- If the bridge is not listed, check the USB data cable and select the configuration
  HID interface, not the keyboard HID interface.
- If a bonded keyboard does not connect, verify the host OS pairing state and read
  diagnostics again after startup settles.
- If a firmware write is needed, disconnect keyboards, run the release preflight,
  preserve a backup, and obtain explicit approval before writing.

- [日本語 getting started](GETTING-STARTED.ja.md)
