# Project Spec: Phase 3: SVG Import and G-code Export (`pf-io`)

## 1. Overview and Goal
* **Purpose:** Import SVG files into a `Document` with correct physical dimensions and write generated G-code to disk. This completes the first vertical slice.
* **Success Criteria:**
  * An SVG exported from Inkscape at 100×50 mm imports at 100×50 mm (±0.01 mm).
  * Distinct stroke colors map to distinct layers.
  * Unsupported content produces `ImportWarning`s and is never silently dropped.
  * **M1:** an integration test imports `tests/fixtures/m1_sample.svg`, generates G-code, and matches `tests/golden/m1_sample.gcode`.

## 2. Tech Stack and Architecture
* **Crate:** `pf-io`. Dependencies: `pf-core`, `thiserror = "2.0"`, `usvg = "0.48"` (⚠️ confirm).
* **Dev-dependencies:** `pf-gcode` (for the M1 integration test only).
* **Flow:**
  ```
  bytes ──► usvg::Tree::from_data() ──► walk nodes (absolute transforms) ──► pf_core::Path ──► Document
  ```
* **Units:** `usvg` resolves units to user units (px at 96 DPI). Convert with `mm = px * 25.4 / dpi`.
  The default DPI is 96 and can be overridden through `SvgImportOptions { dpi }` (72 for Illustrator).
* **Y-axis:** SVG is Y-down. Flip into the Y-up document space at import.

## 3. Executable Commands
* **Test:** `cargo test -p pf-io`
* **M1 test only:** `cargo test -p pf-io --test m1_svg_to_gcode`
* **Update goldens:** `PF_UPDATE_GOLDEN=1 cargo test -p pf-io`

## 4. Project Structure
* `src/lib.rs`: the public API: `import_svg`, `export_gcode`.
* `src/svg.rs`: `import_svg(bytes: &[u8], opts: &SvgImportOptions) -> Result<SvgImport, IoError>`, where `SvgImport { document, warnings }`.
* `src/svg_convert.rs`: usvg path → `pf_core::Path` segment conversion.
* `src/color_layers.rs`: stroke color → `LayerId` mapping.
* `src/gcode_file.rs`: `export_gcode(path: &Path, gcode: &str) -> Result<(), IoError>` with an atomic write (temp file + rename).
* `src/error.rs`: `IoError`, `ImportWarning`.
* `tests/fixtures/*.svg`: Inkscape, Illustrator, nested transforms, viewBox scaling, unsupported content.
* `tests/m1_svg_to_gcode.rs`: the M1 integration test.

## 5. Code Style and Conventions
* Keep usvg types inside `svg_convert.rs`. Public functions only expose `pf-core` types.
* Each warning names the element id when there is one.
* Example:
  ```rust
  #[derive(Debug, Clone, PartialEq)]
  pub enum ImportWarning {
      SkippedText { id: Option<String> },
      SkippedImage { id: Option<String> },
      FillOnlyShape { id: Option<String> }, // imported, assigned to a Fill layer
  }
  ```

## 6. Boundaries and Guardrails
* **Always do:** Add a fixture file for every bug fix. Keep the import side pure (bytes in, data out). Only `gcode_file.rs` touches the filesystem.
* **Ask first:** Before adding `usvg` features that pull in text/font support (`resvg`/`fontdb`), and before supporting other formats (DXF, PDF).
* **Never do:** Never guess units silently. If an SVG has no physical size, warn and fall back to the configured DPI. Never overwrite an existing file without the caller choosing to.

## 7. Implementation Phases
* **3.1 Basic import:** rects, ellipses, and paths with absolute transforms → `Document`. Tests on simple fixtures.
* **3.2 Units and DPI:** viewBox, mm/in/px roots, DPI override. Size-accuracy tests.
* **3.3 Color → layer mapping:** stroke color grouping and layer colors. Test on a multi-color fixture.
* **3.4 Warnings:** text, images, and fill-only shapes. Tests check the warning list.
* **3.5 G-code export:** atomic file write. Tests use a temp dir.
* **3.6 M1 integration:** SVG → `generate()` → golden file.
