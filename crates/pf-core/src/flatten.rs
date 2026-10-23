use crate::geometry::{GeometryError, Point};
use crate::path::{Path, Segment};

/// Subdivision depth limit for Béziers. Each level halves the parameter
/// range, so this allows up to 65 536 pieces per segment and guarantees
/// termination on degenerate input.
const MAX_BEZIER_DEPTH: u32 = 16;

const MAX_ARC_STEPS: f64 = 65_536.0;

/// For a closed polyline the last point repeats the first exactly, so
/// visiting the points in order traces the full outline.
#[derive(Debug, Clone, PartialEq)]
pub struct Polyline {
    pub points: Vec<Point>, // mm
    pub closed: bool,
}

/// Flattens one segment. The result starts at `segment.start()` and ends at
/// `segment.end()` exactly; every point of the true curve lies within
/// `tolerance` mm of the polyline.
pub fn flatten_segment(segment: &Segment, tolerance: f64) -> Result<Vec<Point>, GeometryError> {
    validate_tolerance(tolerance)?;
    let mut points = vec![segment.start()];
    append_segment(segment, tolerance, &mut points);
    Ok(points)
}

pub fn flatten_path(path: &Path, tolerance: f64) -> Result<Vec<Polyline>, GeometryError> {
    validate_tolerance(tolerance)?;
    let polylines = path
        .subpaths()
        .iter()
        .map(|subpath| {
            let start = subpath.start();
            let mut points = vec![start];
            for segment in subpath.segments() {
                append_segment(segment, tolerance, &mut points);
            }
            if subpath.is_closed() {
                // Remove the (sub-CONTINUITY_TOL) closing gap.
                if let Some(last) = points.last_mut() {
                    *last = start;
                }
            }
            Polyline {
                points,
                closed: subpath.is_closed(),
            }
        })
        .collect();
    Ok(polylines)
}

fn validate_tolerance(tolerance: f64) -> Result<(), GeometryError> {
    if tolerance.is_finite() && tolerance > 0.0 {
        Ok(())
    } else {
        Err(GeometryError::InvalidTolerance(tolerance))
    }
}

/// Appends the flattened points of `segment`, excluding its start point.
fn append_segment(segment: &Segment, tolerance: f64, out: &mut Vec<Point>) {
    match *segment {
        Segment::Line { to, .. } => out.push(to),
        Segment::QuadBezier { from, ctrl, to } => flatten_quad(from, ctrl, to, tolerance, 0, out),
        Segment::CubicBezier {
            from,
            ctrl1,
            ctrl2,
            to,
        } => flatten_cubic(from, ctrl1, ctrl2, to, tolerance, 0, out),
        Segment::Arc {
            radii, sweep_angle, ..
        } => {
            let steps = arc_steps(radii.x.max(radii.y), sweep_angle, tolerance);
            for i in 1..steps {
                out.push(segment.point_at(i as f64 / steps as f64));
            }
            out.push(segment.end());
        }
    }
}

/// Recursive de Casteljau subdivision. A Bézier lies in the convex hull of
/// its control points, so when every control point is within `tolerance` of
/// the chord, the whole curve is too.
fn flatten_quad(p0: Point, p1: Point, p2: Point, tolerance: f64, depth: u32, out: &mut Vec<Point>) {
    if depth >= MAX_BEZIER_DEPTH || distance_to_segment(p1, p0, p2) <= tolerance {
        out.push(p2);
        return;
    }
    let p01 = p0.lerp(p1, 0.5);
    let p12 = p1.lerp(p2, 0.5);
    let mid = p01.lerp(p12, 0.5);
    flatten_quad(p0, p01, mid, tolerance, depth + 1, out);
    flatten_quad(mid, p12, p2, tolerance, depth + 1, out);
}

fn flatten_cubic(
    p0: Point,
    p1: Point,
    p2: Point,
    p3: Point,
    tolerance: f64,
    depth: u32,
    out: &mut Vec<Point>,
) {
    let flat = distance_to_segment(p1, p0, p3) <= tolerance
        && distance_to_segment(p2, p0, p3) <= tolerance;
    if depth >= MAX_BEZIER_DEPTH || flat {
        out.push(p3);
        return;
    }
    let p01 = p0.lerp(p1, 0.5);
    let p12 = p1.lerp(p2, 0.5);
    let p23 = p2.lerp(p3, 0.5);
    let p012 = p01.lerp(p12, 0.5);
    let p123 = p12.lerp(p23, 0.5);
    let mid = p012.lerp(p123, 0.5);
    flatten_cubic(p0, p01, p012, mid, tolerance, depth + 1, out);
    flatten_cubic(mid, p123, p23, p3, tolerance, depth + 1, out);
}

