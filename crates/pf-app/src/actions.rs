//! Thin wrappers that call into the library crates and report the outcome
//! in the status bar. Panels call these instead of handling errors.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;

use pf_core::{
    Command, Document, LayerId, MachineProfile, Shape, ShapeId, Transform, is_inside_work_area,
};
use pf_gcode::JobOptions;
use pf_io::{IoError, SvgImport, SvgImportOptions, WriteMode};

use crate::panels::preview::describe_gcode_error;
use crate::state::{AppState, BackgroundJob, ImportReport, StatusLevel};

/// File → Import SVG: asks for a file, then reads and parses it on a
/// background thread. The native dialog itself stays on the UI thread,
/// which some platforms require.
pub fn import_svg(state: &mut AppState) {
    if state.job.is_some() {
        return;
    }
    let picked = rfd::FileDialog::new()
        .set_title("Import SVG")
        .add_filter("SVG", &["svg"])
        .pick_file();
    if let Some(path) = picked {
        start_import(state, path);
    }
}

pub fn start_import(state: &mut AppState, path: PathBuf) {
    let (sender, receiver) = mpsc::channel();
    let worker_path = path.clone();
    let spawned = thread::Builder::new()
        .name("svg-import".into())
        .spawn(move || {
            // Sending only fails once the app has closed; nobody is left to tell.
            let _ = sender.send(read_and_import(&worker_path));
        });
    match spawned {
        Ok(_) => {
            state.set_status(
                StatusLevel::Info,
                format!("Importing {}…", file_name(&path)),
            );
            state.job = Some(BackgroundJob::Import {
                path,
                result: receiver,
            });
        }
        Err(err) => state.set_status(
            StatusLevel::Error,
            format!("Could not start the import: {err}"),
        ),
    }
}

fn read_and_import(path: &Path) -> Result<SvgImport, IoError> {
    let bytes = std::fs::read(path).map_err(|source| IoError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    pf_io::import_svg(&bytes, &SvgImportOptions::default())
}

/// File → Save G-code: asks where to save, then generates and writes the
/// file on a background thread.
pub fn save_gcode(state: &mut AppState) {
    if state.job.is_some() {
        return;
    }
    let stem = state.last_import_stem.as_deref().unwrap_or("job");
    let picked = rfd::FileDialog::new()
        .set_title("Save G-code")
        .add_filter("G-code", &["gcode", "nc"])
        .set_file_name(format!("{stem}.gcode"))
        .save_file();
    if let Some(path) = picked {
        start_export(state, path);
    }
}

/// Overwrites `path`: the save dialog has already confirmed replacing it,
/// and the write is atomic.
pub fn start_export(state: &mut AppState, path: PathBuf) {
    let (sender, receiver) = mpsc::channel();
    let doc = state.document.clone();
    let profile = state.profile.clone();
    let options = state.job_options.clone();
    let worker_path = path.clone();
    let spawned = thread::Builder::new()
        .name("gcode-export".into())
        .spawn(move || {
            // Sending only fails once the app has closed; nobody is left to tell.
            let _ = sender.send(write_gcode(&doc, &profile, &options, &worker_path));
        });
    match spawned {
        Ok(_) => {
            state.set_status(StatusLevel::Info, format!("Saving {}…", file_name(&path)));
            state.job = Some(BackgroundJob::Export {
                path,
                result: receiver,
            });
        }
        Err(err) => state.set_status(StatusLevel::Error, format!("Could not start saving: {err}")),
    }
}

/// Returns the number of lines written.
fn write_gcode(
    doc: &Document,
    profile: &MachineProfile,
    options: &JobOptions,
    path: &Path,
) -> anyhow::Result<usize> {
    let gcode = pf_gcode::generate(doc, profile, options)
        .map_err(|err| anyhow::anyhow!(describe_gcode_error(&err, doc)))?;
    pf_io::export_gcode(path, &gcode, WriteMode::Overwrite)?;
    Ok(gcode.lines().count())
}

/// Plans the job preview if the document, profile or options changed.
pub fn refresh_preview(state: &mut AppState) {
    state.preview.sync(
        &state.document,
        state.history.revision(),
        &state.profile,
        &state.job_options,
    );
}

/// Picks up the result of a finished background job. Call once per frame.
pub fn poll_jobs(state: &mut AppState) {
    let Some(job) = state.job.take() else {
        return;
    };
    match job {
        BackgroundJob::Import { path, result } => match result.try_recv() {
            Ok(Ok(import)) => finish_import(state, &path, import),
            Ok(Err(err)) => {
                state.set_status(StatusLevel::Error, format!("Import failed: {err}"));
            }
            Err(TryRecvError::Empty) => state.job = Some(BackgroundJob::Import { path, result }),
            Err(TryRecvError::Disconnected) => state.set_status(
                StatusLevel::Error,
                format!("Import of {} stopped unexpectedly", file_name(&path)),
            ),
        },
        BackgroundJob::Export { path, result } => match result.try_recv() {
            Ok(Ok(lines)) => state.set_status(
                StatusLevel::Info,
                format!("Saved {lines} lines of G-code to {}", file_name(&path)),
            ),
            Ok(Err(err)) => state.set_status(
                StatusLevel::Error,
                format!("Could not save {}: {err:#}", file_name(&path)),
            ),
            Err(TryRecvError::Empty) => state.job = Some(BackgroundJob::Export { path, result }),
            Err(TryRecvError::Disconnected) => state.set_status(
                StatusLevel::Error,
                format!("Saving {} stopped unexpectedly", file_name(&path)),
            ),
        },
    }
}

fn finish_import(state: &mut AppState, path: &Path, import: SvgImport) {
    let file = file_name(path);
    if !import.warnings.is_empty() {
        state.import_report = Some(ImportReport {
            file: file.clone(),
            warnings: import.warnings,
        });
    }
    if import.document.shapes().is_empty() {
        state.set_status(
            StatusLevel::Warning,
            format!("{file} contains no shapes that can be imported"),
        );
        return;
    }

    let first_new = state.document.shapes().len();
    let command = Command::ImportDocument(import.document);
    if let Err(err) = state.history.apply(&mut state.document, command) {
        state.set_status(StatusLevel::Error, format!("Import failed: {err}"));
        return;
    }
    if let Some(stem) = path.file_stem() {
        state.last_import_stem = Some(stem.to_string_lossy().into_owned());
    }
    let added = &state.document.shapes()[first_new..];
    let count = added.len();
    let added_ids = added.iter().map(|s| s.id).collect();
    let bounds = added
        .iter()
        .filter_map(Shape::bounds)
        .reduce(|a, b| a.union(&b));
    let outside = bounds.is_some_and(|b| !is_inside_work_area(&state.profile, &b));
    state.selection = added_ids;
    if outside {
        state.set_status(
            StatusLevel::Warning,
            format!("Imported {count} shape(s) from {file}; some lie outside the work area"),
        );
    } else {
        state.set_status(
            StatusLevel::Info,
            format!("Imported {count} shape(s) from {file}"),
        );
    }
}

fn file_name(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.display().to_string(),
    }
}

