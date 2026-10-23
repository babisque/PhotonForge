//! Selection handles and the transforms produced by dragging them. All
//! positions are in document millimeters; the app converts its pixel
//! sizes with the viewport zoom.

use crate::geometry::{EPS, Point, Rect};
use crate::transform::Transform;

/// A drag never shrinks a selection below this size (mm), and never
/// mirrors it.
pub const MIN_SIZE_MM: f64 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Handle {
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
    /// Sits above the top edge, `rotate_offset_mm` away.
    Rotate,
}

impl Handle {
    pub const ALL: [Handle; 9] = [
        Handle::N,
        Handle::NE,
        Handle::E,
        Handle::SE,
        Handle::S,
        Handle::SW,
        Handle::W,
        Handle::NW,
        Handle::Rotate,
    ];

    /// Unit direction from the bounds center: x is -1 (west), 0 or 1
    /// (east); y is -1 (south), 0 or 1 (north). Rotate has none.
    fn direction(self) -> Option<(f64, f64)> {
        let dir = match self {
            Handle::N => (0.0, 1.0),
            Handle::NE => (1.0, 1.0),
            Handle::E => (1.0, 0.0),
            Handle::SE => (1.0, -1.0),
            Handle::S => (0.0, -1.0),
            Handle::SW => (-1.0, -1.0),
            Handle::W => (-1.0, 0.0),
            Handle::NW => (-1.0, 1.0),
            Handle::Rotate => return None,
        };
        Some(dir)
    }
}

pub fn handle_position(bounds: Rect, handle: Handle, rotate_offset_mm: f64) -> Point {
    let c = bounds.center();
    let (hw, hh) = (bounds.width() / 2.0, bounds.height() / 2.0);
    match handle.direction() {
        Some((dx, dy)) => Point::new(c.x + dx * hw, c.y + dy * hh),
        None => Point::new(c.x, bounds.max.y + rotate_offset_mm),
    }
}

/// The handle nearest to `point` within `tolerance_mm` (per axis).
pub fn hit_handle(
    bounds: Rect,
    point: Point,
    tolerance_mm: f64,
    rotate_offset_mm: f64,
) -> Option<Handle> {
    Handle::ALL
        .into_iter()
        .map(|h| (h, handle_position(bounds, h, rotate_offset_mm)))
        .filter(|(_, p)| {
            (p.x - point.x).abs() <= tolerance_mm && (p.y - point.y).abs() <= tolerance_mm
        })
        .min_by(|(_, p), (_, q)| point.distance(*p).total_cmp(&point.distance(*q)))
        .map(|(h, _)| h)
}

pub fn move_delta(from: Point, to: Point) -> Transform {
    Transform::translate(to.x - from.x, to.y - from.y)
}

/// Scaling for dragging `handle` from `from` to `to`, anchored at the
/// opposite handle. Corner handles scale both axes, edge handles one.
/// `keep_aspect` scales both axes by the same factor. A zero-size axis
/// cannot be scaled; dragging the rotate handle gives the identity.
pub fn scale_delta(
    bounds: Rect,
    handle: Handle,
    from: Point,
    to: Point,
    keep_aspect: bool,
) -> Transform {
    let Some((dx, dy)) = handle.direction() else {
        return Transform::identity();
    };
    let start = handle_position(bounds, handle, 0.0);
    let anchor = handle_position(bounds, opposite(handle), 0.0);
    let end = Point::new(start.x + to.x - from.x, start.y + to.y - from.y);

    let axis_scale = |dir: f64, size: f64, start: f64, end: f64, anchor: f64| {
        if dir == 0.0 || size < EPS {
            return None;
        }
        let s = (end - anchor) / (start - anchor);
        Some(s.max(MIN_SIZE_MM / size))
    };
    let sx = axis_scale(dx, bounds.width(), start.x, end.x, anchor.x);
    let sy = axis_scale(dy, bounds.height(), start.y, end.y, anchor.y);

    let (sx, sy) = if keep_aspect {
        // The axis that moved further from 1 decides.
        let s = match (sx, sy) {
            (Some(x), Some(y)) if (x - 1.0).abs() >= (y - 1.0).abs() => x,
            (Some(_), Some(y)) => y,
            (Some(x), None) => x,
            (None, Some(y)) => y,
            (None, None) => 1.0,
        };
        let floor = MIN_SIZE_MM / bounds.width().max(bounds.height()).max(EPS);
        let s = s.max(floor);
        let usable = |size: f64| if size < EPS { 1.0 } else { s };
        (usable(bounds.width()), usable(bounds.height()))
    } else {
        (sx.unwrap_or(1.0), sy.unwrap_or(1.0))
    };
    Transform::scale_about(anchor, sx, sy)
}

