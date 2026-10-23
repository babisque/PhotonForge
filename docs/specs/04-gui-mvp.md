# Project Spec: Phase 4: GUI MVP (`pf-app`)

## 1. Overview and Goal
* **Purpose:** An offline designer: import SVGs, arrange shapes, set layer parameters,
  preview toolpaths, and export G-code. No hardware.
* **Success Criteria:**
  * **M2:** a user can import an SVG, move/scale/rotate it, edit layer settings, preview, and save G-code entirely from the GUI.
  * Canvas pan/zoom stays at 60 FPS with 10k flattened segments on a mid-range machine.
  * Undo/redo works for every document mutation.
  * `pf-app` holds no business logic. Geometry, generation, estimates, and undo all live in other crates.

## 2. Tech Stack and Architecture
* **Crate:** `pf-app` (binary). Dependencies: `eframe = "0.37"`, `egui = "0.37"`, `anyhow = "1.0"`,
  `rfd = "0.17"` (⚠️ approve: native file dialogs), `pf-core`, `pf-io`, `pf-gcode`.
* **Pattern:** egui immediate mode. `PhotonForgeApp` owns `AppState`, and each panel is a function `fn show_x(ui, &mut AppState)`.
* **Undo/redo:** a `pf-core::history` module with a `Command` enum and a `History { undo, redo }` stack. The app records commands and never mutates the `Document` directly.
* **Coordinate mapping:** `Viewport { offset, zoom }` converts mm (Y-up) ↔ screen px (Y-down). The pure math lives in `pf-core::viewport` so it is testable.

## 3. Executable Commands
* **Dev:** `cargo run -p pf-app`
* **Release build for perf checks:** `cargo run -p pf-app --release`
* **Test:** `cargo test --workspace`
* **Lint:** `cargo clippy --workspace --all-targets -- -D warnings`

## 4. Project Structure
* `crates/pf-app/src/main.rs`: entry point.
* `src/app.rs`: `PhotonForgeApp`, top-level layout (menu bar, side panels, central canvas).
* `src/state.rs`: `AppState { document, history, selection, viewport, profile, job_options, status }`.
* `src/panels/canvas.rs`: drawing, pan/zoom, selection, transform handles.
* `src/panels/layers.rs`: layer list and settings editors.
* `src/panels/machine.rs`: machine profile editor.
* `src/panels/preview.rs`: toolpath preview toggle and time estimate.
* `src/actions.rs`: thin wrappers that call `pf-io`, `pf-gcode`, and `History`, and convert errors to `anyhow`.
* `crates/pf-core/src/history.rs`, `crates/pf-core/src/viewport.rs`: new pure modules with tests.

## 5. Code Style and Conventions
* Panel functions take `&mut AppState` and return nothing. Side effects go through `actions.rs`.
* Errors from actions are shown in a status bar or toast, never with `panic!`.
* Example:
  ```rust
  pub fn show_layers(ui: &mut egui::Ui, state: &mut AppState) {
      for layer in state.document.layers() {
          let mut speed = layer.speed_mm_min;
          if ui.add(egui::DragValue::new(&mut speed).suffix(" mm/min")).changed() {
              state.history.apply(&mut state.document, Command::SetLayerSpeed { id: layer.id, speed });
          }
      }
  }
  ```
* Interaction: the selected shape shows 8 scale handles plus a rotate handle. Shift keeps the aspect ratio. Scrolling zooms around the cursor, and dragging with the middle mouse button (or Space) pans.

## 6. Boundaries and Guardrails
* **Always do:** Put new logic in a library crate with tests first, then call it from the app. Show import warnings to the user.
* **Ask first:** Before adding UI dependencies (icon fonts, `egui_extras` features, docking crates), and before changing the layout significantly.
* **Never do:** Never compute toolpaths, G-code, or geometry inside `pf-app`. Never let the UI edit `MachineProfile` limits beyond the safety maxima. Never block the UI thread on file I/O longer than a frame. Use a background thread for large imports.

## 7. Implementation Phases
* **4.1 Viewport and canvas:** `pf-core::viewport` math with tests. Draw the work-area outline, grid, and shapes, with pan/zoom.
* **4.2 Import:** menu → `rfd` dialog → `pf_io::import_svg` → document, and a warnings dialog.
* **4.3 History:** `pf-core::history` (`Command`, apply/undo/redo) with tests. Ctrl+Z / Ctrl+Y.
* **4.4 Selection and transforms:** hit testing (pure, in `pf-core`), handles, move/scale/rotate via commands.
* **4.5 Layers panel:** mode, speed, power, passes, output toggle, M3/M4.
* **4.6 Machine profile panel:** edit the profile within the safety limits.
* **4.7 Preview and export:** toolpath overlay (cut vs. travel colors), time estimate, "Save G-code…". This is the M2 acceptance walkthrough.
