use std::f64::consts::{PI, TAU};

use crate::geometry::{GeometryError, Point, Rect, Vec2};
use crate::transform::Transform;

/// Maximum gap (mm) between consecutive segment endpoints that still counts
/// as connected. Larger than `EPS` to absorb float noise from importers and
/// transforms.
pub const CONTINUITY_TOL: f64 = 1e-6;

/// Below this magnitude a polynomial coefficient is treated as zero.
const COEFF_TINY: f64 = 1e-12;

/// One piece of a path. Every variant carries its own start point so it can
/// be bounded, transformed, and flattened on its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    Line {
        from: Point,
        to: Point,
    },
    QuadBezier {
        from: Point,
        ctrl: Point,
        to: Point,
    },
    CubicBezier {
        from: Point,
        ctrl1: Point,
        ctrl2: Point,
        to: Point,
    },
    /// Elliptical arc in centre form. The point at angle `θ` is
    /// `center + R(x_rotation) · (radii.x·cos θ, radii.y·sin θ)`.
    /// The arc runs from `start_angle` to `start_angle + sweep_angle`;
    /// a positive sweep is counter-clockwise.
    Arc {
        center: Point,
        radii: Vec2,      // mm, both >= 0
        x_rotation: f64,  // radians
        start_angle: f64, // radians
        sweep_angle: f64, // radians
    },
}

impl Segment {
    pub fn start(&self) -> Point {
        self.point_at(0.0)
    }

    pub fn end(&self) -> Point {
        self.point_at(1.0)
    }

    /// Point at parameter `t` in [0, 1]. For arcs, `t` is linear in angle.
    pub fn point_at(&self, t: f64) -> Point {
        match *self {
            Segment::Line { from, to } => {
                if t == 1.0 {
                    to
                } else {
                    from.lerp(to, t)
                }
            }
            Segment::QuadBezier { from, ctrl, to } => quad_point(from, ctrl, to, t),
            Segment::CubicBezier {
                from,
                ctrl1,
                ctrl2,
                to,
            } => cubic_point(from, ctrl1, ctrl2, to, t),
            Segment::Arc {
                center,
                radii,
                x_rotation,
                start_angle,
                sweep_angle,
            } => ellipse_point(center, radii, x_rotation, start_angle + t * sweep_angle),
        }
    }

    /// The segment mapped through `t`. Exact for every variant: an affine
    /// image of an elliptical arc is again an elliptical arc.
    pub fn transform(&self, t: &Transform) -> Segment {
        match *self {
            Segment::Line { from, to } => Segment::Line {
                from: t.apply_point(from),
                to: t.apply_point(to),
            },
            Segment::QuadBezier { from, ctrl, to } => Segment::QuadBezier {
                from: t.apply_point(from),
                ctrl: t.apply_point(ctrl),
                to: t.apply_point(to),
            },
            Segment::CubicBezier {
                from,
                ctrl1,
                ctrl2,
                to,
            } => Segment::CubicBezier {
                from: t.apply_point(from),
                ctrl1: t.apply_point(ctrl1),
                ctrl2: t.apply_point(ctrl2),
                to: t.apply_point(to),
            },
            Segment::Arc {
                center,
                radii,
                x_rotation,
                start_angle,
                sweep_angle,
            } => transform_arc(t, center, radii, x_rotation, start_angle, sweep_angle),
        }
    }

    /// Tight axis-aligned bounds, computed from the curve's extrema rather
    /// than its control points.
    pub fn bounds(&self) -> Rect {
        let ends = Rect::new(self.start(), self.end());
        let extrema: [Option<f64>; 4] = match *self {
            Segment::Line { .. } => [None; 4],
            Segment::QuadBezier { from, ctrl, to } => [
                quad_extremum(from.x, ctrl.x, to.x),
                quad_extremum(from.y, ctrl.y, to.y),
                None,
                None,
            ],
            Segment::CubicBezier {
                from,
                ctrl1,
                ctrl2,
                to,
            } => {
                let [x0, x1] = cubic_extrema(from.x, ctrl1.x, ctrl2.x, to.x);
                let [y0, y1] = cubic_extrema(from.y, ctrl1.y, ctrl2.y, to.y);
                [x0, x1, y0, y1]
            }
            Segment::Arc {
                center,
                radii,
                x_rotation,
                start_angle,
                sweep_angle,
            } => {
                return arc_extreme_angles(radii, x_rotation)
                    .into_iter()
                    .filter(|&angle| angle_in_sweep(angle, start_angle, sweep_angle))
                    .fold(ends, |r, angle| {
                        r.expand_to(ellipse_point(center, radii, x_rotation, angle))
                    });
            }
        };
        extrema
            .into_iter()
            .flatten()
            .filter(|&t| t > 0.0 && t < 1.0)
            .fold(ends, |r, t| r.expand_to(self.point_at(t)))
    }
}