/// Rotation about `center` that turns `from` towards `to`. With `snap`,
/// the angle is rounded to multiples of that step (radians). Points on the
/// center give the identity.
pub fn rotate_delta(center: Point, from: Point, to: Point, snap: Option<f64>) -> Transform {
    let (a, b) = (from - center, to - center);
    if a.length() < EPS || b.length() < EPS {
        return Transform::identity();
    }
    let mut angle = b.y.atan2(b.x) - a.y.atan2(a.x);
    if let Some(step) = snap.filter(|s| s.is_finite() && *s > 0.0) {
        angle = (angle / step).round() * step;
    }
    Transform::rotate_about(center, angle)
}

fn opposite(handle: Handle) -> Handle {
    match handle {
        Handle::N => Handle::S,
        Handle::NE => Handle::SW,
        Handle::E => Handle::W,
        Handle::SE => Handle::NW,
        Handle::S => Handle::N,
        Handle::SW => Handle::NE,
        Handle::W => Handle::E,
        Handle::NW => Handle::SE,
        Handle::Rotate => Handle::Rotate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    const TOL: f64 = 1e-9;

    fn bounds() -> Rect {
        Rect::new(Point::new(10.0, 20.0), Point::new(30.0, 30.0)) // 20 × 10
    }

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn handle_positions_follow_the_compass() {
        let b = bounds();
        assert!(handle_position(b, Handle::NE, 5.0).approx_eq(p(30.0, 30.0), TOL));
        assert!(handle_position(b, Handle::S, 5.0).approx_eq(p(20.0, 20.0), TOL));
        assert!(handle_position(b, Handle::W, 5.0).approx_eq(p(10.0, 25.0), TOL));
        assert!(handle_position(b, Handle::Rotate, 5.0).approx_eq(p(20.0, 35.0), TOL));
    }

    #[test]
    fn hit_handle_picks_the_nearest_within_tolerance() {
        let b = bounds();
        assert_eq!(hit_handle(b, p(30.4, 29.7), 0.5, 5.0), Some(Handle::NE));
        assert_eq!(hit_handle(b, p(20.0, 35.2), 0.5, 5.0), Some(Handle::Rotate));
        assert_eq!(hit_handle(b, p(25.0, 25.0), 0.5, 5.0), None);
        // On a tiny selection several handles overlap; the closest wins.
        let tiny = Rect::new(p(0.0, 0.0), p(0.2, 0.2));
        assert_eq!(hit_handle(tiny, p(0.21, 0.21), 0.5, 5.0), Some(Handle::NE));
    }

    #[test]
    fn move_delta_translates() {
        let t = move_delta(p(1.0, 2.0), p(4.0, -2.0));
        assert!(t.apply_point(p(0.0, 0.0)).approx_eq(p(3.0, -4.0), TOL));
    }

    #[test]
    fn corner_scale_keeps_the_opposite_corner_fixed() {
        let b = bounds();
        // Drag NE from (30,30) to (40,35): width 20→30, height 10→15.
        let t = scale_delta(b, Handle::NE, p(30.0, 30.0), p(40.0, 35.0), false);
        assert!(t.apply_point(p(10.0, 20.0)).approx_eq(p(10.0, 20.0), TOL));
        assert!(t.apply_point(p(30.0, 30.0)).approx_eq(p(40.0, 35.0), TOL));
    }

    #[test]
    fn grab_offset_does_not_jump() {
        // Grabbing slightly off the handle must not change the result.
        let b = bounds();
        let a = scale_delta(b, Handle::SW, p(10.0, 20.0), p(5.0, 18.0), false);
        let c = scale_delta(b, Handle::SW, p(10.3, 20.2), p(5.3, 18.2), false);
        assert!(a.approx_eq(&c, TOL));
    }

    #[test]
    fn edge_handles_scale_one_axis() {
        let b = bounds();
        let t = scale_delta(b, Handle::E, p(30.0, 25.0), p(50.0, 99.0), false);
        assert!(
            t.transform_rect(&b)
                .approx_eq(&Rect::new(p(10.0, 20.0), p(50.0, 30.0)), TOL)
        );
        let t = scale_delta(b, Handle::S, p(20.0, 20.0), p(0.0, 15.0), false);
        assert!(
            t.transform_rect(&b)
                .approx_eq(&Rect::new(p(10.0, 15.0), p(30.0, 30.0)), TOL)
        );
    }

    #[test]
    fn keep_aspect_uses_the_dominant_axis() {
        let b = bounds();
        // x: 20 → 40 (×2), y: 10 → 11 (×1.1); ×2 wins.
        let t = scale_delta(b, Handle::NE, p(30.0, 30.0), p(50.0, 31.0), true);
        assert!(
            t.transform_rect(&b)
                .approx_eq(&Rect::new(p(10.0, 20.0), p(50.0, 40.0)), TOL)
        );
        // An edge handle with keep_aspect scales both axes about the opposite edge.
        let t = scale_delta(b, Handle::E, p(30.0, 25.0), p(40.0, 25.0), true);
        let scaled = t.transform_rect(&b);
        assert!(approx(scaled.width(), 30.0) && approx(scaled.height(), 15.0));
        assert!(approx(scaled.center().y, 25.0));
    }

    #[test]
    fn scaling_never_mirrors_or_collapses() {
        let b = bounds();
        // Dragging E far past the W edge.
        let t = scale_delta(b, Handle::E, p(30.0, 25.0), p(-100.0, 25.0), false);
        let scaled = t.transform_rect(&b);
        assert!(approx(scaled.width(), MIN_SIZE_MM));
        assert!(approx(scaled.min.x, 10.0));
        let t = scale_delta(b, Handle::NE, p(30.0, 30.0), p(-100.0, -100.0), true);
        let scaled = t.transform_rect(&b);
        assert!(scaled.width() >= MIN_SIZE_MM - TOL && scaled.height() > 0.0);
    }

    #[test]
    fn degenerate_axes_are_left_alone() {
        // A horizontal line: no height.
        let line = Rect::new(p(0.0, 5.0), p(10.0, 5.0));
        let t = scale_delta(line, Handle::NE, p(10.0, 5.0), p(20.0, 50.0), false);
        assert!(
            t.transform_rect(&line)
                .approx_eq(&Rect::new(p(0.0, 5.0), p(20.0, 5.0)), TOL)
        );
        let t = scale_delta(line, Handle::NE, p(10.0, 5.0), p(20.0, 50.0), true);
        assert!(
            t.transform_rect(&line)
                .approx_eq(&Rect::new(p(0.0, 5.0), p(20.0, 5.0)), TOL)
        );
        let point = Rect::new(p(1.0, 1.0), p(1.0, 1.0));
        let t = scale_delta(point, Handle::SE, p(1.0, 1.0), p(9.0, -9.0), true);
        assert!(t.is_identity());
        assert!(scale_delta(line, Handle::Rotate, p(0.0, 0.0), p(5.0, 5.0), false).is_identity());
    }

    #[test]
    fn rotate_turns_about_the_center() {
        let c = p(0.0, 0.0);
        let t = rotate_delta(c, p(10.0, 0.0), p(0.0, 3.0), None);
        assert!(t.apply_point(p(5.0, 0.0)).approx_eq(p(0.0, 5.0), 1e-9));
        let around = rotate_delta(p(5.0, 5.0), p(6.0, 5.0), p(4.0, 5.0), None);
        assert!(around.apply_point(p(7.0, 5.0)).approx_eq(p(3.0, 5.0), 1e-9));
    }

    #[test]
    fn rotate_snaps_to_steps() {
        let c = p(0.0, 0.0);
        // 50° snaps to 45° with a 15° step.
        let to = p(50f64.to_radians().cos(), 50f64.to_radians().sin());
        let t = rotate_delta(c, p(1.0, 0.0), to, Some(15f64.to_radians()));
        let expected = Transform::rotate(FRAC_PI_4);
        assert!(t.approx_eq(&expected, 1e-9));
        // Invalid steps are ignored.
        let free = rotate_delta(c, p(1.0, 0.0), p(0.0, 1.0), Some(0.0));
        assert!(free.approx_eq(&Transform::rotate(FRAC_PI_2), 1e-9));
        let half = rotate_delta(c, p(1.0, 0.0), p(-1.0, 0.0), Some(f64::NAN));
        assert!(half.approx_eq(&Transform::rotate(PI), 1e-9));
    }

    #[test]
    fn rotate_from_the_center_is_identity() {
        let c = p(2.0, 2.0);
        assert!(rotate_delta(c, c, p(5.0, 5.0), None).is_identity());
        assert!(rotate_delta(c, p(5.0, 5.0), c, None).is_identity());
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }
}
