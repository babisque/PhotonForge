//! Everything the GUI shows and edits. Panels read and change it; the logic
//! behind every change lives in the library crates.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use pf_core::{
    Document, Handle, History, LayerId, MachineError, MachineProfile, Origin, Point, Rect, ShapeId,
    Viewport,
};
use pf_gcode::JobOptions;
use pf_io::{ImportWarning, IoError, SvgImport};

use crate::display::{DisplayCache, PreviewCache};

/// Which editor the right-hand side panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SideTab {
    #[default]
    Layers,
    Machine,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatusMessage {
    pub level: StatusLevel,
    pub text: String,
}

/// What a primary-button drag on the canvas is doing. Positions are in mm.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Interaction {
    #[default]
    Idle,
    Marquee {
        start: Point,
    },
    Move {
        start: Point,
    },
    Scale {
        handle: Handle,
        start: Point,
        bounds: Rect,
    },
    Rotate {
        start: Point,
        center: Point,
    },
}

/// Machine profile values being edited. Only `MachineProfile::new` decides
/// whether they are valid, so the active profile always passes validation.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileDraft {
    pub width_mm: f64,
    pub height_mm: f64,
    pub origin: Origin,
    pub max_s_value: u32,
    pub max_feed_mm_min: f64,
    pub max_power_percent: f64,
    pub frame_power_percent: f64,
}

impl ProfileDraft {
    /// Work areas start at machine (0, 0).
    pub fn from_profile(profile: &MachineProfile) -> ProfileDraft {
        let area = profile.work_area();
        ProfileDraft {
            width_mm: area.width(),
            height_mm: area.height(),
            origin: profile.origin(),
            max_s_value: profile.max_s_value(),
            max_feed_mm_min: profile.max_feed_mm_min(),
            max_power_percent: profile.max_power_percent(),
            frame_power_percent: profile.frame_power_percent(),
        }
    }

    pub fn build(&self) -> Result<MachineProfile, MachineError> {
        MachineProfile::new(
            Rect::new(Point::ORIGIN, Point::new(self.width_mm, self.height_mm)),
            self.origin,
            self.max_s_value,
            self.max_feed_mm_min,
            self.max_power_percent,
            self.frame_power_percent,
        )
    }
}

/// Work running on a background thread so file I/O never blocks a frame.
#[derive(Debug)]
pub enum BackgroundJob {
    Import {
        path: PathBuf,
        result: Receiver<Result<SvgImport, IoError>>,
    },
    /// Generates G-code and writes it; reports the number of lines.
    Export {
        path: PathBuf,
        result: Receiver<anyhow::Result<usize>>,
    },
}

/// Warnings from the last import, shown until the user dismisses them.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportReport {
    pub file: String,
    pub warnings: Vec<ImportWarning>,
}

#[derive(Debug)]
pub struct AppState {
    pub document: Document,
    /// The only way the document is edited, so undo covers every change.
    pub history: History,
    pub profile: MachineProfile,
    pub profile_draft: ProfileDraft,
    /// Why the last attempt to apply the draft was refused.
    pub profile_error: Option<String>,
    pub viewport: Viewport,
    pub display: DisplayCache,
    /// Selected shapes, in the order they were selected.
    pub selection: Vec<ShapeId>,
    /// The layer shown in the layer editor.
    pub selected_layer: Option<LayerId>,
    pub interaction: Interaction,
    /// An arrow-key nudge is in progress; its steps merge into one undo step.
    pub nudging: bool,
    /// Fit the work area into the canvas on the next frame.
    pub needs_fit: bool,
    pub cursor_mm: Option<Point>,
    pub show_frame_time: bool,
    pub side_tab: SideTab,
    pub status: Option<StatusMessage>,
    /// At most one job runs at a time.
    pub job: Option<BackgroundJob>,
    pub import_report: Option<ImportReport>,
    /// File stem of the last import, used to name the G-code file.
    pub last_import_stem: Option<String>,
    pub job_options: JobOptions,
    pub preview: PreviewCache,
    /// Draw the planned toolpaths over the canvas.
    pub show_preview: bool,
    pub show_travels: bool,
}

impl AppState {
    pub fn new() -> AppState {
        AppState {
            document: Document::new(),
            history: History::new(),
            profile: MachineProfile::default(),
            profile_draft: ProfileDraft::from_profile(&MachineProfile::default()),
            profile_error: None,
            viewport: Viewport::default(),
            display: DisplayCache::default(),
            selection: Vec::new(),
            selected_layer: None,
            interaction: Interaction::Idle,
            nudging: false,
            needs_fit: true,
            cursor_mm: None,
            show_frame_time: false,
            side_tab: SideTab::default(),
            status: None,
            job: None,
            import_report: None,
            last_import_stem: None,
            job_options: JobOptions::default(),
            preview: PreviewCache::default(),
            show_preview: false,
            show_travels: true,
        }
    }

    pub fn set_status(&mut self, level: StatusLevel, text: impl Into<String>) {
        self.status = Some(StatusMessage {
            level,
            text: text.into(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_is_an_empty_document_with_the_default_profile() {
        let state = AppState::new();
        assert!(state.document.layers().is_empty());
        assert!(state.document.shapes().is_empty());
        assert_eq!(state.profile, MachineProfile::default());
        assert!(state.needs_fit);
        assert_eq!(state.profile_draft.build(), Ok(state.profile.clone()));
        assert_eq!(state.side_tab, SideTab::Layers);
        assert!(!state.history.can_undo());
        assert_eq!(state.status, None);
        assert!(state.job.is_none());
        assert_eq!(state.import_report, None);
    }

    #[test]
    fn draft_round_trips_a_valid_profile() {
        let profile = MachineProfile::new(
            Rect::new(Point::ORIGIN, Point::new(400.0, 400.0)),
            Origin::BottomLeft,
            255,
            3000.0,
            80.0,
            1.0,
        )
        .unwrap();
        let draft = ProfileDraft::from_profile(&profile);
        assert_eq!(draft.width_mm, 400.0);
        assert_eq!(draft.max_s_value, 255);
        assert_eq!(draft.build(), Ok(profile));
    }

    #[test]
    fn invalid_drafts_are_refused_by_the_core_rules() {
        let valid = ProfileDraft::from_profile(&MachineProfile::default());
        let too_much_power = ProfileDraft {
            max_power_percent: 120.0,
            ..valid.clone()
        };
        assert!(matches!(
            too_much_power.build(),
            Err(MachineError::InvalidPowerLimit(_))
        ));
        let frame_above_max = ProfileDraft {
            max_power_percent: 40.0,
            frame_power_percent: 50.0,
            ..valid.clone()
        };
        assert!(matches!(
            frame_above_max.build(),
            Err(MachineError::InvalidFramePower { .. })
        ));
        let no_area = ProfileDraft {
            width_mm: 0.0,
            ..valid
        };
        assert!(matches!(
            no_area.build(),
            Err(MachineError::InvalidWorkArea { .. })
        ));
    }
}