/// Invariants (checked by `SubPath::new`): at least one segment, all values
/// finite, each segment starts within `CONTINUITY_TOL` of where the previous
/// one ends, and a closed sub-path ends within `CONTINUITY_TOL` of its start.
#[derive(Debug, Clone, PartialEq)]
pub struct SubPath {
    segments: Vec<Segment>,
    closed: bool,
}

impl SubPath {
    pub fn new(segments: Vec<Segment>, closed: bool) -> Result<SubPath, GeometryError> {
        let first = segments.first().ok_or(GeometryError::EmptySubPath)?;
        for segment in &segments {
            validate_segment(segment)?;
        }
        for (index, pair) in segments.windows(2).enumerate() {
            let gap = pair[0].end().distance(pair[1].start());
            if gap > CONTINUITY_TOL {
                return Err(GeometryError::Discontinuous {
                    index: index + 1,
                    gap,
                });
            }
        }
        if closed {
            let last = segments.last().unwrap_or(first);
            let gap = last.end().distance(first.start());
            if gap > CONTINUITY_TOL {
                return Err(GeometryError::NotClosed { gap });
            }
        }
        Ok(SubPath { segments, closed })
    }

    /// Builds a sub-path from segments the caller has constructed to satisfy
    /// the invariants (used for primitive shapes built from validated input).
    pub(crate) fn new_unchecked(segments: Vec<Segment>, closed: bool) -> SubPath {
        debug_assert!(SubPath::new(segments.clone(), closed).is_ok());
        SubPath { segments, closed }
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn start(&self) -> Point {
        // Never empty: guaranteed by `SubPath::new`.
        self.segments[0].start()
    }

    pub fn end(&self) -> Point {
        self.segments[self.segments.len() - 1].end()
    }

    pub fn transform(&self, t: &Transform) -> SubPath {
        SubPath {
            segments: self.segments.iter().map(|s| s.transform(t)).collect(),
            closed: self.closed,
        }
    }

    pub fn bounds(&self) -> Rect {
        let first = self.segments[0].bounds();
        self.segments[1..]
            .iter()
            .fold(first, |r, s| r.union(&s.bounds()))
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Path {
    subpaths: Vec<SubPath>,
}

impl Path {
    pub fn new(subpaths: Vec<SubPath>) -> Path {
        Path { subpaths }
    }

    pub fn builder() -> PathBuilder {
        PathBuilder::default()
    }

    pub fn subpaths(&self) -> &[SubPath] {
        &self.subpaths
    }

    pub fn is_empty(&self) -> bool {
        self.subpaths.is_empty()
    }

    pub fn transform(&self, t: &Transform) -> Path {
        Path {
            subpaths: self.subpaths.iter().map(|s| s.transform(t)).collect(),
        }
    }

    pub fn bounds(&self) -> Option<Rect> {
        self.subpaths
            .iter()
            .map(SubPath::bounds)
            .reduce(|a, b| a.union(&b))
    }
}

/// Step-by-step path construction in the style of SVG path commands.
///
/// ```
/// use pf_core::{Path, Point};
/// let square = Path::builder()
///     .move_to(Point::new(0.0, 0.0))?
///     .line_to(Point::new(10.0, 0.0))?
///     .line_to(Point::new(10.0, 10.0))?
///     .line_to(Point::new(0.0, 10.0))?
///     .close()?
///     .build()?;
/// assert_eq!(square.subpaths().len(), 1);
/// # Ok::<(), pf_core::GeometryError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PathBuilder {
    subpaths: Vec<SubPath>,
    segments: Vec<Segment>,
    subpath_start: Option<Point>,
    current: Option<Point>,
}

impl PathBuilder {
    /// Ends the sub-path in progress (left open) and starts a new one at `p`.
    pub fn move_to(mut self, p: Point) -> Result<Self, GeometryError> {
        validate_point(p)?;
        self.finish_open()?;
        self.subpath_start = Some(p);
        self.current = Some(p);
        Ok(self)
    }

    pub fn line_to(self, to: Point) -> Result<Self, GeometryError> {
        let from = self.current.ok_or(GeometryError::NoCurrentPoint)?;
        self.push(Segment::Line { from, to })
    }

    pub fn quad_to(self, ctrl: Point, to: Point) -> Result<Self, GeometryError> {
        let from = self.current.ok_or(GeometryError::NoCurrentPoint)?;
        self.push(Segment::QuadBezier { from, ctrl, to })
    }

    pub fn cubic_to(self, ctrl1: Point, ctrl2: Point, to: Point) -> Result<Self, GeometryError> {
        let from = self.current.ok_or(GeometryError::NoCurrentPoint)?;
        self.push(Segment::CubicBezier {
            from,
            ctrl1,
            ctrl2,
            to,
        })
    }

    /// Appends an elliptical arc (see `Segment::Arc`). With no current point
    /// the arc starts a new sub-path; otherwise it must begin at the current
    /// point.
    pub fn arc_to(
        mut self,
        center: Point,
        radii: Vec2,
        x_rotation: f64,
        start_angle: f64,
        sweep_angle: f64,
    ) -> Result<Self, GeometryError> {
        let arc = Segment::Arc {
            center,
            radii,
            x_rotation,
            start_angle,
            sweep_angle,
        };
        validate_segment(&arc)?;
        match self.current {
            None => self.subpath_start = Some(arc.start()),
            Some(current) => {
                let gap = current.distance(arc.start());
                if gap > CONTINUITY_TOL {
                    return Err(GeometryError::Discontinuous {
                        index: self.segments.len(),
                        gap,
                    });
                }
            }
        }
        self.push(arc)
    }

    /// Closes the sub-path in progress, adding a line back to its start if
    /// needed. A new sub-path must then begin with `move_to` (or `arc_to`).
    pub fn close(mut self) -> Result<Self, GeometryError> {
        let start = self.subpath_start.ok_or(GeometryError::NoCurrentPoint)?;
        let end = self.current.ok_or(GeometryError::NoCurrentPoint)?;
        if self.segments.is_empty() {
            return Err(GeometryError::EmptySubPath);
        }
        if end.distance(start) > CONTINUITY_TOL {
            self.segments.push(Segment::Line {
                from: end,
                to: start,
            });
        }
        let segments = std::mem::take(&mut self.segments);
        self.subpaths.push(SubPath::new(segments, true)?);
        self.subpath_start = None;
        self.current = None;
        Ok(self)
    }

    /// Finishes the path. A trailing `move_to` with no segments is dropped.
    pub fn build(mut self) -> Result<Path, GeometryError> {
        self.finish_open()?;
        Ok(Path::new(self.subpaths))
    }

    fn push(mut self, segment: Segment) -> Result<Self, GeometryError> {
        validate_segment(&segment)?;
        self.current = Some(segment.end());
        self.segments.push(segment);
        Ok(self)
    }

    fn finish_open(&mut self) -> Result<(), GeometryError> {
        if !self.segments.is_empty() {
            let segments = std::mem::take(&mut self.segments);
            self.subpaths.push(SubPath::new(segments, false)?);
        }
        Ok(())
    }
}

fn validate_point(p: Point) -> Result<(), GeometryError> {
    if p.x.is_finite() && p.y.is_finite() {
        Ok(())
    } else {
        Err(GeometryError::NonFinite)
    }
}

fn validate_segment(segment: &Segment) -> Result<(), GeometryError> {
    match *segment {
        Segment::Line { from, to } => {
            validate_point(from)?;
            validate_point(to)
        }
        Segment::QuadBezier { from, ctrl, to } => {
            validate_point(from)?;
            validate_point(ctrl)?;
            validate_point(to)
        }
        Segment::CubicBezier {
            from,
            ctrl1,
            ctrl2,
            to,
        } => {
            validate_point(from)?;
            validate_point(ctrl1)?;
            validate_point(ctrl2)?;
            validate_point(to)
        }
        Segment::Arc {
            center,
            radii,
            x_rotation,
            start_angle,
            sweep_angle,
        } => {
            validate_point(center)?;
            let radii_ok =
                radii.x.is_finite() && radii.y.is_finite() && radii.x >= 0.0 && radii.y >= 0.0;
            if !radii_ok {
                return Err(GeometryError::InvalidArcRadii {
                    rx: radii.x,
                    ry: radii.y,
                });
            }
            if x_rotation.is_finite() && start_angle.is_finite() && sweep_angle.is_finite() {
                Ok(())
            } else {
                Err(GeometryError::NonFinite)
            }
        }
    }
}

fn quad_point(p0: Point, p1: Point, p2: Point, t: f64) -> Point {
    let mt = 1.0 - t;
    let (w0, w1, w2) = (mt * mt, 2.0 * mt * t, t * t);
    Point::new(
        w0 * p0.x + w1 * p1.x + w2 * p2.x,
        w0 * p0.y + w1 * p1.y + w2 * p2.y,
    )
}

fn cubic_point(p0: Point, p1: Point, p2: Point, p3: Point, t: f64) -> Point {
    let mt = 1.0 - t;
    let (w0, w1, w2, w3) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
    Point::new(
        w0 * p0.x + w1 * p1.x + w2 * p2.x + w3 * p3.x,
        w0 * p0.y + w1 * p1.y + w2 * p2.y + w3 * p3.y,
    )
}

pub(crate) fn ellipse_point(center: Point, radii: Vec2, x_rotation: f64, angle: f64) -> Point {
    let (sin_r, cos_r) = x_rotation.sin_cos();
    let (sin_a, cos_a) = angle.sin_cos();
    let lx = radii.x * cos_a;
    let ly = radii.y * sin_a;
    Point::new(
        center.x + lx * cos_r - ly * sin_r,
        center.y + lx * sin_r + ly * cos_r,
    )
}

fn quad_extremum(p0: f64, p1: f64, p2: f64) -> Option<f64> {
    let denom = p0 - 2.0 * p1 + p2;
    if denom.abs() < COEFF_TINY {
        None
    } else {
        Some((p0 - p1) / denom)
    }
}

fn cubic_extrema(p0: f64, p1: f64, p2: f64, p3: f64) -> [Option<f64>; 2] {
    // B'(t)/3 = (a - 2b + c)·t² + 2(b - a)·t + a
    let a = p1 - p0;
    let b = p2 - p1;
    let c = p3 - p2;
    quadratic_roots(a - 2.0 * b + c, 2.0 * (b - a), a)
}

/// Real roots of `a·t² + b·t + c = 0`, falling back to the linear case.
fn quadratic_roots(a: f64, b: f64, c: f64) -> [Option<f64>; 2] {
    if a.abs() < COEFF_TINY {
        if b.abs() < COEFF_TINY {
            return [None, None];
        }
        return [Some(-c / b), None];
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return [None, None];
    }
    let root = disc.sqrt();
    [Some((-b + root) / (2.0 * a)), Some((-b - root) / (2.0 * a))]
}

fn arc_extreme_angles(radii: Vec2, x_rotation: f64) -> [f64; 4] {
    let (sin_r, cos_r) = x_rotation.sin_cos();
    // dx/dθ = 0 and dy/dθ = 0 for the rotated ellipse.
    let tx = f64::atan2(-radii.y * sin_r, radii.x * cos_r);
    let ty = f64::atan2(radii.y * cos_r, radii.x * sin_r);
    [tx, tx + PI, ty, ty + PI]
}

fn angle_in_sweep(angle: f64, start: f64, sweep: f64) -> bool {
    if sweep.abs() >= TAU {
        return true;
    }
    let offset = if sweep >= 0.0 {
        (angle - start).rem_euclid(TAU)
    } else {
        (start - angle).rem_euclid(TAU)
    };
    offset <= sweep.abs()
}

/// Maps an arc through an affine transform.
///
/// The arc is `center + M·(cos θ, sin θ)` with `M = R(x_rotation)·diag(rx, ry)`.
/// After the transform the matrix is `M' = L·M` (L = linear part), which a
/// closed-form 2×2 SVD splits into `R(φ)·diag(sx, sy)·R(β)`. The rotation
/// `R(β)` shifts the angle parameter; a negative `sy` (mirror) reverses it.
fn transform_arc(
    t: &Transform,
    center: Point,
    radii: Vec2,
    x_rotation: f64,
    start_angle: f64,
    sweep_angle: f64,
) -> Segment {
    let (sin_r, cos_r) = x_rotation.sin_cos();
    let col0 = t.apply_vector(Vec2::new(radii.x * cos_r, radii.x * sin_r));
    let col1 = t.apply_vector(Vec2::new(-radii.y * sin_r, radii.y * cos_r));
    let (p, q, r, s) = (col0.x, col1.x, col0.y, col1.y);

    let e = (p + s) / 2.0;
    let f = (p - s) / 2.0;
    let g = (r + q) / 2.0;
    let h = (r - q) / 2.0;
    let sx = e.hypot(h) + f.hypot(g);
    let sy = e.hypot(h) - f.hypot(g);
    let a1 = g.atan2(f);
    let a2 = h.atan2(e);
    let beta = (a2 - a1) / 2.0;
    let phi = (a2 + a1) / 2.0;

    let (start_angle, sweep_angle) = if sy >= 0.0 {
        (start_angle + beta, sweep_angle)
    } else {
        (-(start_angle + beta), -sweep_angle)
    };
    Segment::Arc {
        center: t.apply_point(center),
        radii: Vec2::new(sx, sy.abs()),
        x_rotation: phi,
        start_angle,
        sweep_angle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{EPS, approx_eq};
    use std::f64::consts::FRAC_PI_2;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn arc(center: Point, rx: f64, ry: f64, rot: f64, start: f64, sweep: f64) -> Segment {
        Segment::Arc {
            center,
            radii: Vec2::new(rx, ry),
            x_rotation: rot,
            start_angle: start,
            sweep_angle: sweep,
        }
    }

    fn sampled_bounds(segment: &Segment) -> Rect {
        let n = 20_000;
        Rect::from_points((0..=n).map(|i| segment.point_at(i as f64 / n as f64))).unwrap()
    }

    fn sample_segments() -> Vec<Segment> {
        vec![
            Segment::Line {
                from: p(1.0, 2.0),
                to: p(-3.0, 5.0),
            },
            Segment::QuadBezier {
                from: p(0.0, 0.0),
                ctrl: p(5.0, 10.0),
                to: p(10.0, 0.0),
            },
            Segment::CubicBezier {
                from: p(0.0, 0.0),
                ctrl1: p(-5.0, 20.0),
                ctrl2: p(15.0, -20.0),
                to: p(10.0, 0.0),
            }, // S-curve
            Segment::CubicBezier {
                from: p(0.0, 0.0),
                ctrl1: p(10.0, 10.0),
                ctrl2: p(0.0, 10.0),
                to: p(10.0, 0.0),
            }, // loop
            arc(p(2.0, 3.0), 4.0, 1.5, 0.4, 0.3, 2.5),
            arc(p(-1.0, 0.0), 2.0, 2.0, 0.0, 1.0, -4.0),
            arc(p(0.0, 0.0), 3.0, 1.0, -0.8, 0.0, TAU),
        ]
    }

    fn sample_transforms() -> Vec<Transform> {
        vec![
            Transform::identity(),
            Transform::translate(7.0, -2.0),
            Transform::rotate(0.9),
            Transform::scale(2.0, 0.5),
            Transform::scale(-1.0, 1.0),
            Transform::scale(1.0, -3.0),
            Transform {
                a: 1.0,
                b: 0.3,
                c: 0.8,
                d: 1.2,
                e: 5.0,
                f: 1.0,
            }, // shear
            Transform {
                a: -0.5,
                b: 1.1,
                c: 0.7,
                d: 0.2,
                e: 0.0,
                f: 0.0,
            }, // mirrored shear
        ]
    }

    #[test]
    fn segment_endpoints() {
        let cubic = Segment::CubicBezier {
            from: p(0.0, 0.0),
            ctrl1: p(1.0, 2.0),
            ctrl2: p(3.0, 2.0),
            to: p(4.0, 0.0),
        };
        assert!(cubic.start().approx_eq(p(0.0, 0.0), EPS));
        assert!(cubic.end().approx_eq(p(4.0, 0.0), EPS));
        assert!(cubic.point_at(0.5).approx_eq(p(2.0, 1.5), EPS));

        let quarter = arc(p(1.0, 1.0), 2.0, 2.0, 0.0, 0.0, FRAC_PI_2);
        assert!(quarter.start().approx_eq(p(3.0, 1.0), EPS));
        assert!(quarter.end().approx_eq(p(1.0, 3.0), EPS));
    }

    #[test]
    fn line_and_quad_bounds() {
        let line = Segment::Line {
            from: p(3.0, -1.0),
            to: p(-2.0, 4.0),
        };
        assert!(
            line.bounds()
                .approx_eq(&Rect::new(p(-2.0, -1.0), p(3.0, 4.0)), EPS)
        );

        let quad = Segment::QuadBezier {
            from: p(0.0, 0.0),
            ctrl: p(5.0, 10.0),
            to: p(10.0, 0.0),
        };
        // Apex at t = 0.5 is y = 5, not the control point's 10.
        assert!(
            quad.bounds()
                .approx_eq(&Rect::new(p(0.0, 0.0), p(10.0, 5.0)), EPS)
        );
    }

    #[test]
    fn cubic_bounds_are_tight_not_control_hull() {
        let cubic = Segment::CubicBezier {
            from: p(0.0, 0.0),
            ctrl1: p(-5.0, 20.0),
            ctrl2: p(15.0, -20.0),
            to: p(10.0, 0.0),
        };
        let b = cubic.bounds();
        let hull =
            Rect::from_points([p(0.0, 0.0), p(-5.0, 20.0), p(15.0, -20.0), p(10.0, 0.0)]).unwrap();
        assert!(b.height() < hull.height());
        assert!(b.approx_eq(&sampled_bounds(&cubic), 1e-6));
    }

    #[test]
    fn quarter_half_and_full_circle_bounds() {
        let c = p(0.0, 0.0);
        let quarter = arc(c, 1.0, 1.0, 0.0, 0.0, FRAC_PI_2);
        assert!(
            quarter
                .bounds()
                .approx_eq(&Rect::new(p(0.0, 0.0), p(1.0, 1.0)), EPS)
        );
        let half = arc(c, 1.0, 1.0, 0.0, 0.0, PI);
        assert!(
            half.bounds()
                .approx_eq(&Rect::new(p(-1.0, 0.0), p(1.0, 1.0)), EPS)
        );
        let full = arc(c, 1.0, 1.0, 0.0, 0.0, TAU);
        assert!(
            full.bounds()
                .approx_eq(&Rect::new(p(-1.0, -1.0), p(1.0, 1.0)), EPS)
        );
        let clockwise = arc(c, 1.0, 1.0, 0.0, 0.0, -FRAC_PI_2);
        assert!(
            clockwise
                .bounds()
                .approx_eq(&Rect::new(p(0.0, -1.0), p(1.0, 0.0)), EPS)
        );
    }

    #[test]
    fn rotated_ellipse_bounds_match_sampling() {
        let full = arc(p(5.0, -2.0), 6.0, 2.0, 0.6, 0.0, TAU);
        assert!(full.bounds().approx_eq(&sampled_bounds(&full), 1e-6));
        let partial = arc(p(0.0, 0.0), 4.0, 1.0, -1.2, 2.0, -3.5);
        assert!(partial.bounds().approx_eq(&sampled_bounds(&partial), 1e-6));
    }

    #[test]
    fn every_sample_bound_matches_sampling() {
        for s in sample_segments() {
            assert!(s.bounds().approx_eq(&sampled_bounds(&s), 1e-6), "{s:?}");
        }
    }

    #[test]
    fn transform_matches_pointwise_mapping() {
        for s in sample_segments() {
            for t in sample_transforms() {
                let mapped = s.transform(&t);
                for i in 0..=20 {
                    let u = i as f64 / 20.0;
                    let expected = t.apply_point(s.point_at(u));
                    assert!(
                        mapped.point_at(u).approx_eq(expected, 1e-9),
                        "{s:?} under {t:?} at t={u}"
                    );
                }
            }
        }
    }

    #[test]
    fn transformed_arc_radii_are_non_negative() {
        for t in sample_transforms() {
            if let Segment::Arc { radii, .. } =
                arc(p(0.0, 0.0), 3.0, 1.0, 0.5, 0.0, 1.0).transform(&t)
            {
                assert!(radii.x >= 0.0 && radii.y >= 0.0);
            }
        }
    }

    #[test]
    fn transform_then_bounds_is_consistent() {
        let path = Path::builder()
            .move_to(p(0.0, 0.0))
            .unwrap()
            .cubic_to(p(2.0, 6.0), p(8.0, -6.0), p(10.0, 0.0))
            .unwrap()
            .arc_to(p(10.0, 2.0), Vec2::new(2.0, 2.0), 0.0, -FRAC_PI_2, PI)
            .unwrap()
            .close()
            .unwrap()
            .build()
            .unwrap();
        for t in sample_transforms() {
            let moved = path.transform(&t);
            let expected = moved.subpaths()[0]
                .segments()
                .iter()
                .map(sampled_bounds)
                .reduce(|a, b| a.union(&b))
                .unwrap();
            assert!(moved.bounds().unwrap().approx_eq(&expected, 1e-6));
        }
    }

    #[test]
    fn subpath_accepts_connected_segments() {
        let sub = SubPath::new(
            vec![
                Segment::Line {
                    from: p(0.0, 0.0),
                    to: p(1.0, 0.0),
                },
                Segment::Line {
                    from: p(1.0, 0.0),
                    to: p(1.0, 1.0),
                },
                Segment::Line {
                    from: p(1.0, 1.0),
                    to: p(0.0, 0.0),
                },
            ],
            true,
        )
        .unwrap();
        assert!(sub.is_closed());
        assert!(sub.start().approx_eq(p(0.0, 0.0), EPS));
        assert!(sub.end().approx_eq(p(0.0, 0.0), EPS));
        assert!(
            sub.bounds()
                .approx_eq(&Rect::new(p(0.0, 0.0), p(1.0, 1.0)), EPS)
        );
    }

    #[test]
    fn subpath_accepts_gap_within_tolerance() {
        let sub = SubPath::new(
            vec![
                Segment::Line {
                    from: p(0.0, 0.0),
                    to: p(1.0, 0.0),
                },
                Segment::Line {
                    from: p(1.0 + CONTINUITY_TOL / 2.0, 0.0),
                    to: p(2.0, 0.0),
                },
            ],
            false,
        );
        assert!(sub.is_ok());
    }

    #[test]
    fn subpath_rejects_gap() {
        let err = SubPath::new(
            vec![
                Segment::Line {
                    from: p(0.0, 0.0),
                    to: p(1.0, 0.0),
                },
                Segment::Line {
                    from: p(1.5, 0.0),
                    to: p(2.0, 0.0),
                },
            ],
            false,
        )
        .unwrap_err();
        match err {
            GeometryError::Discontinuous { index, gap } => {
                assert_eq!(index, 1);
                assert!(approx_eq(gap, 0.5, EPS));
            }
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn subpath_rejects_empty_unclosed_and_invalid() {
        assert_eq!(
            SubPath::new(vec![], false),
            Err(GeometryError::EmptySubPath)
        );

        let open_line = vec![Segment::Line {
            from: p(0.0, 0.0),
            to: p(1.0, 0.0),
        }];
        assert!(matches!(
            SubPath::new(open_line, true),
            Err(GeometryError::NotClosed { .. })
        ));

        let nan = vec![Segment::Line {
            from: p(f64::NAN, 0.0),
            to: p(1.0, 0.0),
        }];
        assert_eq!(SubPath::new(nan, false), Err(GeometryError::NonFinite));

        let bad_arc = vec![arc(p(0.0, 0.0), -1.0, 1.0, 0.0, 0.0, 1.0)];
        assert!(matches!(
            SubPath::new(bad_arc, false),
            Err(GeometryError::InvalidArcRadii { .. })
        ));
    }

    #[test]
    fn builder_close_adds_closing_line_only_when_needed() {
        let triangle = Path::builder()
            .move_to(p(0.0, 0.0))
            .unwrap()
            .line_to(p(4.0, 0.0))
            .unwrap()
            .line_to(p(0.0, 3.0))
            .unwrap()
            .close()
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(triangle.subpaths()[0].segments().len(), 3);

        let already_closed = Path::builder()
            .move_to(p(0.0, 0.0))
            .unwrap()
            .line_to(p(4.0, 0.0))
            .unwrap()
            .line_to(p(0.0, 0.0))
            .unwrap()
            .close()
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(already_closed.subpaths()[0].segments().len(), 2);
    }

    #[test]
    fn builder_multiple_subpaths_and_trailing_move() {
        let path = Path::builder()
            .move_to(p(0.0, 0.0))
            .unwrap()
            .line_to(p(1.0, 0.0))
            .unwrap()
            .move_to(p(5.0, 5.0))
            .unwrap()
            .quad_to(p(6.0, 7.0), p(7.0, 5.0))
            .unwrap()
            .move_to(p(9.0, 9.0))
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(path.subpaths().len(), 2);
        assert!(!path.subpaths()[0].is_closed());
        let bounds = path.bounds().unwrap();
        assert!(bounds.approx_eq(&Rect::new(p(0.0, 0.0), p(7.0, 6.0)), EPS));
    }

    #[test]
    fn builder_requires_current_point() {
        assert_eq!(
            Path::builder().line_to(p(1.0, 1.0)),
            Err(GeometryError::NoCurrentPoint)
        );
        let after_close = Path::builder()
            .move_to(p(0.0, 0.0))
            .unwrap()
            .line_to(p(1.0, 0.0))
            .unwrap()
            .close()
            .unwrap()
            .line_to(p(2.0, 2.0));
        assert_eq!(after_close, Err(GeometryError::NoCurrentPoint));
        assert_eq!(
            Path::builder().move_to(p(0.0, 0.0)).unwrap().close(),
            Err(GeometryError::EmptySubPath)
        );
    }

    #[test]
    fn builder_arc_starts_subpath_or_must_connect() {
        let circle = Path::builder()
            .arc_to(p(0.0, 0.0), Vec2::new(2.0, 2.0), 0.0, 0.0, TAU)
            .unwrap()
            .close()
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(circle.subpaths()[0].segments().len(), 1);
        assert!(circle.subpaths()[0].start().approx_eq(p(2.0, 0.0), EPS));

        let disconnected = Path::builder().move_to(p(10.0, 10.0)).unwrap().arc_to(
            p(0.0, 0.0),
            Vec2::new(1.0, 1.0),
            0.0,
            0.0,
            PI,
        );
        assert!(matches!(
            disconnected,
            Err(GeometryError::Discontinuous { index: 0, .. })
        ));
    }

    #[test]
    fn builder_rejects_non_finite_points() {
        assert_eq!(
            Path::builder().move_to(p(f64::INFINITY, 0.0)),
            Err(GeometryError::NonFinite)
        );
        let bad = Path::builder()
            .move_to(p(0.0, 0.0))
            .unwrap()
            .line_to(p(1.0, f64::NAN));
        assert_eq!(bad, Err(GeometryError::NonFinite));
    }

    #[test]
    fn empty_path_has_no_bounds() {
        assert_eq!(Path::default().bounds(), None);
        assert!(Path::builder().build().unwrap().is_empty());
    }
}
