# Project Spec: Phase 1: Core Geometry (`pf-core`)

## 1. Overview and Goal
* **Purpose:** Build the pure in-memory model (geometry, shapes, layers, document, machine profile) that import, G-code generation, and the GUI all depend on.
* **Success Criteria:**
  * Shapes can be built, transformed, and flattened to polylines within a given tolerance.
  * A `Document` holds ordered layers and the shapes assigned to them.
  * `MachineProfile` validates work-area bounds and clamps power to its limits.
  * Unit tests cover transforms, bounding boxes, flattening error bounds, and profile validation.
  * The crate has no I/O, no GUI, and no dependencies other than `thiserror`.

## 2. Tech Stack and Architecture
* **Crate:** `pf-core` (library). Dependencies: `thiserror = "2.0"` only.
* **Numeric type:** `f64` everywhere. Internal unit is **millimeters**. Angles are radians internally and degrees only at UI boundaries.
* **Coordinate system:** Y-up, origin at the machine origin (bottom-left by default). Importers and the GUI convert at their boundaries.
* **Architecture:** plain data structs, enums for variants (`Segment`, `LayerMode`), and free functions or inherent methods. No trait hierarchies.

## 3. Executable Commands
* **Test this crate:** `cargo test -p pf-core`
* **Lint:** `cargo clippy -p pf-core --all-targets -- -D warnings`
* **Workspace check:** `cargo test --workspace`

## 4. Project Structure
* `crates/pf-core/src/lib.rs`: module declarations and re-exports.
* `src/geometry.rs`: `Point`, `Vec2`, `Rect` (AABB), approximate equality helpers.
* `src/transform.rs`: `Transform` (2D affine, 2×3 matrix), composition, inverse, `apply_point`.
* `src/path.rs`: `Segment` (Line, QuadBezier, CubicBezier, Arc), `SubPath` (open/closed), `Path`.
* `src/flatten.rs`: segment/path → `Vec<Point>` with an adaptive tolerance.
* `src/shape.rs`: `Shape { id, kind: ShapeKind, transform, layer_id }`. `ShapeKind` is Rect, Ellipse, Polygon, or Path.
* `src/layer.rs`: `Layer`, `LayerId`, `LayerMode` (Line, Fill, Image), `PowerMode` (M4 Dynamic, M3 Constant).
* `src/document.rs`: `Document` with layers, shapes, and add/remove/assign helpers.
* `src/machine.rs`: `MachineProfile`, `Origin`, validation functions, `MachineError`.
* `src/units.rs`: mm/inch/px conversion helpers.

## 5. Code Style and Conventions
* Each struct gets `#[derive(Debug, Clone, PartialEq)]`, plus `Copy` for small value types.
* Constructors validate input and return `Result` when the input can be invalid (for example a negative work area).
* Floating-point comparisons in tests use an epsilon helper, never `==`.
* Example:
  ```rust
  /// Machine limits. Defaults are deliberately conservative.
  #[derive(Debug, Clone, PartialEq)]
  pub struct MachineProfile {
      pub work_area: Rect,      // mm
      pub max_s_value: u32,     // GRBL $30
      pub max_feed_mm_min: f64,
      pub max_power_percent: f64,
      pub frame_power_percent: f64,
  }

  pub fn is_inside_work_area(profile: &MachineProfile, bounds: &Rect) -> bool {
      profile.work_area.contains_rect(bounds)
  }
  ```
* **Proposed safety defaults** (needs sign-off, and never relaxed once approved): work area 300×200 mm,
  `max_s_value` 1000, `max_power_percent` 100, `frame_power_percent` 0 (laser off), `max_feed_mm_min` 6000.

## 6. Boundaries and Guardrails
* **Always do:** Test every public function. Document units in field comments. Keep functions pure.
* **Ask first:** Before changing `MachineProfile` defaults, before adding a geometry dependency (for example `kurbo`), and before renaming any public type.
* **Never do:** Never add I/O, serial, or GUI code to `pf-core`. Never relax a safety default. Never use `unwrap()` outside tests.

## 7. Implementation Phases
* **1.1 Primitives:** `Point`, `Vec2`, `Rect`, and unit helpers, with tests.
* **1.2 Transforms:** `Transform` with translate/scale/rotate/compose/inverse, tested against known matrices and round-trips.
* **1.3 Paths:** `Segment`, `SubPath`, `Path`, and bounding boxes (Bézier bounds computed from curve extrema).
* **1.4 Flattening:** adaptive subdivision. Tests check that the maximum deviation is ≤ the tolerance for curves and arcs.
* **1.5 Shapes and layers:** `ShapeKind` → `Path` conversion, `Layer` defaults, `LayerMode`, `PowerMode`.
* **1.6 Document:** add/remove shapes, layer ordering, shape → layer assignment, document bounds.
* **1.7 Machine profile:** the struct, the proposed defaults (⚠️ sign-off), `is_inside_work_area`, `clamp_power`, `MachineError`.
