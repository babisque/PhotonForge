# Project Spec: Phase 8: Vector Fill and Cut Quality

## 1. Overview and Goal
* **Purpose:** Engrave filled vector shapes (hatching) and improve cut quality with kerf
  offsets, better ordering, and tabs.
* **Success Criteria:**
  * Fill layers hatch closed shapes with a configurable interval, angle, and cross-hatch. Holes stay unfilled (even-odd and non-zero rules).
  * Kerf offset moves cut paths inward/outward by a given distance without self-intersections on test fixtures.
  * Travel distance on the ordering benchmark fixture drops by ≥ 30% compared with Phase 2 nearest-neighbor.
  * Tabs leave uncut bridges of a configured width and count.
  * Every feature has golden tests.

## 2. Tech Stack and Architecture
* **Crates:** `pf-core` (polygon ops, hatching, offsets), `pf-gcode` (fill emission, ordering, tabs).
* **Geometry dependency (Decision):**
  * **Option A (recommended):** `i_overlay = "9.1"` (⚠️ approve) for robust offsets and boolean ops, later reused in Phase 10.
  * **Option B:** hand-rolled scanline hatching (no dependency) with offsets limited to simple polygons.
  * Hatching itself is always hand-rolled: intersect horizontal lines with polygon edges, then sort the crossings.
* **Ordering:** 2-opt improvement on top of nearest neighbor, bounded by an iteration budget so it stays deterministic.

## 3. Executable Commands
* **Test:** `cargo test -p pf-core fill offset && cargo test -p pf-gcode fill order tabs`
* **Update goldens:** `PF_UPDATE_GOLDEN=1 cargo test -p pf-gcode`
* **Ordering benchmark (manual):** `cargo test -p pf-gcode --release order_benchmark -- --ignored --nocapture`

## 4. Project Structure
* `crates/pf-core/src/fill.rs`: `hatch(polygons, FillSettings) -> Vec<LineSegment>`, `FillRule`.
* `crates/pf-core/src/offset.rs`: `offset_path(path, distance) -> Result<Vec<Path>, GeometryError>`.
* `crates/pf-gcode/src/fill.rs`: fill toolpaths (bidirectional rows, overscan shared with raster).
* `crates/pf-gcode/src/order.rs`: extended with 2-opt and layer priority.
* `crates/pf-gcode/src/tabs.rs`: splits closed cut paths at tab positions.
* `crates/pf-core/src/layer.rs`: adds `FillSettings`, `kerf_mm`, and `TabSettings` to `Layer`.
* `crates/pf-app/src/panels/layers.rs`: fill, kerf, and tab controls.

## 5. Code Style and Conventions
* Sort hatch crossings by x with `f64::total_cmp`, never with `partial_cmp().unwrap()`.
* Example:
  ```rust
  pub struct FillSettings {
      pub interval_mm: f64,   // > 0, validated
      pub angle_deg: f64,
      pub cross_hatch: bool,
      pub rule: FillRule,     // EvenOdd | NonZero
  }

  pub fn hatch(polys: &[Vec<Point>], s: &FillSettings) -> Result<Vec<LineSegment>, GeometryError> {
      // rotate by -angle, scan horizontally, rotate the segments back by +angle
  }
  ```

## 6. Boundaries and Guardrails
* **Always do:** Add fixtures for nasty geometry (self-intersections, tiny holes, collinear edges). Keep the ordering deterministic.
* **Ask first:** Before adding `i_overlay` or any other geometry crate, and before changing the default ordering for existing goldens.
* **Never do:** Never let a kerf offset move a path outside the work area without validation. Never make tabs the default (opt-in only). Never use randomized ordering heuristics.

## 7. Implementation Phases
* **8.1 Hatching:** scanline polygon fill with fill rules and angle/cross-hatch. Unit tests.
* **8.2 Fill G-code:** fill layer emission with bidirectional rows and overscan. Goldens.
* **8.3 Offsets:** kerf offset (Option A or B per the decision). Fixture tests.
* **8.4 Ordering:** 2-opt and layer priority. Benchmark fixture, goldens.
* **8.5 Tabs:** tab splitting. Goldens.
* **8.6 GUI:** layer panel controls and preview of fill and tabs.
