//! All coordinates are in millimeters, Y-up.

use std::ops::{Add, Mul, Neg, Sub};

use thiserror::Error;

/// Default tolerance for floating-point comparisons (mm).
pub const EPS: f64 = 1e-9;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum GeometryError {
    #[error("a sub-path must contain at least one segment")]
    EmptySubPath,
    #[error("segment {index} does not start where the previous one ends (gap {gap} mm)")]
    Discontinuous { index: usize, gap: f64 },
    #[error("closed sub-path does not end at its start (gap {gap} mm)")]
    NotClosed { gap: f64 },
    #[error("geometry contains a NaN or infinite value")]
    NonFinite,
    #[error("invalid arc radii ({rx}, {ry}): must be finite and non-negative")]
    InvalidArcRadii { rx: f64, ry: f64 },
    #[error("no current point: start the sub-path with move_to")]
    NoCurrentPoint,
    #[error("invalid flattening tolerance {0}: must be finite and greater than zero")]
    InvalidTolerance(f64),
}

pub fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64, // mm
    pub y: f64, // mm
}

impl Point {
    pub const ORIGIN: Point = Point { x: 0.0, y: 0.0 };

    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn distance(self, other: Point) -> f64 {
        (other - self).length()
    }

    /// Linear interpolation: `t = 0` gives `self`, `t = 1` gives `other`.
    pub fn lerp(self, other: Point, t: f64) -> Point {
        self + (other - self) * t
    }

    pub fn approx_eq(self, other: Point, eps: f64) -> bool {
        approx_eq(self.x, other.x, eps) && approx_eq(self.y, other.y, eps)
    }
}

/// A displacement in the plane. Kept distinct from `Point` so that
/// transforms can treat translations correctly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec2 {
    pub x: f64, // mm
    pub y: f64, // mm
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn dot(self, other: Vec2) -> f64 {
        self.x * other.x + self.y * other.y
    }

    /// Z component of the 3D cross product. Positive when `other` is
    /// counter-clockwise from `self`.
    pub fn cross(self, other: Vec2) -> f64 {
        self.x * other.y - self.y * other.x
    }

    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    /// Unit vector in the same direction, or `None` for a (near-)zero or
    /// non-finite vector.
    pub fn normalize(self) -> Option<Vec2> {
        let len = self.length();
        if len.is_finite() && len > EPS {
            Some(Vec2::new(self.x / len, self.y / len))
        } else {
            None
        }
    }

    pub fn approx_eq(self, other: Vec2, eps: f64) -> bool {
        approx_eq(self.x, other.x, eps) && approx_eq(self.y, other.y, eps)
    }
}

impl Sub for Point {
    type Output = Vec2;

