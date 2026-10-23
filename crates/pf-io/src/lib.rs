//! File import and export for PhotonForge: SVG, raster images, and project files.
//!
//! Converts external formats to and from the `pf-core` data model. Imports
//! are pure (bytes in, data out); only `export_gcode` touches the filesystem.

mod color_layers;
mod error;
mod gcode_file;
mod svg;
mod svg_convert;

pub use error::{ImportWarning, IoError};
pub use gcode_file::{WriteMode, export_gcode};
pub use svg::{DEFAULT_SVG_DPI, SvgImport, SvgImportOptions, import_svg};
