# Architecture

UKF separates portable input behavior from board, transport, storage, and browser
adapters:

- `ukf-core` contains transport-independent source identity, per-source profiles,
  conversion policy, key state, and report aggregation. It is `no_std` and does
  not depend on a board, radio, USB controller, storage implementation, browser,
  or UI.
- The nRF52840 adapter owns BLE HOGP input, source lifecycle, persistent
  configuration, diagnostics, and USB HID output.
- The Web UI and host tools use the bounded vendor configuration HID protocol.
  React components call the UI application port instead of WebHID directly.

The current bridge data flow is:

```text
BLE HOGP ─────┐
              ├─ source adapter ─> ukf-core ─> USB HID output
Virtual input ┘                 └> profile/keymap policy

Web UI ─> BridgeHardware ─> WebHID configuration adapter
                         └> in-browser simulation
```

The current product is keyboard-first. USB Host input is a separate future
adapter. Mouse, trackball, sensor, and other pointing-device events are part of
the broader input-device direction, but are not promised by the current bridge.

The firmware supports one USB-side keyboard output path, four registered BLE
source slots (`0..3`), one virtual source slot (`4`), and up to two concurrent
BLE links in the current implementation. Profiles and fixed single-layer
keymaps are owned by source slot; disconnecting one source must not release or
overwrite another source's state.

The current limits are:

- fixed 32-rule, single-layer keymaps;
- no USB Host, matrix scanning, split keyboard, NKRO, consumer-control, or
  pointing-device output;
- no runtime ZMK/QMK compatibility. Future importers will translate a documented
  subset into the independent UKF configuration model.

- [日本語 architecture](ARCHITECTURE.ja.md)