/// Number of equal angle steps for an arc. A chord spanning angle `θ` on a
/// circle of radius `r` deviates from it by `r·(1 − cos(θ/2))`; an ellipse
/// is an affine image of a circle, so using its larger radius bounds the
/// deviation from above.
fn arc_steps(max_radius: f64, sweep_angle: f64, tolerance: f64) -> usize {
    if max_radius <= tolerance / 2.0 || sweep_angle == 0.0 {
        return 1;
    }
    let max_step = 2.0 * (1.0 - tolerance / max_radius).clamp(-1.0, 1.0).acos();
    let steps = (sweep_angle.abs() / max_step)
        .ceil()
        .clamp(1.0, MAX_ARC_STEPS);
    steps as usize
}

fn distance_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len_sq = ab.dot(ab);
    if len_sq == 0.0 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{EPS, Vec2};
    use std::f64::consts::{FRAC_PI_2, PI, TAU};

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    fn cubic(p0: Point, p1: Point, p2: Point, p3: Point) -> Segment {
        Segment::CubicBezier {
            from: p0,
            ctrl1: p1,
            ctrl2: p2,
            to: p3,
        }
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

    fn distance_to_polyline(point: Point, polyline: &[Point]) -> f64 {
        polyline
            .windows(2)
            .map(|w| distance_to_segment(point, w[0], w[1]))
            .fold(f64::INFINITY, f64::min)
    }

    fn max_deviation(segment: &Segment, polyline: &[Point]) -> f64 {
        let n = 2_000;
        (0..=n)
            .map(|i| distance_to_polyline(segment.point_at(i as f64 / n as f64), polyline))
            .fold(0.0, f64::max)
    }

    fn assert_points_eq(actual: &[Point], expected: &[Point]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?} vs {expected:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert!(a.approx_eq(*e, EPS), "{actual:?} vs {expected:?}");
        }
    }

    fn assert_flattened_within(segment: &Segment, tolerance: f64) {
        let points = flatten_segment(segment, tolerance).unwrap();
        assert!(points[0].approx_eq(segment.start(), EPS), "{segment:?}");
        assert!(
            points[points.len() - 1].approx_eq(segment.end(), EPS),
            "{segment:?}"
        );
        let deviation = max_deviation(segment, &points);
        assert!(
            deviation <= tolerance + 1e-9,
            "deviation {deviation} > tolerance {tolerance} for {segment:?}"
        );
    }

    fn curves() -> Vec<Segment> {
        vec![
            // S-curve
            cubic(p(0.0, 0.0), p(-5.0, 20.0), p(15.0, -20.0), p(10.0, 0.0)),
            // Cusp: the derivative vanishes at t = 0.5.
            cubic(p(0.0, 0.0), p(10.0, 10.0), p(0.0, 10.0), p(10.0, 0.0)),
            // Self-crossing loop
            cubic(p(0.0, 0.0), p(30.0, 15.0), p(-20.0, 15.0), p(10.0, 0.0)),
            // Collinear control points, overshooting the chord
            cubic(p(0.0, 0.0), p(15.0, 0.0), p(-5.0, 0.0), p(10.0, 0.0)),
            // Start == end (teardrop)
            cubic(p(0.0, 0.0), p(10.0, 10.0), p(-10.0, 10.0), p(0.0, 0.0)),
            // Fully degenerate
            cubic(p(3.0, 3.0), p(3.0, 3.0), p(3.0, 3.0), p(3.0, 3.0)),
            Segment::QuadBezier {
                from: p(0.0, 0.0),
                ctrl: p(50.0, 100.0),
                to: p(100.0, 0.0),
            },
            Segment::QuadBezier {
                from: p(0.0, 0.0),
                ctrl: p(-10.0, 0.0),
                to: p(5.0, 0.0),
            },
            // Arcs: small, large, full circle, rotated ellipse clockwise, zero radius
            arc(p(0.0, 0.0), 0.5, 0.5, 0.0, 0.0, PI),
            arc(p(100.0, 100.0), 500.0, 500.0, 0.0, 0.2, 1.0),
            arc(p(0.0, 0.0), 20.0, 20.0, 0.0, 0.0, TAU),
            arc(p(5.0, -3.0), 40.0, 8.0, 0.7, 1.0, -4.5),
            arc(p(1.0, 1.0), 0.0, 0.0, 0.0, 0.0, PI),
        ]
    }

    #[test]
    fn every_curve_stays_within_tolerance() {
        for tolerance in [1.0, 0.1, 0.01, 0.001] {
            for segment in curves() {
                assert_flattened_within(&segment, tolerance);
            }
        }
    }

    #[test]
    fn tolerance_larger_than_curve_gives_single_chord() {
        let small = arc(p(0.0, 0.0), 0.1, 0.1, 0.0, 0.0, FRAC_PI_2);
        assert_eq!(flatten_segment(&small, 1.0).unwrap().len(), 2);
        let tiny_cubic = cubic(p(0.0, 0.0), p(0.1, 0.1), p(0.2, 0.1), p(0.3, 0.0));
        assert_eq!(flatten_segment(&tiny_cubic, 1.0).unwrap().len(), 2);
    }

    #[test]
    fn tighter_tolerance_gives_more_points() {
        for segment in &curves()[..3] {
            let coarse = flatten_segment(segment, 0.5).unwrap().len();
            let fine = flatten_segment(segment, 0.005).unwrap().len();
            assert!(fine > coarse, "{segment:?}: {fine} <= {coarse}");
        }
        let circle = arc(p(0.0, 0.0), 20.0, 20.0, 0.0, 0.0, TAU);
        let coarse = flatten_segment(&circle, 0.5).unwrap().len();
        let fine = flatten_segment(&circle, 0.005).unwrap().len();
        assert!(fine > coarse);
    }

    #[test]
    fn line_flattens_to_its_endpoints() {
        let line = Segment::Line {
            from: p(1.0, 2.0),
            to: p(3.0, 4.0),
        };
        assert_points_eq(
            &flatten_segment(&line, 0.01).unwrap(),
            &[p(1.0, 2.0), p(3.0, 4.0)],
        );
    }

    #[test]
    fn invalid_tolerance_is_rejected() {
        let line = Segment::Line {
            from: p(0.0, 0.0),
            to: p(1.0, 0.0),
        };
        for tolerance in [0.0, -0.1, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                flatten_segment(&line, tolerance),
                Err(GeometryError::InvalidTolerance(_))
            ));
            assert!(matches!(
                flatten_path(&Path::default(), tolerance),
                Err(GeometryError::InvalidTolerance(_))
            ));
        }
    }

    #[test]
    fn closed_path_repeats_start_exactly() {
        let square = Path::builder()
            .move_to(p(0.0, 0.0))
            .unwrap()
            .line_to(p(10.0, 0.0))
            .unwrap()
            .line_to(p(10.0, 10.0))
            .unwrap()
            .line_to(p(0.0, 10.0))
            .unwrap()
            .close()
            .unwrap()
            .build()
            .unwrap();
        let polylines = flatten_path(&square, 0.01).unwrap();
        assert_eq!(polylines.len(), 1);
        let square = &polylines[0];
        assert!(square.closed);
        assert_points_eq(
            &square.points,
            &[
                p(0.0, 0.0),
                p(10.0, 0.0),
                p(10.0, 10.0),
                p(0.0, 10.0),
                p(0.0, 0.0),
            ],
        );
    }

    #[test]
    fn path_with_mixed_segments_stays_within_tolerance() {
        let tolerance = 0.01;
        let path = Path::builder()
            .move_to(p(0.0, 0.0))
            .unwrap()
            .cubic_to(p(5.0, 20.0), p(15.0, -20.0), p(20.0, 0.0))
            .unwrap()
            .arc_to(p(20.0, 5.0), Vec2::new(5.0, 5.0), 0.0, -FRAC_PI_2, PI)
            .unwrap()
            .close()
            .unwrap()
            .move_to(p(50.0, 50.0))
            .unwrap()
            .quad_to(p(60.0, 70.0), p(70.0, 50.0))
            .unwrap()
            .build()
            .unwrap();
        let polylines = flatten_path(&path, tolerance).unwrap();
        assert_eq!(polylines.len(), 2);
        assert!(polylines[0].closed);
        assert!(!polylines[1].closed);
        for (subpath, polyline) in path.subpaths().iter().zip(&polylines) {
            assert!(polyline.points[0].approx_eq(subpath.start(), EPS));
            for segment in subpath.segments() {
                assert!(max_deviation(segment, &polyline.points) <= tolerance + 1e-9);
            }
        }
    }

    #[test]
    fn empty_path_flattens_to_nothing() {
        assert!(flatten_path(&Path::default(), 0.1).unwrap().is_empty());
    }

    #[test]
    fn distance_to_segment_cases() {
        let a = p(0.0, 0.0);
        let b = p(10.0, 0.0);
        assert!((distance_to_segment(p(5.0, 3.0), a, b) - 3.0).abs() < EPS);
        assert!((distance_to_segment(p(-3.0, 4.0), a, b) - 5.0).abs() < EPS);
        assert!((distance_to_segment(p(13.0, 4.0), a, b) - 5.0).abs() < EPS);
        assert!((distance_to_segment(p(3.0, 4.0), a, a) - 5.0).abs() < EPS);
    }
}
