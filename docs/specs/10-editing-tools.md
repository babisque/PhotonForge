# Project Spec: Phase 10: Editing Tools and Polish

## 1. Overview and Goal
* **Purpose:** Make PhotonForge a self-sufficient designer: drawing tools, node editing, text,
  alignment, arrays, booleans, material presets, and multiple machine profiles.
* **Success Criteria:**
  * Users can draw rectangles, ellipses, lines, and polygons, and edit path nodes, without leaving the app.
  * Text renders to vector paths from system or bundled TTF/OTF fonts.
  * Align/distribute, grid/circular arrays, and union/difference/intersection all work and are undoable.
  * Material presets (named layer settings) and multiple machine profiles can be created, selected, and persisted.
  * All new geometry logic lives in `pf-core` with tests.

## 2. Tech Stack and Architecture
* **New dependencies (⚠️ approve each one):**
  * `ttf-parser = "0.25"`: glyph outlines → `pf_core::Path`.
  * `i_overlay = "9.1"`: boolean ops (shared with Phase 8 Option A).
  * Font discovery (`fontdb`) is optional. Alternatively, bundle one OFL-licensed font and let the user pick font files.
* **Tools in the GUI:** a `Tool` enum (`Select`, `Rect`, `Ellipse`, `Line`, `Polygon`, `Node`, `Text`) in `AppState`. Each tool is a function that handles canvas input and emits `Command`s.
* **Persistence:** presets and profiles are stored in a user config dir as JSON (separate from project files). ⚠️ Reading the config dir needs `dirs` or a similar crate, to be proposed.

## 3. Executable Commands
* **Test:** `cargo test -p pf-core text boolean align array`
* **Dev:** `cargo run -p pf-app`
* **Workspace:** `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`

## 4. Project Structure
* `crates/pf-core/src/edit/align.rs`: align/distribute functions.
* `src/edit/array.rs`: grid and circular arrays.
* `src/edit/boolean.rs`: union/difference/intersection wrappers.
* `src/edit/nodes.rs`: node insert/delete/move, and segment type changes.
* `src/text.rs`: `text_to_paths(font_bytes, text, size_mm, options)`.
* `src/presets.rs`: `MaterialPreset`, `apply_preset(layer, preset)`.
* `crates/pf-io/src/config.rs`: load/save presets and machine profiles.
* `crates/pf-app/src/tools/*.rs`: one file per tool.
* `crates/pf-app/src/panels/presets.rs`, `panels/profiles.rs`: management UIs.

## 5. Code Style and Conventions
* Every edit operation is a pure function that returns new shapes, wrapped in a `Command` for undo:
  ```rust
  pub fn align(shapes: &[Shape], mode: AlignMode) -> Vec<(ShapeId, Transform)> {
      // returns per-shape transforms; History applies them as one undoable Command::Batch
  }
  ```
* Tools never mutate the document directly. They produce `Command`s.

## 6. Boundaries and Guardrails
* **Always do:** Make every operation undoable. Test the geometry with fixtures. Keep tool input handling separate from the geometry math.
* **Ask first:** Before each new dependency, before bundling a font (license check), and before changing how presets interact with layer settings.
* **Never do:** Never let a preset or profile exceed the safety maxima. Clamp the values and warn. Never load system fonts on the UI thread at startup. Never store presets inside project files without a format proposal (Phase 9 rules).

## 7. Implementation Phases
* **10.1 Drawing tools:** the `Tool` enum, plus rect/ellipse/line/polygon tools.
* **10.2 Node editing:** node operations in `pf-core` with tests, and the node tool.
* **10.3 Align/distribute and arrays:** pure functions, tests, toolbar UI.
* **10.4 Boolean ops:** ⚠️ `i_overlay`. Union/difference/intersection with fixture tests.
* **10.5 Text:** ⚠️ `ttf-parser`. Text-to-path, text tool, font selection.
* **10.6 Material presets:** model, persistence, UI.
* **10.7 Multiple machine profiles:** persistence, selection, per-profile safety clamping.