/// Applies a document edit as a new undo step, reporting failures.
pub fn apply(state: &mut AppState, command: Command) {
    record(state, command, false);
}

/// Like `apply`, but continues the previous step of the same kind until
/// `History::seal` is called.
pub fn apply_merged(state: &mut AppState, command: Command) {
    record(state, command, true);
}

fn record(state: &mut AppState, command: Command, merged: bool) {
    let label = command.label();
    let result = if merged {
        state.history.apply_merged(&mut state.document, command)
    } else {
        state.history.apply(&mut state.document, command)
    };
    if let Err(err) = result {
        state.set_status(StatusLevel::Error, format!("{label} failed: {err}"));
    }
}

/// Applies `delta` on top of every selected shape's transform.
pub fn transform_selection(state: &mut AppState, delta: Transform, merged: bool) {
    if state.selection.is_empty() || delta.is_identity() {
        return;
    }
    let selected: HashSet<ShapeId> = state.selection.iter().copied().collect();
    let items = state
        .document
        .shapes()
        .iter()
        .filter(|s| selected.contains(&s.id))
        .map(|s| (s.id, s.transform.then(delta)))
        .collect();
    record(state, Command::SetTransforms(items), merged);
}

pub fn delete_selection(state: &mut AppState) {
    if state.selection.is_empty() {
        return;
    }
    let ids = std::mem::take(&mut state.selection);
    apply(state, Command::DeleteShapes(ids));
}

/// Replaces the machine profile with the draft if it passes validation;
/// otherwise keeps the current profile and records why.
pub fn apply_profile_draft(state: &mut AppState) {
    match state.profile_draft.build() {
        Ok(profile) => {
            if profile.work_area() != state.profile.work_area() {
                state.needs_fit = true;
            }
            state.profile = profile;
            state.profile_error = None;
            state.set_status(StatusLevel::Info, "Machine profile updated");
        }
        Err(err) => state.profile_error = Some(err.to_string()),
    }
}

