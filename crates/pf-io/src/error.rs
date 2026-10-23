use std::path::PathBuf;

use pf_core::{DocumentError, UnitsError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IoError {
    #[error("could not read SVG: {0}")]
    Parse(String),
    #[error(transparent)]
    InvalidDpi(#[from] UnitsError),
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} already exists", .0.display())]
    AlreadyExists(PathBuf),
}

/// Something in an imported file that was skipped or changed. `id` is the
/// SVG element id, when the element has one.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportWarning {
    SkippedText {
        id: Option<String>,
    },
    SkippedImage {
        id: Option<String>,
    },
    /// Imported, assigned to a Fill layer.
    FillOnlyShape {
        id: Option<String>,
    },
    /// Gradient or pattern paint. Imported on a black layer.
    UnsupportedPaint {
        id: Option<String>,
    },
    /// The group's contents are imported unclipped.
    IgnoredClipPath {
        id: Option<String>,
    },
    /// The group's contents are imported unmasked.
    IgnoredMask {
        id: Option<String>,
    },
    /// Hidden, or has neither stroke nor fill. Skipped.
    HiddenShape {
        id: Option<String>,
    },
    /// The root size has no physical unit, so `assumed_dpi` set the scale.
    NoPhysicalSize {
        assumed_dpi: f64,
    },
    /// Geometry that cannot be represented. Skipped.
    InvalidGeometry {
        id: Option<String>,
        reason: String,
    },
}
