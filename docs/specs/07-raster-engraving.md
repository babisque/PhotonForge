# Project Spec: Phase 7: Raster Engraving

## 1. Overview and Goal
* **Purpose:** Import bitmap images, process and dither them, and generate scanline G-code for engraving.
* **Success Criteria:**
  * **M4:** a photo is engraved on a real machine with recognizable tonal detail.
  * PNG, JPEG, BMP, and GIF import, with the physical size taken from DPI metadata or a user-set size.
  * Dithering modes: threshold, Floyd–Steinberg, Jarvis–Judice–Ninke, Stucki, ordered (Bayer 4×4/8×8), and grayscale (variable `S`).
  * Scanline output supports bidirectional travel, overscan, and skipping blank runs with `G0`.
  * Golden tests cover each mode on tiny synthetic images.

## 2. Tech Stack and Architecture
* **Crates:**
  * `pf-io`: `image = "0.25"` (⚠️ approve; only enable the `png`, `jpeg`, `bmp`, and `gif` features).
  * `pf-core`: a `GrayImage` type of its own (`width`, `height`, `Vec<u8>`) and the processing functions. No dependency on `image`.
  * `pf-gcode`: `raster.rs` for scanline emission.
* **Pipeline:**
  ```
  file ──pf-io──► GrayImage ──pf-core: adjust → resample(line interval) → dither──► Bitmap/GrayImage
       ──pf-gcode: scanlines(overscan, bidirectional) ──► G-code
  ```
* **Resampling:** resample to 1 pixel per scan line (`line_interval_mm`, default 0.1 mm = 254 DPI) using area averaging.

## 3. Executable Commands
* **Test:** `cargo test -p pf-core raster && cargo test -p pf-gcode raster`
* **Update goldens:** `PF_UPDATE_GOLDEN=1 cargo test -p pf-gcode`
* **Workspace:** `cargo test --workspace`

## 4. Project Structure
* `crates/pf-core/src/raster/image.rs`: `GrayImage`, `Bitmap`.
* `src/raster/adjust.rs`: brightness, contrast, gamma, invert.
* `src/raster/resample.rs`: area-average resample.
* `src/raster/dither.rs`: `DitherMode` and the error-diffusion kernels as data tables.
* `src/shape.rs`: adds `ShapeKind::Image { image, settings }`.
* `crates/pf-io/src/raster.rs`: `import_image(bytes) -> Result<GrayImage, IoError>`.
* `crates/pf-gcode/src/raster.rs`: `emit_raster(&RasterJob) -> Result<String, GcodeError>`.
* `crates/pf-gcode/tests/golden/raster_*.gcode`: goldens.
* `crates/pf-app/src/panels/image_settings.rs`: GUI controls with a live dither preview.

## 5. Code Style and Conventions
* Error-diffusion kernels are data, not separate functions:
  ```rust
  pub struct Kernel { pub divisor: u16, pub taps: &'static [(i8, i8, u8)] } // (dx, dy, weight)

  pub const FLOYD_STEINBERG: Kernel = Kernel {
      divisor: 16,
      taps: &[(1, 0, 7), (-1, 1, 3), (0, 1, 5), (1, 1, 1)],
  };
  ```
* Use serpentine scanning for error diffusion to reduce directional artifacts.
* Raster G-code: each row turns on with `M4` and changes `S` within one `G1` move per run. Overscan uses `S0` beyond the image edge.

## 6. Boundaries and Guardrails
* **Always do:** Golden-test every dither mode and scan option. Validate the image placement against the work area before emitting.
* **Ask first:** Before adding the `image` dependency or extra features, before adding SIMD or parallel crates (`rayon`), and before changing the default line interval.
* **Never do:** Never exceed the layer's max power or the profile max in grayscale mode. Never put the overscan outside the work area. If it doesn't fit, reduce the overscan and warn. Never block the GUI thread while dithering large images.

## 7. Implementation Phases
* **7.1 Image types and import:** `GrayImage`, `import_image` (luminance conversion, DPI metadata).
* **7.2 Adjustments and resampling:** pure functions with tests.
* **7.3 Dithering:** kernels plus ordered and grayscale modes. Test the output against known small outputs.
* **7.4 Scanline G-code:** unidirectional, then bidirectional, overscan, blank skipping. Goldens.
* **7.5 Document integration:** `ShapeKind::Image`, Image layer mode, bounds/validation.
* **7.6 GUI:** image settings panel with a live preview (background thread).
* **7.7 M4 hardware run:** engrave a test photo and record the settings and result. ⚠️ Uses the Phase 6 hardware path.
