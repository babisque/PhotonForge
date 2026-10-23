# PhotonForge
Free and open-source (GPL-3.0) laser engraving/cutting software — an alternative to LightBurn.

## Stack
- Rust stable (edition 2024), Cargo workspace
- GUI: egui/eframe
- First supported controller: GRBL (G-code over serial, via `serialport`)
- SVG import via `usvg`; raster images via `image`

## Workspace layout
- `crates/pf-core` — geometry, shapes, layers, units (internal unit: mm). No I/O, no GUI.
- `crates/pf-io` — file import/export (SVG, images, project files).
- `crates/pf-gcode` — toolpath → G-code generation. Pure functions.
- `crates/pf-device` — device communication (GRBL serial, status, streaming).
- `crates/pf-app` — eframe GUI binary. Thin layer over the other crates.

## Run commands
- dev: cargo run -p pf-app
- test: cargo test --workspace
- lint: cargo clippy --workspace --all-targets -- -D warnings
- format: cargo fmt --all
- check: cargo check --workspace

## Conventions
- Prefer small, pure functions and plain data structs over trait hierarchies.
- No complex abstractions — prioritize clarity over brevity.
- Every new feature requires tests (unit tests in the same file, integration tests in `tests/`).
- G-code generation must have snapshot/golden tests for its output.
- Business logic never lives in the GUI crate; the app only calls into the other crates.
- No `unwrap()`/`expect()` outside tests — use `Result` with `thiserror` in libraries and `anyhow` in the app.
- Code, comments, commit messages and docs in English.
- Run `cargo fmt` and `cargo clippy` before considering a task done.

## Off-limits (never touch without explicit instruction)
- Any code path that sends motion or laser-on commands to real hardware — safety first; propose the change and wait for approval.
- Laser safety limits (max power, work area bounds, frame/ping power) — never relax defaults.
- Project file format (serialization) — propose a migration plan first.
- Adding new dependencies — propose the crate and why before adding it.
- `LICENSE`.

## Fallbacks (when in doubt)
- Propose 2 options and ask me to choose.
- Never rename public APIs of a crate without asking.
- If a GRBL behavior is unclear, cite the GRBL docs/source instead of guessing.
