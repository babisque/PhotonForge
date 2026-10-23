use crate::geometry::{EPS, Point, Rect, Vec2, approx_eq};

/// A 2D affine transform stored as a 2×3 matrix:
///
/// ```text
/// | a  c  e |     x' = a·x + c·y + e
/// | b  d  f |     y' = b·x + d·y + f
/// ```
///
/// This is the same element order as SVG's `matrix(a b c d e f)`.
/// Translation (`e`, `f`) is in mm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64, // mm
    pub f: f64, // mm
}

impl Transform {
    pub const IDENTITY: Transform = Transform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub const fn identity() -> Self {
        Self::IDENTITY
    }

    pub const fn translate(dx: f64, dy: f64) -> Self {
        Self {
            e: dx,
            f: dy,
            ..Self::IDENTITY
        }
    }

    pub const fn scale(sx: f64, sy: f64) -> Self {
        Self {
            a: sx,
            d: sy,
            ..Self::IDENTITY
        }
    }

    /// Counter-clockwise rotation (Y-up) about the origin by `angle` radians.
    pub fn rotate(angle: f64) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            e: 0.0,
            f: 0.0,
        }
    }

    pub fn rotate_about(center: Point, angle: f64) -> Self {
        Self::translate(-center.x, -center.y)
            .then(Self::rotate(angle))
            .then(Self::translate(center.x, center.y))
    }

    pub fn scale_about(center: Point, sx: f64, sy: f64) -> Self {
        Self::translate(-center.x, -center.y)
            .then(Self::scale(sx, sy))
            .then(Self::translate(center.x, center.y))
    }

    /// Composition: `a.then(b)` applies `a` first, then `b`.
    pub fn then(self, next: Transform) -> Transform {
        let s = self;
        let n = next;
        Transform {
            a: n.a * s.a + n.c * s.b,
            b: n.b * s.a + n.d * s.b,
            c: n.a * s.c + n.c * s.d,
            d: n.b * s.c + n.d * s.d,
            e: n.a * s.e + n.c * s.f + n.e,
            f: n.b * s.e + n.d * s.f + n.f,
        }
    }

    pub fn determinant(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// The inverse transform, or `None` when the matrix is singular
    /// (|determinant| < `EPS`) or not finite.
    pub fn inverse(&self) -> Option<Transform> {
        let det = self.determinant();
        if !det.is_finite() || det.abs() < EPS {
            return None;
        }
        Some(Transform {
            a: self.d / det,
            b: -self.b / det,
            c: -self.c / det,
            d: self.a / det,
            e: (self.c * self.f - self.d * self.e) / det,
            f: (self.b * self.e - self.a * self.f) / det,
        })
    }

    pub fn apply_point(&self, p: Point) -> Point {
        Point::new(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }

    /// Applies the linear part only; translation does not move vectors.
    pub fn apply_vector(&self, v: Vec2) -> Vec2 {
        Vec2::new(self.a * v.x + self.c * v.y, self.b * v.x + self.d * v.y)
    }

    pub fn transform_rect(&self, rect: &Rect) -> Rect {
        let corners = [
            Point::new(rect.min.x, rect.max.y),
            Point::new(rect.max.x, rect.min.y),
            rect.max,
        ];
        let start = self.apply_point(rect.min);
        corners.into_iter().fold(Rect::new(start, start), |acc, p| {
            acc.expand_to(self.apply_point(p))
        })
    }

    pub fn is_identity(&self) -> bool {
        self.approx_eq(&Self::IDENTITY, EPS)
    }

    pub fn approx_eq(&self, other: &Transform, eps: f64) -> bool {
        approx_eq(self.a, other.a, eps)
            && approx_eq(self.b, other.b, eps)
            && approx_eq(self.c, other.c, eps)
            && approx_eq(self.d, other.d, eps)
            && approx_eq(self.e, other.e, eps)
            && approx_eq(self.f, other.f, eps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    fn sample_transforms() -> Vec<Transform> {
        vec![
            Transform::identity(),
            Transform::translate(12.5, -3.0),
            Transform::scale(2.0, 0.5),
            Transform::scale(-1.0, 1.0),
            Transform::rotate(0.3),
            Transform::rotate(-2.0),
            Transform::rotate_about(Point::new(5.0, 7.0), 1.1),
            Transform::scale_about(Point::new(-4.0, 2.0), 3.0, 0.25),
            Transform::rotate(0.7)
                .then(Transform::scale(1.5, -2.0))
                .then(Transform::translate(100.0, 50.0)),
            Transform {
                a: 1.0,
                b: 0.2,
                c: 0.5,
                d: 1.0,
                e: 3.0,
                f: -4.0,
            }, // shear
        ]
    }

    #[test]
    fn identity_leaves_points_unchanged() {
        let p = Point::new(3.0, -4.0);
        assert!(Transform::identity().apply_point(p).approx_eq(p, EPS));
        assert!(Transform::identity().is_identity());
    }

    #[test]
    fn translate_moves_points_but_not_vectors() {
        let t = Transform::translate(10.0, 5.0);
        let p = t.apply_point(Point::new(1.0, 2.0));
        assert!(p.approx_eq(Point::new(11.0, 7.0), EPS));
        let v = t.apply_vector(Vec2::new(1.0, 2.0));
        assert!(v.approx_eq(Vec2::new(1.0, 2.0), EPS));
    }

    #[test]
    fn scale_matrix() {
        let t = Transform::scale(2.0, 3.0);
        let p = t.apply_point(Point::new(4.0, -1.0));
        assert!(p.approx_eq(Point::new(8.0, -3.0), EPS));
        assert!(approx_eq(t.determinant(), 6.0, EPS));
    }

    #[test]
    fn rotate_quarter_turn_is_counter_clockwise() {
        let t = Transform::rotate(FRAC_PI_2);
        assert!(
            t.apply_point(Point::new(1.0, 0.0))
                .approx_eq(Point::new(0.0, 1.0), EPS)
        );
        assert!(
            t.apply_point(Point::new(0.0, 1.0))
                .approx_eq(Point::new(-1.0, 0.0), EPS)
        );
        assert!(approx_eq(t.determinant(), 1.0, EPS));
    }

    #[test]
    fn rotate_matches_known_matrix() {
        let t = Transform::rotate(PI / 6.0);
        let (s, c) = (0.5, 3.0_f64.sqrt() / 2.0);
        let expected = Transform {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: 0.0,
            f: 0.0,
        };
        assert!(t.approx_eq(&expected, EPS));
    }

    #[test]
    fn rotate_about_keeps_center_fixed() {
        let center = Point::new(5.0, 5.0);
        let t = Transform::rotate_about(center, FRAC_PI_2);
        assert!(t.apply_point(center).approx_eq(center, EPS));
        assert!(
            t.apply_point(Point::new(6.0, 5.0))
                .approx_eq(Point::new(5.0, 6.0), EPS)
        );
    }

    #[test]
    fn scale_about_keeps_center_fixed() {
        let center = Point::new(2.0, 3.0);
        let t = Transform::scale_about(center, 2.0, 2.0);
        assert!(t.apply_point(center).approx_eq(center, EPS));
        assert!(
            t.apply_point(Point::new(3.0, 3.0))
                .approx_eq(Point::new(4.0, 3.0), EPS)
        );
    }

    #[test]
    fn then_applies_left_first() {
        let p = Point::new(1.0, 0.0);
        let translate_then_rotate =
            Transform::translate(1.0, 0.0).then(Transform::rotate(FRAC_PI_2));
        let rotate_then_translate =
            Transform::rotate(FRAC_PI_2).then(Transform::translate(1.0, 0.0));
        assert!(
            translate_then_rotate
                .apply_point(p)
                .approx_eq(Point::new(0.0, 2.0), EPS)
        );
        assert!(
            rotate_then_translate
                .apply_point(p)
                .approx_eq(Point::new(1.0, 1.0), EPS)
        );
    }

    #[test]
    fn then_matches_sequential_application() {
        let p = Point::new(3.5, -1.25);
        for a in sample_transforms() {
            for b in sample_transforms() {
                let composed = a.then(b).apply_point(p);
                let sequential = b.apply_point(a.apply_point(p));
                assert!(composed.approx_eq(sequential, 1e-9));
            }
        }
    }

    #[test]
    fn then_with_identity_is_noop() {
        for t in sample_transforms() {
            assert!(t.then(Transform::IDENTITY).approx_eq(&t, EPS));
            assert!(Transform::IDENTITY.then(t).approx_eq(&t, EPS));
        }
    }

    #[test]
    fn inverse_round_trips() {
        for t in sample_transforms() {
            let inv = t.inverse().unwrap();
            assert!(t.then(inv).is_identity(), "t·t⁻¹ ≠ I for {t:?}");
            assert!(inv.then(t).is_identity(), "t⁻¹·t ≠ I for {t:?}");
            let p = Point::new(-7.0, 11.0);
            assert!(inv.apply_point(t.apply_point(p)).approx_eq(p, 1e-9));
        }
    }

    #[test]
    fn singular_transform_has_no_inverse() {
        assert_eq!(Transform::scale(0.0, 1.0).inverse(), None);
        let collapsed = Transform {
            a: 1.0,
            b: 2.0,
            c: 2.0,
            d: 4.0,
            e: 0.0,
            f: 0.0,
        };
        assert_eq!(collapsed.inverse(), None);
        assert_eq!(Transform::scale(f64::NAN, 1.0).inverse(), None);
    }

    #[test]
    fn transform_rect_rotated_quarter_turn() {
        let r = Rect::new(Point::new(0.0, 0.0), Point::new(2.0, 1.0));
        let out = Transform::rotate(FRAC_PI_2).transform_rect(&r);
        let expected = Rect::new(Point::new(-1.0, 0.0), Point::new(0.0, 2.0));
        assert!(out.approx_eq(&expected, EPS));
    }

    #[test]
    fn transform_rect_rotated_45_degrees_grows() {
        let r = Rect::new(Point::new(-1.0, -1.0), Point::new(1.0, 1.0));
        let out = Transform::rotate(PI / 4.0).transform_rect(&r);
        let h = 2.0_f64.sqrt();
        let expected = Rect::new(Point::new(-h, -h), Point::new(h, h));
        assert!(out.approx_eq(&expected, EPS));
    }

    #[test]
    fn transform_rect_with_mirror_stays_normalized() {
        let r = Rect::new(Point::new(1.0, 1.0), Point::new(3.0, 2.0));
        let out = Transform::scale(-1.0, -1.0).transform_rect(&r);
        let expected = Rect::new(Point::new(-3.0, -2.0), Point::new(-1.0, -1.0));
        assert!(out.approx_eq(&expected, EPS));
    }
}
