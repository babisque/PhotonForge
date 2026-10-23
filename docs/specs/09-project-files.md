# Project Spec: Phase 9: Project Files ⚠️ Format Proposal First

> **No code ships in this phase until sub-phase 9.1 (the format proposal) is approved.**

## 1. Overview and Goal
* **Purpose:** Save and reopen complete work sessions (document, layers, images, machine profile)
  in a versioned, forward-migratable format.
* **Success Criteria:**
  * Save → open round-trips without data loss (structural equality in tests).
  * Every file carries a `format_version`, and older versions load through tested migrations.
  * Files from a newer, unknown version are rejected with a clear error and never partially loaded.
  * A corrupted file fails with a typed error. Nothing panics, and the open document is untouched.

## 2. Tech Stack and Architecture
* **Crates:** `pf-io` (serialization), `pf-core` (derives on model types).
* **Dependencies (⚠️ approve):** `serde = "1.0"` with `derive` (in `pf-core` and `pf-io`), `serde_json = "1.0"` (in `pf-io`).
* **Format (Decision D2):** JSON with the `.pfproj` extension, UTF-8, pretty-printed for readable diffs.
* **Images:** embedded as base64 PNG inside the JSON (single self-contained file). ⚠️ Adding `base64` is a new dependency to approve. The alternative is a zip container, decided in 9.1.
* **Separate DTO layer:** serialized structs (`ProjectFileV1`) are distinct from the `pf-core` model, so model refactors don't break the format.
  ```
  Document ──to_dto──► ProjectFileV1 ──serde_json──► .pfproj
  .pfproj ──parse version──► migrate(vN → vLatest) ──from_dto──► Document
  ```

## 3. Executable Commands
* **Test:** `cargo test -p pf-io project`
* **Round-trip fixtures:** `cargo test -p pf-io --test project_fixtures`
* **Workspace:** `cargo test --workspace`

## 4. Project Structure
* `docs/project-format.md`: the format proposal and spec (written in 9.1).
* `crates/pf-io/src/project/mod.rs`: `save_project`, `load_project`.
* `src/project/v1.rs`: `ProjectFileV1` DTOs and the `to_dto`/`from_dto` conversions.
* `src/project/migrate.rs`: a migration chain `fn migrate(value: serde_json::Value, from: u32) -> Result<Value, ProjectError>`.
* `tests/fixtures/projects/v1_*.pfproj`: frozen fixtures (never edited after release).
* `tests/project_fixtures.rs`: loads every fixture and asserts the expected document.

## 5. Code Style and Conventions
* DTOs use `#[serde(deny_unknown_fields)]` within a version, and every field is explicit (no `#[serde(flatten)]` magic).
* Example:
  ```rust
  #[derive(Serialize, Deserialize)]
  #[serde(deny_unknown_fields)]
  pub struct ProjectFileV1 {
      pub format_version: u32, // always 1 here
      pub app_version: String,
      pub machine: MachineProfileDto,
      pub layers: Vec<LayerDto>,
      pub shapes: Vec<ShapeDto>,
  }
  ```
* Saving is atomic (temp file + rename), the same as G-code export.

## 6. Boundaries and Guardrails
* **Always do:** Add a frozen fixture for every format version. Write a migration plus a test for every version bump.
* **Ask first:** Before any change to the format, a DTO, or the migration code. That includes the 9.1 proposal itself, and the choice of image storage and the `base64`/zip dependency.
* **Never do:** Never edit a released fixture. Never load a file with a newer version. Never let a project file override the safety maxima. A loaded profile is clamped to the limits and the user is warned.

## 7. Implementation Phases
* **9.1 Format proposal (⚠️ approval):** write `docs/project-format.md` covering the schema, versioning, migration strategy, image storage, and safety-limit handling. No code.
* **9.2 DTOs and round-trip:** `ProjectFileV1`, the conversions, save/load. Round-trip tests.
* **9.3 Version handling:** version detection, rejecting future versions, the migration chain skeleton with a test-only v0 → v1 migration.
* **9.4 Images:** embedded image storage per the approved proposal. Round-trip tests.
* **9.5 Fixtures:** frozen v1 fixtures and the fixture test runner.
* **9.6 GUI:** New/Open/Save/Save As, unsaved-changes prompt, recent files.
