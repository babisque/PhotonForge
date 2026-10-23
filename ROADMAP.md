# PhotonForge Roadmap

This roadmap orders the work so each phase produces something testable before the
next one starts. The first runnable vertical slice (SVG in, G-code file out)
lands before any code talks to hardware.

Legend: ⚠️ = needs explicit approval per `CLAUDE.md` (new dependency, hardware
motion/laser path, safety limits, or project file format).

---

## Phase 0: Workspace bootstrap

**Goal:** an empty but green workspace.

- Create the Cargo workspace (edition 2024) with the five crates: `pf-core`, `pf-io`,
  `pf-gcode`, `pf-device`, `pf-app`.
- Wire the crate dependency direction:
  `pf-app → {pf-io, pf-gcode, pf-device} → pf-core`. `pf-core` depends on nothing internal.
- Add `rustfmt.toml`, workspace-level clippy lints, and a CI workflow (fmt, clippy `-D warnings`, test)
  for Windows, Linux and macOS.
- ✅ GPL-3.0 `LICENSE` file added. Set `license = "GPL-3.0-or-later"` in the workspace `Cargo.toml`.
- ⚠️ Baseline dependencies to approve: `thiserror` (libs), `anyhow` (app), `eframe`/`egui` (app).
  Golden tests need no dependency (Decision D1).

**Done when:** `cargo check/test/clippy/fmt` all pass, and `cargo run -p pf-app` opens an empty window.

---

## Phase 1: Core geometry (`pf-core`)

**Goal:** the in-memory model everything else builds on. Pure, no I/O.

- Primitives: `Point`, `Vec2`, `Rect`/bounding box, 2D affine `Transform`. All units in mm.
- Paths: segments (line, cubic Bézier, quadratic Bézier, arc), open and closed subpaths.
- Flattening: Bézier/arc → polyline with a configurable tolerance.
- Shapes: rectangle, ellipse, polygon, generic path. Each shape has its own transform.
- Layers: id, display color, mode (`Line`, `Fill`, `Image`), speed, power min/max, passes, enabled/output flags.
- `Document`: ordered layers and the shapes assigned to them.
- `MachineProfile`: work area, origin corner, max S value (`$30`), max speed, with
  **conservative defaults** and validation helpers (`is_inside_work_area`, power clamping).
  ⚠️ The default values themselves need your sign-off and are never relaxed afterwards.

**Done when:** unit tests cover transforms, bounding boxes, flattening tolerance, and the profile validation.

---

## Phase 2: Vector G-code generation (`pf-gcode`)

**Goal:** `Document` + `MachineProfile` → G-code string for line (cut/score) layers.

- Pure function: `generate(&Document, &MachineProfile, &JobOptions) -> Result<String, GcodeError>`.
- GRBL laser-mode output: `G21`, `G90`, `M4` dynamic power (`M3` optional), `S` scaled to `$30`,
  `G0` travel with laser off, `G1` cuts with `F`, multiple passes, `M5` at the end.
- Header/footer templates and a job origin option (absolute / current position / user origin).
- Basic path ordering: nearest-neighbor between subpaths, inner shapes before outer for cuts.
- **Refuse to generate** when any toolpath leaves the work area or power exceeds the profile limits.
- Golden/snapshot tests for every output variant (single line, multi-pass, multi-layer, out-of-bounds error).
- Cite the GRBL laser-mode docs for any power/mode behavior.

**Done when:** golden tests pass and the output for a sample file runs cleanly in a G-code simulator
(e.g. NC Viewer or CAMotics). No hardware involved.

---

## Phase 3: SVG import (`pf-io`)

**Goal:** real files feed the pipeline.

- SVG → `Document` via `usvg` (already in the stack). Resolve viewBox, units, DPI
  (Inkscape 96, Illustrator 72), and nested transforms into mm.
- Map stroke colors to layers, as LightBurn does.
- Handle unsupported content explicitly (text without paths, embedded images): skip with warnings, never silently.
- Fixture-based tests: `tests/fixtures/*.svg` → expected shape count, bounding boxes, layer assignment.
- G-code export to a file (`pf-io`), so the slice is complete end to end.

**Milestone M1:** an integration test that imports an SVG, generates G-code, and matches a golden file.

---

## Phase 4: GUI MVP (`pf-app`)

**Goal:** a usable offline designer. Still no hardware.

- Canvas: pan, zoom, grid, work-area outline, shapes drawn in layer colors.
- Import SVG (file dialog), select / move / scale / rotate shapes.
- Layers panel: mode, speed, power, passes, output toggle.
- Machine profile settings screen (with the defaults from Phase 1).
- Toolpath preview: travel vs. cut lines, estimated job time (the estimate itself lives in `pf-gcode`).
- "Save G-code…" export.
- Undo/redo as a plain command stack in `pf-core`. The app only calls into it.
- ⚠️ Probably needs `rfd` for native file dialogs.

**Milestone M2:** design → preview → export G-code entirely from the GUI.

