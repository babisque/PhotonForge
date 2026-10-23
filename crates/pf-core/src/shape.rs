use std::f64::consts::{FRAC_PI_2, PI};

use thiserror::Error;

use crate::geometry::{EPS, Point, Rect, Vec2};
use crate::layer::LayerId;
use crate::path::{Path, Segment, SubPath};
use crate::transform::Transform;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShapeId(pub u64);

#[derive(Debug, Clone, PartialEq, Error)]
pub enum ShapeError {
    #[error("invalid size {width} x {height} mm: must be finite and greater than zero")]
    InvalidSize { width: f64, height: f64 },
    #[error("invalid corner radius {radius} mm: must be between 0 and {max} mm")]
    InvalidCornerRadius { radius: f64, max: f64 },
    #[error("invalid radii ({rx}, {ry}) mm: must be finite and greater than zero")]
    InvalidRadii { rx: f64, ry: f64 },
    #[error("polygon needs at least {min} points, got {got}")]
    TooFewPoints { min: usize, got: usize },
    #[error("polygon contains a NaN or infinite coordinate")]
    NonFinite,
}

/// Rectangle with its bottom-left corner at the local origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectShape {
    width: f64,         // mm
    height: f64,        // mm
    corner_radius: f64, // mm
}

impl RectShape {
    /// `corner_radius` may be at most half of the shorter side.
    pub fn new(width: f64, height: f64, corner_radius: f64) -> Result<RectShape, ShapeError> {
        let size_ok = width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0;
        if !size_ok {
            return Err(ShapeError::InvalidSize { width, height });
        }
        let max = width.min(height) / 2.0;
        if !(corner_radius.is_finite() && (0.0..=max).contains(&corner_radius)) {
            return Err(ShapeError::InvalidCornerRadius {
                radius: corner_radius,
                max,
            });
        }
        Ok(RectShape {
            width,
            height,
            corner_radius,
        })
    }

    pub fn width(&self) -> f64 {
        self.width
    }

    pub fn height(&self) -> f64 {
        self.height
    }

    pub fn corner_radius(&self) -> f64 {
        self.corner_radius
    }

    /// Closed counter-clockwise outline. Rounded corners are quarter arcs.
    pub fn to_path(&self) -> Path {
        let (w, h, r) = (self.width, self.height, self.corner_radius);
        if r == 0.0 {
            let corners = [
                Point::new(0.0, 0.0),
                Point::new(w, 0.0),
                Point::new(w, h),
                Point::new(0.0, h),
            ];
            return Path::new(vec![closed_polygon(&corners)]);
        }
        let corner = |center: Point, start_angle: f64| Segment::Arc {
            center,
            radii: Vec2::new(r, r),
            x_rotation: 0.0,
            start_angle,
            sweep_angle: FRAC_PI_2,
        };
        let arcs = [
            corner(Point::new(w - r, r), -FRAC_PI_2),
            corner(Point::new(w - r, h - r), 0.0),
            corner(Point::new(r, h - r), FRAC_PI_2),
            corner(Point::new(r, r), PI),
        ];
        let mut segments = Vec::with_capacity(8);
        let mut current = arcs[3].end();
        for arc in arcs {
            // Straight edges vanish when the radius is exactly half a side.
            if current.distance(arc.start()) > EPS {
                segments.push(Segment::Line {
                    from: current,
                    to: arc.start(),
                });
            }
            segments.push(arc);
            current = arc.end();
        }
        Path::new(vec![SubPath::new_unchecked(segments, true)])
    }
}

/// Ellipse centred on the local origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EllipseShape {
    rx: f64, // mm
    ry: f64, // mm
}

impl EllipseShape {
    pub fn new(rx: f64, ry: f64) -> Result<EllipseShape, ShapeError> {
        if rx.is_finite() && ry.is_finite() && rx > 0.0 && ry > 0.0 {
            Ok(EllipseShape { rx, ry })
        } else {
            Err(ShapeError::InvalidRadii { rx, ry })
        }
    }

    pub fn rx(&self) -> f64 {
        self.rx
    }

    pub fn ry(&self) -> f64 {
        self.ry
    }

