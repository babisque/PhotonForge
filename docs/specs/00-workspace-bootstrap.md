# Project Spec: Phase 0: Workspace Bootstrap

## 1. Overview and Goal
* **Purpose:** Create the Cargo workspace, the five crate skeletons, the lint/format configuration,
  and CI. Every later phase builds on this foundation.
* **Success Criteria:**
  * `cargo check`, `cargo test`, `cargo clippy -D warnings`, and `cargo fmt --check` all pass on a clean checkout.
  * `cargo run -p pf-app` opens an empty window titled "PhotonForge".
  * CI runs the same four checks on Windows, Linux, and macOS and passes.
  * The crate dependency direction is enforced by the manifests: `pf-core` has no internal dependencies.

## 2. Tech Stack and Architecture
* **Language:** Rust stable 1.99, edition 2024, workspace `resolver = "3"`.
* **GUI:** `eframe = "0.37"` / `egui = "0.37"` (only in `pf-app`).
* **Errors:** `thiserror = "2.0"` (library crates), `anyhow = "1.0"` (`pf-app` only).
* **CI:** GitHub Actions with `dtolnay/rust-toolchain@stable` and `Swatinem/rust-cache@v2`.
* **Dependency graph:**
  ```
  pf-app ──► pf-io ─────► pf-core
     │  ──► pf-gcode ──► pf-core
     └──► pf-device ──► pf-core
  ```
* **License metadata:** `license = "GPL-3.0-or-later"` in `[workspace.package]`.

## 3. Executable Commands
* **Dev:** `cargo run -p pf-app`
* **Check:** `cargo check --workspace --all-targets`
* **Test:** `cargo test --workspace`
* **Lint:** `cargo clippy --workspace --all-targets -- -D warnings`
* **Format:** `cargo fmt --all` / CI: `cargo fmt --all -- --check`

## 4. Project Structure
* `Cargo.toml`: workspace manifest with `[workspace.package]`, `[workspace.dependencies]`, and `[workspace.lints]`.
* `rustfmt.toml`: formatting rules (`edition = "2024"`, `max_width = 100`).
* `crates/pf-core/src/lib.rs`: empty library with a doc comment that describes its scope.
* `crates/pf-io/src/lib.rs`, `crates/pf-gcode/src/lib.rs`, `crates/pf-device/src/lib.rs`: same.
* `crates/pf-app/src/main.rs`: eframe entry point.
* `crates/pf-app/src/app.rs`: the `PhotonForgeApp` struct that implements `eframe::App`.
* `.github/workflows/ci.yml`: the fmt, clippy, and test matrix.
* `LICENSE` (exists), `ROADMAP.md`, `docs/specs/`.

## 5. Code Style and Conventions
* Every crate inherits `version`, `edition`, `license`, and `lints` from the workspace (`*.workspace = true`).
* Shared dependency versions live only in `[workspace.dependencies]`. Crates reference them with `{ workspace = true }`.
* Workspace lints deny `clippy::unwrap_used` and `clippy::expect_used`, and allow them in tests.
* Example (`Cargo.toml`):
  ```toml
  [workspace]
  resolver = "3"
  members = ["crates/*"]

  [workspace.package]
  version = "0.1.0"
  edition = "2024"
  license = "GPL-3.0-or-later"

  [workspace.lints.clippy]
  unwrap_used = "deny"
  expect_used = "deny"
  ```
* Example (`crates/pf-app/src/main.rs`):
  ```rust
  fn main() -> anyhow::Result<()> {
      let options = eframe::NativeOptions::default();
      eframe::run_native(
          "PhotonForge",
          options,
          Box::new(|_cc| Ok(Box::new(app::PhotonForgeApp::default()))),
      )
      .map_err(|e| anyhow::anyhow!("failed to start GUI: {e}"))
  }
  ```

## 6. Boundaries and Guardrails
* **Always do:** Run fmt, clippy, and test before declaring the phase done. Keep library crates free of GUI and I/O dependencies they don't need.
* **Ask first:** Before adding any dependency beyond `thiserror`, `anyhow`, `eframe`, and `egui`, and before adding extra CI jobs (release builds, coverage).
* **Never do:** Never modify `LICENSE`. Never put business logic in `pf-app`. Never add `unsafe` code.

## 7. Implementation Phases
* **0.1 Workspace manifest:** root `Cargo.toml`, `rustfmt.toml`, workspace lints. Done when `cargo metadata` succeeds.
* **0.2 Library skeletons:** create `pf-core`, `pf-io`, `pf-gcode`, and `pf-device` with doc comments and one placeholder test each. Wire the internal dependencies. Done when `cargo test --workspace` passes.
* **0.3 App skeleton:** `pf-app` with eframe/anyhow and an empty `PhotonForgeApp` that shows a central panel. Done when `cargo run -p pf-app` opens the window.
* **0.4 CI:** `.github/workflows/ci.yml` with an OS matrix running fmt-check, clippy, and test. Done when it passes on the first push.
* **0.5 Docs:** add a short `README.md` (what the project is, how to build it, the license). Done when the phase review is approved.
