//! Mapping between document millimeters (Y up) and screen pixels (Y down).
//! Screen positions reuse `Point`, but their values are pixels.

use crate::geometry::{Point, Rect, Vec2};

pub const MIN_ZOOM: f64 = 0.05; // px per mm
pub const MAX_ZOOM: f64 = 500.0; // px per mm

/// Invariant: `zoom` is finite and within `[MIN_ZOOM, MAX_ZOOM]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    offset: Point, // px: where document (0, 0) appears on screen
    zoom: f64,     // px per mm
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport {
            offset: Point::ORIGIN,
            zoom: 1.0,
        }
    }
}

impl Viewport {
    /// A non-finite `zoom` falls back to 1 px/mm.
    pub fn new(offset: Point, zoom: f64) -> Viewport {
        Viewport {
            offset,
            zoom: clamp_zoom(zoom),
        }
    }

    pub fn offset(&self) -> Point {
        self.offset
    }

    pub fn zoom(&self) -> f64 {
        self.zoom
    }

    pub fn to_screen(&self, p: Point) -> Point {
        Point::new(
            self.offset.x + p.x * self.zoom,
            self.offset.y - p.y * self.zoom,
        )
    }

    pub fn to_world(&self, s: Point) -> Point {
        Point::new(
            (s.x - self.offset.x) / self.zoom,
            (self.offset.y - s.y) / self.zoom,
        )
    }

    pub fn pan(&mut self, delta_px: Vec2) {
        self.offset = self.offset + delta_px;
    }

    /// Scales the zoom by `factor`, keeping the document point under
    /// `anchor_px` fixed on screen. Invalid factors are ignored.
    pub fn zoom_about(&mut self, anchor_px: Point, factor: f64) {
        if !(factor.is_finite() && factor > 0.0) {
            return;
        }
        let world = self.to_world(anchor_px);
        self.zoom = clamp_zoom(self.zoom * factor);
        self.offset = Point::new(
            anchor_px.x - world.x * self.zoom,
            anchor_px.y + world.y * self.zoom,
        );
    }

    /// The viewport that centers `world` in `screen` with `margin_px` on
    /// every side. Empty extents count as 1 mm so the zoom stays finite.
    pub fn fit(world: Rect, screen: Rect, margin_px: f64) -> Viewport {
        let avail_w = (screen.width() - 2.0 * margin_px).max(1.0);
        let avail_h = (screen.height() - 2.0 * margin_px).max(1.0);
        let zoom =
            clamp_zoom((avail_w / world.width().max(1.0)).min(avail_h / world.height().max(1.0)));
        let s = screen.center();
        let w = world.center();
        Viewport {
            offset: Point::new(s.x - w.x * zoom, s.y + w.y * zoom),
            zoom,
        }
    }

    /// The document area (mm) shown inside `screen` (px).
    pub fn visible_world_rect(&self, screen: Rect) -> Rect {
        Rect::new(self.to_world(screen.min), self.to_world(screen.max))
    }
}

/// The smallest grid step (mm) from the 1-2-5 series that is at least
/// `min_px` wide at `zoom`.
pub fn grid_step(zoom: f64, min_px: f64) -> f64 {
    let raw = min_px / zoom;
    if !(raw.is_finite() && raw > 0.0) {
        return 1.0;
    }
    let decade = 10f64.powf(raw.log10().floor());
    for m in [1.0, 2.0, 5.0] {
        let step = m * decade;
        // Tolerance absorbs rounding in log10/powf at exact series values.
        if step >= raw * (1.0 - 1e-9) {
            return step;
        }
    }
    10.0 * decade
}