    /// Closed counter-clockwise outline of four quarter arcs, starting at
    /// (`rx`, 0).
    pub fn to_path(&self) -> Path {
        let segments = (0..4)
            .map(|i| Segment::Arc {
                center: Point::ORIGIN,
                radii: Vec2::new(self.rx, self.ry),
                x_rotation: 0.0,
                start_angle: i as f64 * FRAC_PI_2,
                sweep_angle: FRAC_PI_2,
            })
            .collect();
        Path::new(vec![SubPath::new_unchecked(segments, true)])
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PolygonShape {
    points: Vec<Point>, // mm
    closed: bool,
}

impl PolygonShape {
    /// An open polyline needs at least 2 points, a closed polygon at least 3.
    pub fn new(points: Vec<Point>, closed: bool) -> Result<PolygonShape, ShapeError> {
        let min = if closed { 3 } else { 2 };
        if points.len() < min {
            return Err(ShapeError::TooFewPoints {
                min,
                got: points.len(),
            });
        }
        if points.iter().any(|p| !(p.x.is_finite() && p.y.is_finite())) {
            return Err(ShapeError::NonFinite);
        }
        Ok(PolygonShape { points, closed })
    }

    pub fn points(&self) -> &[Point] {
        &self.points
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Straight lines between consecutive points; a closed polygon gets a
    /// closing line unless its last point already equals the first.
    pub fn to_path(&self) -> Path {
        if self.closed {
            return Path::new(vec![closed_polygon(&self.points)]);
        }
        let segments = lines_through(&self.points);
        Path::new(vec![SubPath::new_unchecked(segments, false)])
    }
}

/// The geometry of a shape, in local coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum ShapeKind {
    Rect(RectShape),
    Ellipse(EllipseShape),
    Polygon(PolygonShape),
    Path(Path),
}

impl ShapeKind {
    pub fn to_path(&self) -> Path {
        match self {
            ShapeKind::Rect(rect) => rect.to_path(),
            ShapeKind::Ellipse(ellipse) => ellipse.to_path(),
            ShapeKind::Polygon(polygon) => polygon.to_path(),
            ShapeKind::Path(path) => path.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub id: ShapeId,
    pub kind: ShapeKind,
    /// Local → document coordinates.
    pub transform: Transform,
    pub layer_id: LayerId,
}

impl Shape {
    /// Outline in local coordinates.
    pub fn to_path(&self) -> Path {
        self.kind.to_path()
    }

    /// Outline in document coordinates.
    pub fn world_path(&self) -> Path {
        self.kind.to_path().transform(&self.transform)
    }

    pub fn bounds(&self) -> Option<Rect> {
        self.world_path().bounds()
    }
}

fn lines_through(points: &[Point]) -> Vec<Segment> {
    points
        .windows(2)
        .map(|w| Segment::Line {
            from: w[0],
            to: w[1],
        })
        .collect()
}

fn closed_polygon(points: &[Point]) -> SubPath {
    let mut segments = lines_through(points);
    if let (Some(&first), Some(&last)) = (points.first(), points.last())
        && first != last
    {
        segments.push(Segment::Line {
            from: last,
            to: first,
        });
    }
    SubPath::new_unchecked(segments, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn shape(kind: ShapeKind, transform: Transform) -> Shape {
        Shape {
            id: ShapeId(1),
            kind,
            transform,
            layer_id: LayerId(0),
        }
    }

    fn only_subpath(path: &Path) -> &SubPath {
        assert_eq!(path.subpaths().len(), 1);
        &path.subpaths()[0]
    }

    #[test]
    fn rect_to_path() {
        let path = RectShape::new(30.0, 20.0, 0.0).unwrap().to_path();
        let sub = only_subpath(&path);
        assert!(sub.is_closed());
        assert_eq!(sub.segments().len(), 4);
        assert!(sub.start().approx_eq(p(0.0, 0.0), EPS));
        let expected = Rect::new(p(0.0, 0.0), p(30.0, 20.0));
        assert!(path.bounds().unwrap().approx_eq(&expected, EPS));
    }

    #[test]
    fn rounded_rect_to_path() {
        let path = RectShape::new(30.0, 20.0, 5.0).unwrap().to_path();
        let sub = only_subpath(&path);
        assert!(sub.is_closed());
        assert_eq!(sub.segments().len(), 8);
        let arcs = sub
            .segments()
            .iter()
            .filter(|s| matches!(s, Segment::Arc { .. }))
            .count();
        assert_eq!(arcs, 4);
        let expected = Rect::new(p(0.0, 0.0), p(30.0, 20.0));
        assert!(path.bounds().unwrap().approx_eq(&expected, EPS));
        assert!(SubPath::new(sub.segments().to_vec(), true).is_ok());
    }

    #[test]
    fn fully_rounded_rect_drops_zero_length_edges() {
        // Radius is half the height: the two vertical edges vanish.
        let path = RectShape::new(30.0, 20.0, 10.0).unwrap().to_path();
        let sub = only_subpath(&path);
        assert_eq!(sub.segments().len(), 6);
        let expected = Rect::new(p(0.0, 0.0), p(30.0, 20.0));
        assert!(path.bounds().unwrap().approx_eq(&expected, EPS));
        assert!(SubPath::new(sub.segments().to_vec(), true).is_ok());
    }

    #[test]
    fn rect_with_zero_radius_equals_plain_rect() {
        let plain = RectShape::new(12.0, 8.0, 0.0).unwrap().to_path();
        let polygon = PolygonShape::new(
            vec![p(0.0, 0.0), p(12.0, 0.0), p(12.0, 8.0), p(0.0, 8.0)],
            true,
        )
        .unwrap()
        .to_path();
        assert_eq!(plain, polygon);
    }

    #[test]
    fn invalid_rects_are_rejected() {
        for (w, h) in [
            (0.0, 5.0),
            (-1.0, 5.0),
            (5.0, f64::NAN),
            (f64::INFINITY, 5.0),
        ] {
            assert!(matches!(
                RectShape::new(w, h, 0.0),
                Err(ShapeError::InvalidSize { .. })
            ));
        }
        for r in [-0.1, 5.1, f64::NAN] {
            assert!(matches!(
                RectShape::new(20.0, 10.0, r),
                Err(ShapeError::InvalidCornerRadius { .. })
            ));
        }
        assert!(RectShape::new(20.0, 10.0, 5.0).is_ok());
    }

    #[test]
    fn ellipse_to_path() {
        let path = EllipseShape::new(10.0, 4.0).unwrap().to_path();
        let sub = only_subpath(&path);
        assert!(sub.is_closed());
        assert_eq!(sub.segments().len(), 4);
        assert!(sub.start().approx_eq(p(10.0, 0.0), EPS));
        let expected = Rect::new(p(-10.0, -4.0), p(10.0, 4.0));
        assert!(path.bounds().unwrap().approx_eq(&expected, EPS));
        assert!(SubPath::new(sub.segments().to_vec(), true).is_ok());
    }

    #[test]
    fn invalid_ellipses_are_rejected() {
        for (rx, ry) in [(0.0, 1.0), (1.0, -1.0), (f64::NAN, 1.0)] {
            assert!(matches!(
                EllipseShape::new(rx, ry),
                Err(ShapeError::InvalidRadii { .. })
            ));
        }
    }

    #[test]
    fn polygon_to_path() {
        let triangle = PolygonShape::new(vec![p(0.0, 0.0), p(4.0, 0.0), p(0.0, 3.0)], true)
            .unwrap()
            .to_path();
        let sub = only_subpath(&triangle);
        assert!(sub.is_closed());
        assert_eq!(sub.segments().len(), 3);
        assert!(sub.end().approx_eq(p(0.0, 0.0), EPS));

        let open = PolygonShape::new(vec![p(0.0, 0.0), p(4.0, 0.0), p(0.0, 3.0)], false)
            .unwrap()
            .to_path();
        let sub = only_subpath(&open);
        assert!(!sub.is_closed());
        assert_eq!(sub.segments().len(), 2);
        let expected = Rect::new(p(0.0, 0.0), p(4.0, 3.0));
        assert!(open.bounds().unwrap().approx_eq(&expected, EPS));
    }

    #[test]
    fn closed_polygon_repeating_first_point_gets_no_extra_line() {
        let points = vec![p(0.0, 0.0), p(4.0, 0.0), p(0.0, 3.0), p(0.0, 0.0)];
        let path = PolygonShape::new(points, true).unwrap().to_path();
        assert_eq!(only_subpath(&path).segments().len(), 3);
    }

    #[test]
    fn invalid_polygons_are_rejected() {
        assert_eq!(
            PolygonShape::new(vec![p(0.0, 0.0)], false),
            Err(ShapeError::TooFewPoints { min: 2, got: 1 })
        );
        assert_eq!(
            PolygonShape::new(vec![p(0.0, 0.0), p(1.0, 0.0)], true),
            Err(ShapeError::TooFewPoints { min: 3, got: 2 })
        );
        assert_eq!(
            PolygonShape::new(vec![p(0.0, 0.0), p(f64::NAN, 0.0)], false),
            Err(ShapeError::NonFinite)
        );
    }

    #[test]
    fn path_kind_passes_through() {
        let path = Path::builder()
            .move_to(p(1.0, 1.0))
            .unwrap()
            .line_to(p(2.0, 3.0))
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(ShapeKind::Path(path.clone()).to_path(), path);
    }

    #[test]
    fn transformed_shape_bounds() {
        let rect = ShapeKind::Rect(RectShape::new(20.0, 10.0, 0.0).unwrap());
        let moved = shape(
            rect,
            Transform::rotate(FRAC_PI_2).then(Transform::translate(100.0, 50.0)),
        );
        // Rotating (0,0)-(20,10) by 90° gives (-10,0)-(0,20), then translate.
        let expected = Rect::new(p(90.0, 50.0), p(100.0, 70.0));
        assert!(moved.bounds().unwrap().approx_eq(&expected, EPS));
        let local = Rect::new(p(0.0, 0.0), p(20.0, 10.0));
        assert!(moved.to_path().bounds().unwrap().approx_eq(&local, EPS));
    }

    #[test]
    fn rotated_ellipse_bounds_are_tight() {
        let ellipse = ShapeKind::Ellipse(EllipseShape::new(10.0, 5.0).unwrap());
        let s = shape(ellipse, Transform::rotate(FRAC_PI_2));
        let expected = Rect::new(p(-5.0, -10.0), p(5.0, 10.0));
        assert!(s.bounds().unwrap().approx_eq(&expected, 1e-9));
    }

    #[test]
    fn empty_path_shape_has_no_bounds() {
        let s = shape(ShapeKind::Path(Path::default()), Transform::identity());
        assert_eq!(s.bounds(), None);
    }
}
