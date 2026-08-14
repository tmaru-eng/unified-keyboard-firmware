# Contributing

Thank you for helping improve Unified Keyboard Firmware.

## Before opening a pull request

- Keep changes focused and include a test for behavior changes.
- Run the Rust, UI, and Python checks listed in the public README.
- Do not include pairing keys, BLE addresses, local paths, browser profiles, or
  hardware validation logs in issues or pull requests.
- Do not flash a physical board as part of an automated test. Hardware writes
  require an explicit owner decision and a verified recovery path.

## Pull requests

Use a short-lived branch such as `feat/...`, `fix/...`, or `docs/...`. Explain
the user-visible change, the safety impact, and the checks that were run. Keep
the default branch releasable; merge only after the required checks are green.

The project currently uses Japanese commit messages and pull-request text for
maintainer changes. English contributions are welcome in source code and public
documentation.