fn clamp_zoom(zoom: f64) -> f64 {
    if zoom.is_finite() {
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::approx_eq;

    const TOL: f64 = 1e-9;

    fn screen(w: f64, h: f64) -> Rect {
        Rect::new(Point::ORIGIN, Point::new(w, h))
    }

    #[test]
    fn y_axis_is_flipped() {
        let vp = Viewport::new(Point::new(100.0, 500.0), 2.0);
        let s = vp.to_screen(Point::new(10.0, 20.0));
        assert!(s.approx_eq(Point::new(120.0, 460.0), TOL));
        let above = vp.to_screen(Point::new(10.0, 30.0));
        assert!(above.y < s.y);
    }

    #[test]
    fn screen_world_round_trip() {
        let vp = Viewport::new(Point::new(-37.5, 812.25), 3.7);
        for p in [
            Point::ORIGIN,
            Point::new(123.4, -56.7),
            Point::new(-1e3, 2e3),
        ] {
            assert!(vp.to_world(vp.to_screen(p)).approx_eq(p, 1e-9));
        }
    }

    #[test]
    fn pan_moves_the_offset() {
        let mut vp = Viewport::default();
        vp.pan(Vec2::new(5.0, -3.0));
        assert!(vp.offset().approx_eq(Point::new(5.0, -3.0), TOL));
        assert!(approx_eq(vp.zoom(), 1.0, TOL));
    }

    #[test]
    fn zoom_about_keeps_the_anchor_fixed() {
        let mut vp = Viewport::new(Point::new(40.0, 600.0), 1.5);
        let anchor = Point::new(321.0, 123.0);
        let before = vp.to_world(anchor);
        vp.zoom_about(anchor, 2.5);
        assert!(approx_eq(vp.zoom(), 3.75, TOL));
        assert!(vp.to_world(anchor).approx_eq(before, 1e-9));
        vp.zoom_about(anchor, 0.1);
        assert!(vp.to_world(anchor).approx_eq(before, 1e-9));
    }

    #[test]
    fn zoom_is_clamped_and_invalid_factors_are_ignored() {
        let mut vp = Viewport::default();
        vp.zoom_about(Point::ORIGIN, 1e9);
        assert!(approx_eq(vp.zoom(), MAX_ZOOM, TOL));
        vp.zoom_about(Point::ORIGIN, 1e-12);
        assert!(approx_eq(vp.zoom(), MIN_ZOOM, TOL));
        for bad in [0.0, -2.0, f64::NAN, f64::INFINITY] {
            let before = vp;
            vp.zoom_about(Point::new(10.0, 10.0), bad);
            assert_eq!(vp, before);
        }
        assert!(approx_eq(
            Viewport::new(Point::ORIGIN, f64::NAN).zoom(),
            1.0,
            TOL
        ));
        assert!(approx_eq(
            Viewport::new(Point::ORIGIN, 0.0).zoom(),
            MIN_ZOOM,
            TOL
        ));
    }

    #[test]
    fn fit_centers_and_contains_the_world_rect() {
        let world = Rect::new(Point::ORIGIN, Point::new(300.0, 200.0));
        let screen = screen(800.0, 600.0);
        let vp = Viewport::fit(world, screen, 20.0);
        // Width limits: (800 - 40) / 300.
        assert!(approx_eq(vp.zoom(), 760.0 / 300.0, TOL));
        assert!(
            vp.to_screen(world.center())
                .approx_eq(screen.center(), 1e-9)
        );
        let shown = vp.visible_world_rect(screen);
        assert!(shown.contains_rect(&world));
    }

    #[test]
    fn fit_handles_degenerate_input() {
        let point = Rect::new(Point::new(5.0, 5.0), Point::new(5.0, 5.0));
        let vp = Viewport::fit(point, screen(0.0, 0.0), 50.0);
        assert!(vp.zoom().is_finite());
        assert!(vp.offset().x.is_finite() && vp.offset().y.is_finite());
    }

    #[test]
    fn visible_world_rect_is_normalized() {
        let vp = Viewport::new(Point::new(0.0, 100.0), 2.0);
        let shown = vp.visible_world_rect(screen(200.0, 100.0));
        let expected = Rect::new(Point::ORIGIN, Point::new(100.0, 50.0));
        assert!(shown.approx_eq(&expected, TOL));
    }

    #[test]
    fn grid_step_follows_the_1_2_5_series() {
        let cases = [
            (1.0, 10.0, 10.0),
            (2.0, 10.0, 5.0),
            (3.0, 10.0, 5.0),
            (4.0, 10.0, 5.0),
            (6.0, 10.0, 2.0),
            (10.0, 10.0, 1.0),
            (0.3, 10.0, 50.0),
            (0.05, 10.0, 200.0),
            (400.0, 10.0, 0.05),
        ];
        for (zoom, min_px, expected) in cases {
            let step = grid_step(zoom, min_px);
            assert!(
                approx_eq(step, expected, 1e-12),
                "zoom {zoom}: got {step}, expected {expected}"
            );
            assert!(step * zoom >= min_px * (1.0 - 1e-9));
        }
    }

    #[test]
    fn grid_step_with_invalid_input_is_one_mm() {
        assert!(approx_eq(grid_step(0.0, 10.0), 1.0, TOL));
        assert!(approx_eq(grid_step(f64::NAN, 10.0), 1.0, TOL));
        assert!(approx_eq(grid_step(1.0, 0.0), 1.0, TOL));
    }
}
