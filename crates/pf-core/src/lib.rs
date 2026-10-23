//! Core data model for PhotonForge: geometry, shapes, layers, and units.
//!
//! The internal unit is the millimeter. This crate has no I/O and no GUI code,
//! and it depends on no other PhotonForge crate.

pub mod document;
pub mod flatten;
pub mod geometry;
pub mod handles;
pub mod history;
pub mod hit;
pub mod layer;
pub mod machine;
pub mod path;
pub mod shape;
pub mod transform;
pub mod units;
pub mod viewport;

pub use document::{Document, DocumentError};
pub use flatten::{Polyline, flatten_path, flatten_segment};
pub use geometry::{EPS, GeometryError, Point, Rect, Vec2, approx_eq, distance_to_segment};
pub use handles::{
    Handle, MIN_SIZE_MM, handle_position, hit_handle, move_delta, rotate_delta, scale_delta,
};
pub use history::{Command, History, HistoryError, MAX_UNDO};
pub use hit::{MarqueeMode, pick_shape, selection_bounds, shapes_in_rect};
pub use layer::{Layer, LayerId, LayerMode, PowerMode};
pub use machine::{
    MachineError, MachineProfile, Origin, clamp_feed, clamp_power, is_inside_work_area, power_to_s,
};
pub use path::{CONTINUITY_TOL, Path, PathBuilder, Segment, SubPath};
pub use shape::{EllipseShape, PolygonShape, RectShape, Shape, ShapeError, ShapeId, ShapeKind};
pub use transform::Transform;
pub use units::UnitsError;
pub use viewport::{MAX_ZOOM, MIN_ZOOM, Viewport, grid_step};
