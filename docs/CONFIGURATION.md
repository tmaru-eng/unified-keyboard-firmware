# Configuration model

UKF uses versioned, bounded configuration records. A bridge has four registered
BLE source slots (`0..3`) and one virtual source slot (`4`). Profiles and keymaps are
attached to source slots; the current keymap editor provides a fixed single-layer keymap.

The compatibility profile can convert ANSI US symbols to the host's JIS layout,
map Caps Lock to Control, and swap Alt/GUI. Each source has its own profile and
keymap state. The editor selects a registered BLE slot or the virtual slot, then
reads, edits, and saves only that source's keymap. A change for one source is not
silently copied to another source.

The current keymap wire target is a versioned single-layer keymap with up to 32
rules. The legacy global target remains available for compatibility; layered
actions, transparent keys, Mod-Tap, Tap-Hold, combos, macros, and NKRO are not part
of this contract.

Settings travel through the 32-byte vendor configuration HID protocol using a
length-first transfer, numbered chunks, and an explicit CRC-checked commit. A
partial or invalid transfer is not applied. Source link control is separate:
disconnect and reconnect commands change runtime link permission in RAM and do
not write flash. Pairing removal deletes the bond, name, and source settings via
the existing quiet-boundary persistence path.

ZMK and QMK files are not the runtime format. Future importers may accept a
documented subset, then convert it into UKF's independent versioned model.

- [日本語 configuration](CONFIGURATION.ja.md)
