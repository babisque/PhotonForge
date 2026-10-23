//! Core data model for PhotonForge: geometry, shapes, layers, and units.
//!
//! The internal unit is the millimeter. This crate has no I/O and no GUI code,
//! and it depends on no other PhotonForge crate.

pub mod document;
pub mod flatten;
pub mod geometry;
pub mod layer;
pub mod machine;
pub mod path;
pub mod shape;
pub mod transform;
pub mod units;

pub use document::{Document, DocumentError};
pub use flatten::{Polyline, flatten_path, flatten_segment};
pub use geometry::{EPS, GeometryError, Point, Rect, Vec2, approx_eq};
pub use layer::{Layer, LayerId, LayerMode, PowerMode};
pub use machine::{
    MachineError, MachineProfile, Origin, clamp_feed, clamp_power, is_inside_work_area, power_to_s,
};
pub use path::{CONTINUITY_TOL, Path, PathBuilder, Segment, SubPath};
pub use shape::{EllipseShape, PolygonShape, RectShape, Shape, ShapeError, ShapeId, ShapeKind};
pub use transform::Transform;
pub use units::UnitsError;
