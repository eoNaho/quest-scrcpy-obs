# Third-party notices

## scrcpy server

This project bundles `assets/scrcpy-server` (the scrcpy device-side server,
release v4.0) from:

- Genymobile/scrcpy — https://github.com/Genymobile/scrcpy

scrcpy is licensed under the **Apache License, Version 2.0**. The full license
text is available at:

- https://www.apache.org/licenses/LICENSE-2.0
- https://github.com/Genymobile/scrcpy/blob/master/LICENSE

The server jar is redistributed unmodified and is pushed to the device at
runtime to capture the screen. All credit for the scrcpy server (and for the
Meta Quest black-screen fix it contains) goes to the scrcpy authors.

## Noto Emoji

This project bundles `assets/NotoEmoji-Regular.ttf` (monochrome Noto Emoji) so
the UI's emoji glyphs render in egui:

- google/fonts — https://github.com/google/fonts/tree/main/ofl/notoemoji

Licensed under the **SIL Open Font License, Version 1.1** (https://openfontlicense.org).

## Spout2

The `spout2-rs` crate (Windows-only, used for the **📡 OBS** live output)
statically links the Spout2 SDK from:

- leadedge/Spout2 — https://github.com/leadedge/Spout2

Spout2 is licensed under the **BSD 2-Clause License**.

## virtualcam

The `virtualcam` crate (Windows-only, used for the **🎥 Virtual cam** live
output) is licensed under the **GNU Affero General Public License v3.0
(AGPL-3.0)**:

- https://crates.io/crates/virtualcam
- https://www.gnu.org/licenses/agpl-3.0.html

Unlike every other dependency of this project, AGPL-3.0 is a strong copyleft
license: including it means the *distributed binary* of quest-scrcpy carries
AGPL-3.0 obligations (e.g. making complete corresponding source available to
anyone who receives the binary) on top of the project's own MIT license. This
was a deliberate, informed tradeoff to get a real OS-level virtual camera
without authoring and registering a driver from scratch — see the "License"
section of the README.

## Rust crates

Built on the Rust crate ecosystem, including `eframe`/`egui`, `wgpu`,
`windows`, `cpal`, `clap`, `image`, `anyhow`, and `crossbeam-channel`, each
under their respective MIT/Apache-2.0 licenses.