/// Moves the selected shapes onto `layer`.
pub fn assign_selection(state: &mut AppState, layer: LayerId) {
    if state.selection.is_empty() {
        return;
    }
    let ids = state.selection.clone();
    apply(state, Command::AssignShapes { ids, layer });
}

pub fn select_all(state: &mut AppState) {
    state.selection = state.document.shapes().iter().map(|s| s.id).collect();
}

/// Drops selected IDs that are no longer in the document.
fn prune_selection(state: &mut AppState) {
    let existing: HashSet<ShapeId> = state.document.shapes().iter().map(|s| s.id).collect();
    state.selection.retain(|id| existing.contains(id));
}

pub fn undo(state: &mut AppState) {
    let result = state.history.undo(&mut state.document);
    prune_selection(state);
    match result {
        Ok(Some(label)) => state.set_status(StatusLevel::Info, format!("Undid: {label}")),
        Ok(None) => {}
        Err(err) => state.set_status(StatusLevel::Error, format!("Undo failed: {err}")),
    }
}

pub fn redo(state: &mut AppState) {
    let result = state.history.redo(&mut state.document);
    prune_selection(state);
    match result {
        Ok(Some(label)) => state.set_status(StatusLevel::Info, format!("Redid: {label}")),
        Ok(None) => {}
        Err(err) => state.set_status(StatusLevel::Error, format!("Redo failed: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use pf_io::ImportWarning;

    const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="50mm" viewBox="0 0 100 50">
        <rect x="10" y="10" width="20" height="10" fill="none" stroke="red"/>
        <rect x="40" y="10" width="20" height="10" fill="none" stroke="blue"/>
        <text x="5" y="45">label</text>
    </svg>"#;

    fn parsed(svg: &str) -> SvgImport {
        pf_io::import_svg(svg.as_bytes(), &SvgImportOptions::default()).unwrap()
    }

    /// A state whose pending job has already produced `result`.
    fn state_with_result(result: Result<SvgImport, IoError>) -> AppState {
        let mut state = AppState::new();
        queue_result(&mut state, "dir/logo.svg", result);
        state
    }

    fn queue_result(state: &mut AppState, path: &str, result: Result<SvgImport, IoError>) {
        let (sender, receiver) = mpsc::channel();
        sender.send(result).unwrap();
        state.job = Some(BackgroundJob::Import {
            path: PathBuf::from(path),
            result: receiver,
        });
    }

    fn level(state: &AppState) -> StatusLevel {
        state.status.as_ref().unwrap().level
    }

    fn imported_state() -> AppState {
        let mut state = state_with_result(Ok(parsed(SVG)));
        poll_jobs(&mut state);
        state
    }

    fn translation(state: &AppState, id: ShapeId) -> (f64, f64) {
        let t = state.document.shape(id).unwrap().transform;
        (t.e, t.f)
    }

    #[test]
    fn import_selects_the_new_shapes_and_undo_prunes_them() {
        let mut state = imported_state();
        let ids: Vec<ShapeId> = state.document.shapes().iter().map(|s| s.id).collect();
        assert_eq!(state.selection, ids);
        undo(&mut state);
        assert!(state.selection.is_empty());
    }

    #[test]
    fn transform_selection_moves_only_selected_shapes() {
        let mut state = imported_state();
        let (first, second) = (state.selection[0], state.selection[1]);
        state.selection = vec![second];
        transform_selection(&mut state, Transform::translate(5.0, -2.0), false);
        assert_eq!(translation(&state, first), (0.0, 0.0));
        assert_eq!(translation(&state, second), (5.0, -2.0));
        assert_eq!(state.history.undo_label(), Some("Transform"));
        // The identity is not worth an undo step.
        transform_selection(&mut state, Transform::identity(), false);
        undo(&mut state);
        assert_eq!(translation(&state, second), (0.0, 0.0));
    }

    #[test]
    fn merged_nudges_are_one_undo_step_until_sealed() {
        let mut state = imported_state();
        let id = state.selection[0];
        for _ in 0..3 {
            transform_selection(&mut state, Transform::translate(1.0, 0.0), true);
        }
        state.history.seal();
        transform_selection(&mut state, Transform::translate(0.0, 1.0), true);
        undo(&mut state);
        assert_eq!(translation(&state, id), (3.0, 0.0));
        undo(&mut state);
        assert_eq!(translation(&state, id), (0.0, 0.0));
    }

    #[test]
    fn delete_selection_removes_and_clears() {
        let mut state = imported_state();
        delete_selection(&mut state);
        assert!(state.document.shapes().is_empty());
        assert!(state.selection.is_empty());
        undo(&mut state);
        assert_eq!(state.document.shapes().len(), 2);
        // Deleting with nothing selected adds no step.
        delete_selection(&mut state);
        assert_eq!(state.history.undo_label(), Some("Import"));
    }

    #[test]
    fn select_all_selects_in_document_order() {
        let mut state = imported_state();
        state.selection.clear();
        select_all(&mut state);
        let ids: Vec<ShapeId> = state.document.shapes().iter().map(|s| s.id).collect();
        assert_eq!(state.selection, ids);
    }

    #[test]
    fn failed_edits_are_reported() {
        let mut state = AppState::new();
        apply(&mut state, Command::DeleteShapes(vec![ShapeId(42)]));
        let status = state.status.unwrap();
        assert_eq!(status.level, StatusLevel::Error);
        assert!(status.text.starts_with("Delete failed"));
    }

    #[test]
    fn finished_import_is_one_undoable_step_with_a_report() {
        let mut state = state_with_result(Ok(parsed(SVG)));
        poll_jobs(&mut state);

        assert!(state.job.is_none());
        assert_eq!(state.document.shapes().len(), 2);
        assert_eq!(state.document.layers().len(), 2);
        assert_eq!(state.history.undo_label(), Some("Import"));
        assert_eq!(level(&state), StatusLevel::Info);
        assert_eq!(
            state.status.as_ref().unwrap().text,
            "Imported 2 shape(s) from logo.svg"
        );
        let report = state.import_report.clone().unwrap();
        assert_eq!(report.file, "logo.svg");
        assert!(matches!(
            report.warnings[..],
            [ImportWarning::SkippedText { .. }]
        ));

        undo(&mut state);
        assert!(state.document.shapes().is_empty());
    }

    #[test]
    fn second_import_reuses_layers_by_color() {
        let mut state = state_with_result(Ok(parsed(SVG)));
        poll_jobs(&mut state);
        queue_result(&mut state, "again.svg", Ok(parsed(SVG)));
        poll_jobs(&mut state);
        assert_eq!(state.document.shapes().len(), 4);
        assert_eq!(state.document.layers().len(), 2);
    }

    #[test]
    fn shapes_outside_the_work_area_give_a_warning() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="500mm" height="50mm" viewBox="0 0 500 50">
            <rect x="400" y="10" width="20" height="10" fill="none" stroke="red"/>
        </svg>"#;
        let mut state = state_with_result(Ok(parsed(svg)));
        poll_jobs(&mut state);
        assert_eq!(state.document.shapes().len(), 1);
        assert_eq!(level(&state), StatusLevel::Warning);
        assert_eq!(state.import_report, None);
    }

    #[test]
    fn file_without_shapes_adds_no_undo_step() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm"><text>only text</text></svg>"#;
        let mut state = state_with_result(Ok(parsed(svg)));
        poll_jobs(&mut state);
        assert!(!state.history.can_undo());
        assert_eq!(level(&state), StatusLevel::Warning);
        assert!(state.import_report.is_some());
    }

    #[test]
    fn import_errors_are_reported() {
        let mut state = state_with_result(Err(IoError::Parse("broken".into())));
        poll_jobs(&mut state);
        assert!(state.job.is_none());
        assert_eq!(level(&state), StatusLevel::Error);
        assert!(state.document.shapes().is_empty());
    }

    #[test]
    fn pending_job_is_kept_and_a_dead_worker_is_reported() {
        let (sender, receiver) = mpsc::channel::<Result<SvgImport, IoError>>();
        let mut state = AppState::new();
        state.job = Some(BackgroundJob::Import {
            path: PathBuf::from("slow.svg"),
            result: receiver,
        });
        poll_jobs(&mut state);
        assert!(state.job.is_some());
        assert_eq!(state.status, None);

        drop(sender);
        poll_jobs(&mut state);
        assert!(state.job.is_none());
        assert_eq!(level(&state), StatusLevel::Error);
    }

    #[test]
    fn missing_file_fails_on_the_worker_thread() {
        let mut state = AppState::new();
        start_import(&mut state, PathBuf::from("does/not/exist.svg"));
        assert_eq!(level(&state), StatusLevel::Info);
        let deadline = Instant::now() + Duration::from_secs(10);
        while state.job.is_some() && Instant::now() < deadline {
            poll_jobs(&mut state);
            thread::sleep(Duration::from_millis(5));
        }
        assert!(state.job.is_none());
        assert_eq!(level(&state), StatusLevel::Error);
        assert!(state.status.unwrap().text.contains("exist.svg"));
    }

    fn state_with_one_step() -> (AppState, LayerId) {
        let mut state = AppState::new();
        let layer = state.document.add_layer("Cut", [0, 0, 0]).unwrap();
        let command = Command::SetLayerPasses {
            id: layer,
            passes: 4,
        };
        state.history.apply(&mut state.document, command).unwrap();
        (state, layer)
    }

    #[test]
    fn undo_and_redo_report_their_step() {
        let (mut state, layer) = state_with_one_step();

        undo(&mut state);
        assert_eq!(state.document.layer(layer).unwrap().passes, 1);
        let status = state.status.clone().unwrap();
        assert_eq!(status.level, StatusLevel::Info);
        assert_eq!(status.text, "Undid: Change passes");

        redo(&mut state);
        assert_eq!(state.document.layer(layer).unwrap().passes, 4);
        assert_eq!(state.status.unwrap().text, "Redid: Change passes");
    }

    #[test]
    fn nothing_to_undo_leaves_the_status_alone() {
        let mut state = AppState::new();
        undo(&mut state);
        redo(&mut state);
        assert_eq!(state.status, None);
    }

    #[test]
    fn failed_undo_is_reported_as_an_error() {
        let (mut state, _) = state_with_one_step();
        state.document = Document::new();
        undo(&mut state);
        assert_eq!(state.status.unwrap().level, StatusLevel::Error);
    }

    #[test]
    fn valid_profile_draft_is_applied_and_refits_the_view() {
        let mut state = AppState::new();
        state.needs_fit = false;
        state.profile_draft.width_mm = 400.0;
        state.profile_draft.max_power_percent = 60.0;
        apply_profile_draft(&mut state);
        assert_eq!(state.profile.work_area().width(), 400.0);
        assert_eq!(state.profile.max_power_percent(), 60.0);
        assert!(state.needs_fit);
        assert_eq!(state.profile_error, None);
        assert_eq!(level(&state), StatusLevel::Info);
    }

    #[test]
    fn invalid_profile_draft_keeps_the_current_profile() {
        let mut state = AppState::new();
        let before = state.profile.clone();
        state.profile_draft.max_power_percent = 150.0;
        apply_profile_draft(&mut state);
        assert_eq!(state.profile, before);
        assert!(state.profile_error.as_deref().unwrap().contains("150"));
        assert_eq!(state.status, None);
    }

    /// Polls until the background job finishes or 10 s pass.
    fn wait_for_job(state: &mut AppState) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while state.job.is_some() && Instant::now() < deadline {
            poll_jobs(state);
            thread::sleep(Duration::from_millis(5));
        }
        assert!(state.job.is_none(), "job did not finish");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pf-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn import_remembers_the_file_stem() {
        let state = imported_state();
        assert_eq!(state.last_import_stem.as_deref(), Some("logo"));
    }

    #[test]
    fn export_writes_exactly_what_generate_produces() {
        let mut state = imported_state();
        let dir = temp_dir("export");
        let path = dir.join("logo.gcode");
        std::fs::write(&path, "old contents").unwrap();

        start_export(&mut state, path.clone());
        wait_for_job(&mut state);

        let expected =
            pf_gcode::generate(&state.document, &state.profile, &state.job_options).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        let status = state.status.clone().unwrap();
        assert_eq!(status.level, StatusLevel::Info);
        assert_eq!(
            status.text,
            format!(
                "Saved {} lines of G-code to logo.gcode",
                expected.lines().count()
            )
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_errors_use_layer_names_and_write_nothing() {
        let mut state = imported_state();
        let red = state.document.layers()[0].id;
        apply(
            &mut state,
            Command::SetLayerPower {
                id: red,
                power: 150.0,
            },
        );
        let dir = temp_dir("export-error");
        let path = dir.join("bad.gcode");

        start_export(&mut state, path.clone());
        wait_for_job(&mut state);

        let status = state.status.clone().unwrap();
        assert_eq!(status.level, StatusLevel::Error);
        assert!(status.text.starts_with("Could not save bad.gcode: "));
        assert!(status.text.contains("\"#FF0000\""), "{}", status.text);
        assert!(!path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refresh_preview_follows_edits() {
        let mut state = imported_state();
        refresh_preview(&mut state);
        let before = state.preview.result().unwrap().clone().unwrap();
        let id = state.selection[0];
        transform_selection(&mut state, Transform::translate(1000.0, 0.0), false);
        refresh_preview(&mut state);
        assert!(state.preview.result().unwrap().is_err());
        undo(&mut state);
        refresh_preview(&mut state);
        assert_eq!(state.preview.result().unwrap().as_ref().unwrap(), &before);
        assert!(state.document.shape(id).is_some());
    }
}