    fn sub(self, rhs: Point) -> Vec2 {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Add<Vec2> for Point {
    type Output = Point;

    fn add(self, rhs: Vec2) -> Point {
        Point::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub<Vec2> for Point {
    type Output = Point;

    fn sub(self, rhs: Vec2) -> Point {
        Point::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Add for Vec2 {
    type Output = Vec2;

    fn add(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;

    fn sub(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Vec2;

    fn mul(self, rhs: f64) -> Vec2 {
        Vec2::new(self.x * rhs, self.y * rhs)
    }
}

impl Neg for Vec2 {
    type Output = Vec2;

    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

/// Axis-aligned bounding box. Constructors guarantee `min <= max` on both axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub min: Point, // mm, bottom-left corner
    pub max: Point, // mm, top-right corner
}

impl Rect {
    pub fn new(a: Point, b: Point) -> Self {
        Self {
            min: Point::new(a.x.min(b.x), a.y.min(b.y)),
            max: Point::new(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    pub fn from_points<I: IntoIterator<Item = Point>>(points: I) -> Option<Rect> {
        let mut iter = points.into_iter();
        let first = iter.next()?;
        let start = Rect::new(first, first);
        Some(iter.fold(start, Rect::expand_to))
    }

    pub fn width(&self) -> f64 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f64 {
        self.max.y - self.min.y
    }

    pub fn center(&self) -> Point {
        self.min.lerp(self.max, 0.5)
    }

    pub fn union(&self, other: &Rect) -> Rect {
        Rect {
            min: Point::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Point::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }

    pub fn expand_to(self, point: Point) -> Rect {
        self.union(&Rect::new(point, point))
    }

    /// True when `point` lies inside or on the border (within `EPS`).
    pub fn contains_point(&self, point: Point) -> bool {
        point.x >= self.min.x - EPS
            && point.x <= self.max.x + EPS
            && point.y >= self.min.y - EPS
            && point.y <= self.max.y + EPS
    }

    /// True when `other` lies entirely inside `self`; shared borders count
    /// as inside (within `EPS`).
    pub fn contains_rect(&self, other: &Rect) -> bool {
        self.contains_point(other.min) && self.contains_point(other.max)
    }

    /// True when the rectangles overlap; touching borders count as overlapping.
    pub fn intersects(&self, other: &Rect) -> bool {
        self.min.x <= other.max.x + EPS
            && other.min.x <= self.max.x + EPS
            && self.min.y <= other.max.y + EPS
            && other.min.y <= self.max.y + EPS
    }

    pub fn translate(&self, offset: Vec2) -> Rect {
        Rect {
            min: self.min + offset,
            max: self.max + offset,
        }
    }

    /// Grows every side by `margin` mm. A negative margin shrinks the
    /// rectangle, but never past its center.
    pub fn inflate(&self, margin: f64) -> Rect {
        let c = self.center();
        Rect {
            min: Point::new(
                (self.min.x - margin).min(c.x),
                (self.min.y - margin).min(c.y),
            ),
            max: Point::new(
                (self.max.x + margin).max(c.x),
                (self.max.y + margin).max(c.y),
            ),
        }
    }

    pub fn approx_eq(&self, other: &Rect, eps: f64) -> bool {
        self.min.approx_eq(other.min, eps) && self.max.approx_eq(other.max, eps)
    }
}

/// Shortest distance from `p` to the segment `a`–`b`.
pub fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len_sq = ab.dot(ab);
    if len_sq <= 0.0 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(Point::new(x0, y0), Point::new(x1, y1))
    }

    #[test]
    fn approx_eq_respects_epsilon() {
        assert!(approx_eq(1.0, 1.0 + 1e-10, EPS));
        assert!(!approx_eq(1.0, 1.0 + 1e-6, EPS));
    }

    #[test]
    fn point_minus_point_is_vector() {
        let v = Point::new(4.0, 6.0) - Point::new(1.0, 2.0);
        assert!(v.approx_eq(Vec2::new(3.0, 4.0), EPS));
    }

    #[test]
    fn point_plus_difference_round_trips() {
        let p = Point::new(1.5, -2.0);
        let q = Point::new(-7.25, 3.0);
        assert!((p + (q - p)).approx_eq(q, EPS));
        assert!((q - (q - p)).approx_eq(p, EPS));
    }

    #[test]
    fn vector_algebra_identities() {
        let a = Vec2::new(2.0, -3.0);
        let b = Vec2::new(-1.0, 5.0);
        assert!((a + b).approx_eq(b + a, EPS));
        assert!((a - b).approx_eq(a + (-b), EPS));
        assert!((a * 2.0).approx_eq(a + a, EPS));
        assert!(approx_eq(a.dot(b), b.dot(a), EPS));
        assert!(approx_eq(a.cross(b), -b.cross(a), EPS));
        assert!(approx_eq(a.cross(a), 0.0, EPS));
    }

    #[test]
    fn cross_sign_follows_counter_clockwise() {
        let x = Vec2::new(1.0, 0.0);
        let y = Vec2::new(0.0, 1.0);
        assert!(approx_eq(x.cross(y), 1.0, EPS));
        assert!(approx_eq(y.cross(x), -1.0, EPS));
    }

    #[test]
    fn length_and_distance() {
        assert!(approx_eq(Vec2::new(3.0, 4.0).length(), 5.0, EPS));
        let d = Point::new(1.0, 1.0).distance(Point::new(4.0, 5.0));
        assert!(approx_eq(d, 5.0, EPS));
    }

    #[test]
    fn normalize_gives_unit_vector() {
        let n = Vec2::new(3.0, 4.0).normalize().unwrap();
        assert!(n.approx_eq(Vec2::new(0.6, 0.8), EPS));
        assert!(approx_eq(n.length(), 1.0, EPS));
    }

    #[test]
    fn normalize_rejects_zero_and_non_finite() {
        assert_eq!(Vec2::ZERO.normalize(), None);
        assert_eq!(Vec2::new(1e-12, 0.0).normalize(), None);
        assert_eq!(Vec2::new(f64::NAN, 1.0).normalize(), None);
        assert_eq!(Vec2::new(f64::INFINITY, 0.0).normalize(), None);
    }

    #[test]
    fn lerp_endpoints_and_midpoint() {
        let a = Point::new(0.0, 0.0);
        let b = Point::new(10.0, -4.0);
        assert!(a.lerp(b, 0.0).approx_eq(a, EPS));
        assert!(a.lerp(b, 1.0).approx_eq(b, EPS));
        assert!(a.lerp(b, 0.5).approx_eq(Point::new(5.0, -2.0), EPS));
    }

    #[test]
    fn distance_to_segment_cases() {
        let a = Point::new(0.0, 0.0);
        let b = Point::new(10.0, 0.0);
        // Perpendicular foot inside the segment.
        assert!(approx_eq(
            distance_to_segment(Point::new(4.0, 3.0), a, b),
            3.0,
            EPS
        ));
        // Beyond either end: distance to the nearest endpoint.
        assert!(approx_eq(
            distance_to_segment(Point::new(13.0, 4.0), a, b),
            5.0,
            EPS
        ));
        assert!(approx_eq(
            distance_to_segment(Point::new(-3.0, -4.0), a, b),
            5.0,
            EPS
        ));
        // Degenerate segment.
        assert!(approx_eq(
            distance_to_segment(Point::new(3.0, 4.0), a, a),
            5.0,
            EPS
        ));
    }

    #[test]
    fn inflate_grows_and_shrinks_without_inverting() {
        let r = rect(0.0, 0.0, 10.0, 4.0);
        assert!(r.inflate(1.0).approx_eq(&rect(-1.0, -1.0, 11.0, 5.0), EPS));
        assert!(r.inflate(-1.0).approx_eq(&rect(1.0, 1.0, 9.0, 3.0), EPS));
        assert!(r.inflate(-3.0).approx_eq(&rect(3.0, 2.0, 7.0, 2.0), EPS));
    }

    #[test]
    fn rect_new_normalizes_corners() {
        let r = rect(10.0, 2.0, -5.0, 8.0);
        assert!(r.min.approx_eq(Point::new(-5.0, 2.0), EPS));
        assert!(r.max.approx_eq(Point::new(10.0, 8.0), EPS));
        assert!(approx_eq(r.width(), 15.0, EPS));
        assert!(approx_eq(r.height(), 6.0, EPS));
        assert!(r.center().approx_eq(Point::new(2.5, 5.0), EPS));
    }

    #[test]
    fn rect_from_points() {
        let pts = [
            Point::new(1.0, 5.0),
            Point::new(-2.0, 3.0),
            Point::new(4.0, -1.0),
        ];
        let r = Rect::from_points(pts).unwrap();
        assert!(r.approx_eq(&rect(-2.0, -1.0, 4.0, 5.0), EPS));
    }

    #[test]
    fn rect_from_no_points_is_none() {
        assert_eq!(Rect::from_points(std::iter::empty()), None);
    }

    #[test]
    fn rect_from_single_point_is_degenerate() {
        let r = Rect::from_points([Point::new(3.0, 4.0)]).unwrap();
        assert!(approx_eq(r.width(), 0.0, EPS));
        assert!(approx_eq(r.height(), 0.0, EPS));
        assert!(r.contains_point(Point::new(3.0, 4.0)));
    }

    #[test]
    fn rect_union_and_expand() {
        let a = rect(0.0, 0.0, 1.0, 1.0);
        let b = rect(2.0, -1.0, 3.0, 0.5);
        assert!(a.union(&b).approx_eq(&rect(0.0, -1.0, 3.0, 1.0), EPS));
        assert!(a.union(&b).approx_eq(&b.union(&a), EPS));
        let e = a.expand_to(Point::new(-1.0, 4.0));
        assert!(e.approx_eq(&rect(-1.0, 0.0, 1.0, 4.0), EPS));
        let unchanged = a.expand_to(Point::new(0.5, 0.5));
        assert!(unchanged.approx_eq(&a, EPS));
    }

    #[test]
    fn contains_point_is_inclusive() {
        let r = rect(0.0, 0.0, 10.0, 5.0);
        assert!(r.contains_point(Point::new(5.0, 2.0)));
        assert!(r.contains_point(Point::new(0.0, 0.0)));
        assert!(r.contains_point(Point::new(10.0, 5.0)));
        assert!(r.contains_point(Point::new(10.0 + EPS / 2.0, 5.0)));
        assert!(!r.contains_point(Point::new(10.001, 2.0)));
        assert!(!r.contains_point(Point::new(5.0, -0.001)));
    }

    #[test]
    fn contains_rect_edge_cases() {
        let outer = rect(0.0, 0.0, 300.0, 200.0);
        assert!(outer.contains_rect(&rect(10.0, 10.0, 20.0, 20.0)));
        assert!(outer.contains_rect(&outer));
        assert!(outer.contains_rect(&rect(0.0, 0.0, 300.0, 1.0)));
        assert!(!outer.contains_rect(&rect(290.0, 10.0, 300.5, 20.0)));
        assert!(!outer.contains_rect(&rect(-1.0, -1.0, 400.0, 400.0)));
        // A zero-area rect on the border is still inside.
        assert!(outer.contains_rect(&rect(300.0, 50.0, 300.0, 50.0)));
    }

    #[test]
    fn intersects_cases() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        assert!(a.intersects(&rect(5.0, 5.0, 15.0, 15.0)));
        assert!(a.intersects(&rect(10.0, 0.0, 20.0, 10.0))); // shared edge
        assert!(a.intersects(&rect(2.0, 2.0, 3.0, 3.0))); // contained
        assert!(!a.intersects(&rect(10.5, 0.0, 20.0, 10.0)));
        assert!(!a.intersects(&rect(0.0, -5.0, 10.0, -0.1)));
    }

    #[test]
    fn rect_translate() {
        let r = rect(0.0, 0.0, 2.0, 3.0).translate(Vec2::new(5.0, -1.0));
        assert!(r.approx_eq(&rect(5.0, -1.0, 7.0, 2.0), EPS));
    }
}
