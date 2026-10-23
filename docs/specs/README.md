# PhotonForge Specs

There is one spec per roadmap phase (see [`ROADMAP.md`](../../ROADMAP.md)). Each spec
follows the same template: overview, stack, commands, structure, conventions,
guardrails, and implementation phases.

| Phase | Spec | Milestone |
|-------|------|-----------|
| 0 | [Workspace bootstrap](00-workspace-bootstrap.md) | Green empty workspace |
| 1 | [Core geometry](01-core-geometry.md) | |
| 2 | [Vector G-code generation](02-vector-gcode.md) | |
| 3 | [SVG import and G-code export](03-svg-import.md) | **M1:** SVG → G-code golden test |
| 4 | [GUI MVP](04-gui-mvp.md) | **M2:** design → preview → export in the GUI |
| 5 | [GRBL connection, read-only](05-grbl-readonly.md) | |
| 6 | [Machine control and streaming](06-machine-control.md) ⚠️ | **M3:** first real cut |
| 7 | [Raster engraving](07-raster-engraving.md) | **M4:** engrave a photo |
| 8 | [Vector fill and cut quality](08-vector-fill.md) | |
| 9 | [Project files](09-project-files.md) ⚠️ | |
| 10 | [Editing tools and polish](10-editing-tools.md) | |
| 11 | [Release](11-release.md) | v1.0 |

## Shared baseline (applies to every spec)

- **Toolchain:** Rust stable 1.99, edition 2024, Cargo workspace (resolver 3).
- **Commands:**
  - Dev: `cargo run -p pf-app`
  - Check: `cargo check --workspace --all-targets`
  - Test: `cargo test --workspace`
  - Lint: `cargo clippy --workspace --all-targets -- -D warnings`
  - Format: `cargo fmt --all` (CI uses `cargo fmt --all -- --check`)
  - Update golden files: `PF_UPDATE_GOLDEN=1 cargo test --workspace`
- **Crate versions:** the versions listed in each spec were the latest on crates.io on
  2026-10-23. Every new dependency still needs approval before it goes into a `Cargo.toml`.
- **Error handling:** `thiserror` in library crates, `anyhow` in `pf-app`. No `unwrap()`/`expect()` outside tests.
- **Off-limits without explicit approval:** hardware motion/laser code paths, safety limit
  defaults, the project file format, new dependencies, and `LICENSE`.
