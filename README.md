# Unified Keyboard Firmware

- [日本語 README](README.ja.md)

Unified Keyboard Firmware (UKF) is an independent Rust input-device firmware
platform. Its first product is a BLE-to-USB bridge using the Seeed XIAO
nRF52840 Sense. It converts an external ANSI US keyboard for a Windows host
configured for JIS; conversion runs on the board without a resident PC process
or keyboard driver.

UKF defines its own versioned configuration and execution model. It is not a
runtime compatibility layer for ZMK or QMK. Future importers may translate a
documented subset of existing keyboard configurations into the UKF model, while
reporting unsupported fields instead of silently discarding their meaning.

The project aims to keep configuration portable across hardware, firmware, and
the Web UI. The current bridge supports source-specific profiles, four registered
BLE slots, one virtual source, a fixed 32-rule single-layer keymap, diagnostics,
and a browser-based management UI. Mouse, trackball, and other pointing devices
are future input-device targets; layered actions, USB Host, NKRO, and ZMK/QMK
import are not part of the current release.

## Quick start

Follow [Getting started](docs/GETTING-STARTED.md) to check the matching UF2,
preserve a backup, flash the board, read diagnostics, connect a keyboard, and
configure the bridge. Without hardware, use the simulation instructions on the
same page to inspect the UI and configuration protocol.

Published UI: https://tmaru-eng.github.io/unified-keyboard-firmware/

## Development checks

The project uses Rust stable, Node.js 24, and npm. The main host-side checks are:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
Set-Location ui
npm ci
npm test
npm run build
```

To inspect the UI without hardware:

```powershell
npm run dev
```

Choose **Connect to simulation**. The simulator speaks the same configuration
protocol but does not connect to BLE, USB, or flash and does not send keystrokes
to physical hardware. Physical WebHID use requires a user action in Chrome or
Edge.

## Documents

- [Getting started](docs/GETTING-STARTED.md)
- [Configuration model](docs/CONFIGURATION.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Security and device boundaries](docs/SECURITY.md)
- [日本語 documentation](docs/README.ja.md)

## Project policies

- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## License

The project is dual-licensed under MIT or Apache-2.0. See the license files in
the repository for the applicable terms.