---

## Phase 5: GRBL connection, read-only (`pf-device`)

**Goal:** talk to the controller **without** moving it or firing the laser.

- ⚠️ `serialport` (already in the stack; confirm adding it).
- Port listing, connect/disconnect, baud selection, welcome-banner detection (`Grbl 1.1x`).
- A `Transport` trait boundary (real serial plus an in-memory mock), the one justified abstraction, used for tests.
- Parsers (pure, heavily unit-tested): status reports `<Idle|MPos:…|FS:…>`, `ok`, `error:N`,
  `ALARM:N`, `$$` settings, `[MSG:…]`. Map codes to messages per the GRBL docs.
- Status polling with `?` and reading `$$` / `$I`.
- GUI: connection panel, live status/position, console log.

**Done when:** the app connects to a real GRBL board and shows live status, and the code contains no motion or laser commands.

---

## Phase 6: Machine control and job streaming (`pf-device`) ⚠️ hardware gate

**Goal:** run jobs on real hardware safely. **Each item is proposed and approved before it is implemented.**

- Streaming using GRBL's character-counting protocol (128-byte RX buffer), with the
  simple send-response mode as a fallback. Tested against the mock transport first.
- Realtime commands: feed hold `!`, resume `~`, soft reset `0x18`, plus `$X` unlock and `$H` homing.
- Jogging (`$J=`), with jog-cancel on key release.
- Frame (bounding-box trace) with the laser **off** by default. Any low-power frame option
  follows the profile's frame-power limit.
- Pre-flight checks before every job: connected, idle, no alarm, job inside work area, laser mode `$32=1`.
- Stop or disconnect always ends with `M5` / reset. A lost connection pauses the UI state, and the job never auto-resumes.

**Milestone M3:** cut a simple SVG on a real machine end to end.

---

## Phase 7: Raster engraving

**Goal:** photos and bitmaps.

- Image import via `image` (`pf-io`) into a grayscale buffer placed in the document.
- Processing in `pf-core`: brightness/contrast/gamma, invert, resize to the line interval (DPI).
- Dithering: threshold, Floyd–Steinberg, Jarvis, Stucki, ordered/Bayer, plus a grayscale (variable-S) mode.
- Scanline G-code in `pf-gcode`: bidirectional, overscan, skipping blank runs with `G0`, `S` per segment.
- Golden tests on tiny synthetic images (e.g. 8×8 gradients).

**Milestone M4:** engrave a photo.

---

## Phase 8: Vector fill and cut quality

- Fill mode: scanline hatching of closed shapes (even-odd / non-zero), line interval, angle, cross-hatch.
- Kerf offset and path offsetting (inward/outward).
- Better ordering: group by layer priority, minimize travel, cut inner-first.
- Tabs/bridges for cuts.
- Golden tests for every fill variant.

---

## Phase 9: Project files ⚠️ format proposal first

**Goal:** save and reopen work.

- Write a format proposal before any code: schema, a `version` field, and the migration strategy (see Decision D2).
- Serialize the document, layers, images (embedded or referenced), and machine profile.
- Round-trip tests plus fixtures for each format version, so future migrations stay tested.
- ⚠️ `serde` plus a format crate.

---

## Phase 10: Editing tools and polish

- Drawing tools: rectangle, ellipse, line, polygon, node editing.
- Text to paths. ⚠️ Needs a font crate (e.g. `ttf-parser`), to be proposed.
- Align/distribute, arrays (grid/circular), boolean ops (union/difference). ⚠️ Possibly a geometry crate.
- Material library (named speed/power presets).
- Multiple machine profiles.

---

## Phase 11: Release

- Packaging for Windows (MSI/zip), Linux (AppImage), and macOS (dmg).
- User docs: first-run setup, GRBL configuration (`$30`, `$32`, `$130/131`), and a safety section.
- Versioning, changelog, and a contributor guide.

---

## Later / out of scope for v1

Camera alignment, rotary attachments, other controllers (Ruida, Marlin, Smoothieware),
DXF/PDF import, and print-and-cut.

---

## Decisions

| ID | Decision | Chosen | Rationale |
|----|----------|--------|-----------|
| D1 | G-code golden tests | Hand-rolled: `.gcode` files in `tests/golden/`, compared with `assert_eq!`. An env var (`PF_UPDATE_GOLDEN=1`) rewrites them | No dependency, and the expected output stays plain and reviewable in diffs. Moving to `insta` later is easy if review gets painful |
| D2 | Project file format | JSON via `serde_json`, with a top-level `version` field | Universal, diff-friendly, and other tools can read it. Still goes through the Phase 9 migration proposal |
| D3 | Default power mode | `M4` dynamic, with a per-layer option to use `M3` constant | `M4` avoids burn marks at corners and speed changes, and `M3` is still there for layers that need constant power |
| D4 | Phase order | As written: offline GUI (4) before hardware (5–6) | Most of the app can be built and tested with no machine attached, and hardware work starts on a stable core |
