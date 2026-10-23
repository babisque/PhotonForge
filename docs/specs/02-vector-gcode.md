# Project Spec: Phase 2: Vector G-code Generation (`pf-gcode`)

## 1. Overview and Goal
* **Purpose:** Turn a `Document` plus a `MachineProfile` into GRBL-compatible G-code for
  line (cut/score) layers, as pure, deterministic functions.
* **Success Criteria:**
  * `generate()` produces byte-identical output for identical input.
  * The output uses GRBL laser mode correctly (`M4`/`M3`, `S` scaled to `$30`, laser off on `G0`).
  * Generation **fails** with a typed error if any toolpath leaves the work area or a power value exceeds the profile limits.
  * Every output variant has a golden test in `crates/pf-gcode/tests/golden/`.
  * Sample output runs cleanly in NC Viewer or CAMotics.

## 2. Tech Stack and Architecture
* **Crate:** `pf-gcode`. Dependencies: `pf-core`, `thiserror = "2.0"`.
* **Pipeline (pure functions):**
  ```
  Document ──► plan_toolpaths() ──► Vec<Toolpath> ──► order_toolpaths() ──► validate() ──► emit() ──► String
  ```
* **Reference:** GRBL v1.1 laser mode docs (`doc/markdown/laser_mode.md` in gnea/grbl). In laser mode,
  `G0` motions turn the laser off automatically, and `M4` scales power with the actual speed.
* **Number formatting:** coordinates use 3 decimals and trailing zeros are trimmed (`X10.5`, not `X10.500`). `F` and `S` are integers.

## 3. Executable Commands
* **Test:** `cargo test -p pf-gcode`
* **Update goldens:** `PF_UPDATE_GOLDEN=1 cargo test -p pf-gcode`, then review the diff before committing.
* **Lint:** `cargo clippy -p pf-gcode --all-targets -- -D warnings`

## 4. Project Structure
* `src/lib.rs`: the public `generate(&Document, &MachineProfile, &JobOptions) -> Result<String, GcodeError>`.
* `src/toolpath.rs`: `Toolpath { layer_id, points, closed, speed, power, passes }` and `plan_toolpaths`.
* `src/order.rs`: nearest-neighbor ordering, plus inner-before-outer for cuts.
* `src/validate.rs`: bounds and power checks that return `GcodeError`.
* `src/emit.rs`: G-code text emission, header/footer, number formatting.
* `src/options.rs`: `JobOptions` (origin mode, header/footer templates, flatten tolerance).
* `src/estimate.rs`: job time estimate (distance / feed, with travel at max feed).
* `tests/golden.rs`: golden test runner. `tests/golden/*.gcode` holds the expected outputs.
* `tests/support/mod.rs`: the `assert_golden(name, actual)` helper.

## 5. Code Style and Conventions
* There is no hidden state. Everything comes in through arguments.
* Errors carry context: `GcodeError::OutOfBounds { layer: LayerId, bounds: Rect }`.
* Example (golden helper):
  ```rust
  pub fn assert_golden(name: &str, actual: &str) {
      let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name);
      if std::env::var_os("PF_UPDATE_GOLDEN").is_some() {
          std::fs::write(&path, actual).unwrap();
          return;
      }
      let expected = std::fs::read_to_string(&path).unwrap();
      assert_eq!(expected, actual, "golden mismatch: {name}");
  }
  ```
* Expected output shape:
  ```gcode
  ; PhotonForge job
  G21 G90
  M4 S0
  G0 X10 Y10
  G1 X60 Y10 F1200 S500
  G1 X60 Y40
  M5
  ```

## 6. Boundaries and Guardrails
* **Always do:** Add or update a golden file for every output change and explain the diff. Validate before emitting.
* **Ask first:** Before changing output structure that existing goldens depend on, changing the default power mode, or adding any dependency.
* **Never do:** Never emit G-code for anything outside the work area. Never exceed `max_power_percent`. Never emit `M3`/`M4` with `S` > 0 before the first `G0` to the start point. Never use non-deterministic iteration order (e.g. `HashMap`) in the output path.

## 7. Implementation Phases
* **2.1 Emission primitives:** number formatting, the `G0`/`G1`/`M4`/`M5` writer, header/footer. Unit tests.
* **2.2 Toolpath planning:** flatten line-mode shapes into `Toolpath`s with the layer's speed, power, and passes. Golden test: single rectangle.
* **2.3 Validation:** bounds and power checks with typed errors. Tests: out-of-bounds and over-power cases are rejected.
* **2.4 Multi-layer and passes:** layer order, passes, `M3` per-layer override. Goldens: multi-pass, multi-layer, M3 layer.
* **2.5 Ordering:** nearest-neighbor ordering and inner-before-outer. Golden: nested shapes.
* **2.6 Job options and estimate:** origin modes, custom header/footer, time estimate. Goldens and unit tests.
* **2.7 Simulator check:** manually load sample output into NC Viewer and record the result in the PR.
