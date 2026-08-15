# Security and device boundaries

The bridge handles keystrokes and pairing material, so configuration access is
deliberately narrow. The Web UI accepts only the expected vendor configuration HID
interface and validates report identity, length, version, reserved bytes, and CRC.

The bridge does not require a resident PC process, input hook, or keyboard driver.
Pairing is explicit. Do not leave the pairing window open, and do not publish BLE
addresses, IRKs, pairing keys, private logs, host identifiers, or local paths in
Issues, bug reports, or public documentation.

The simulation is not a security boundary and does not connect to a physical
keyboard. It must be clearly labeled and is shown only after an explicit user
action or a demo scenario.

Configuration HID acknowledgements indicate transport acceptance, not necessarily
that a wireless-side flash write has completed. The firmware defers persistent
changes until a safe radio boundary and must not erase or write flash while a BLE
link is active.

Report a security problem privately using the project's private repository process.
Do not put pairing material or hardware logs in a public issue. See [日本語 security
guidance](SECURITY.ja.md).
