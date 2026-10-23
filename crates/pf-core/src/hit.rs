//! Picking shapes by point and by rectangle. Shapes are hit on their
//! outline, not their interior, because laser outlines are usually unfilled.

use std::collections::HashSet;

use crate::document::Document;
use crate::flatten::{Polyline, flatten_path};
use crate::geometry::{Point, Rect, distance_to_segment};
use crate::shape::{Shape, ShapeId};

/// Chord tolerance for hit testing (mm). Matches the default output
/// tolerance, so picking follows what gets cut.
const HIT_FLATTEN_TOLERANCE: f64 = 0.01;

/// How a selection rectangle chooses shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarqueeMode {
    /// Shapes entirely inside the rectangle.
    Window,
    /// Shapes whose outline touches or lies inside the rectangle.
    Crossing,
}

/// The topmost shape (last in insertion order) whose outline passes within
/// `tolerance_mm` of `point`. A non-positive or non-finite tolerance hits
/// nothing.
pub fn pick_shape(doc: &Document, point: Point, tolerance_mm: f64) -> Option<ShapeId> {
    if !(tolerance_mm.is_finite() && tolerance_mm > 0.0) {
        return None;
    }
    doc.shapes()
        .iter()
        .rev()
        .filter(|s| {
            s.bounds()
                .is_some_and(|b| b.inflate(tolerance_mm).contains_point(point))
        })
        .find(|s| {
            outline(s).iter().any(|polyline| {
                polyline_distance(polyline, point).is_some_and(|d| d <= tolerance_mm)
            })
        })
        .map(|s| s.id)
}

/// Shapes chosen by `rect`, in insertion order.
pub fn shapes_in_rect(doc: &Document, rect: Rect, mode: MarqueeMode) -> Vec<ShapeId> {
    doc.shapes()
        .iter()
        .filter(|s| {
            let Some(bounds) = s.bounds() else {
                return false;
            };
            match mode {
                MarqueeMode::Window => rect.contains_rect(&bounds),
                MarqueeMode::Crossing => {
                    rect.contains_rect(&bounds)
                        || (rect.intersects(&bounds)
                            && outline(s).iter().any(|p| polyline_touches_rect(p, &rect)))
                }
            }
        })
        .map(|s| s.id)
        .collect()
}

/// Union of the bounds of the shapes in `ids`. Unknown IDs are ignored.
pub fn selection_bounds(doc: &Document, ids: &[ShapeId]) -> Option<Rect> {
    let wanted: HashSet<ShapeId> = ids.iter().copied().collect();
    doc.shapes()
        .iter()
        .filter(|s| wanted.contains(&s.id))
        .filter_map(Shape::bounds)
        .reduce(|a, b| a.union(&b))
}

fn outline(shape: &Shape) -> Vec<Polyline> {
    // The tolerance is a valid constant, so flattening cannot fail.
    flatten_path(&shape.world_path(), HIT_FLATTEN_TOLERANCE).unwrap_or_default()
}

fn polyline_distance(polyline: &Polyline, p: Point) -> Option<f64> {
    match polyline.points.as_slice() {
        [] => None,
        [only] => Some(only.distance(p)),
        points => points
            .windows(2)
            .map(|w| distance_to_segment(p, w[0], w[1]))
            .reduce(f64::min),
    }
}

fn polyline_touches_rect(polyline: &Polyline, rect: &Rect) -> bool {
    match polyline.points.as_slice() {
        [] => false,
        [only] => rect.contains_point(*only),
        points => points
            .windows(2)
            .any(|w| segment_touches_rect(w[0], w[1], rect)),
    }
}

fn segment_touches_rect(a: Point, b: Point, rect: &Rect) -> bool {
    if rect.contains_point(a) || rect.contains_point(b) {
        return true;
    }
    let corners = [
        rect.min,
        Point::new(rect.max.x, rect.min.y),
        rect.max,
        Point::new(rect.min.x, rect.max.y),
    ];
    (0..4).any(|i| segments_intersect(a, b, corners[i], corners[(i + 1) % 4]))
}

fn segments_intersect(p1: Point, p2: Point, q1: Point, q2: Point) -> bool {
    let d1 = (p2 - p1).cross(q1 - p1);
    let d2 = (p2 - p1).cross(q2 - p1);
    let d3 = (q2 - q1).cross(p1 - q1);
    let d4 = (q2 - q1).cross(p2 - q1);
    // Touching endpoints count; fully collinear overlap is covered by the
    // endpoint-in-rect check of the caller.
    d1 * d2 <= 0.0 && d3 * d4 <= 0.0 && !(d1 == 0.0 && d2 == 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::{EllipseShape, RectShape, ShapeKind};
    use crate::transform::Transform;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(Point::new(x0, y0), Point::new(x1, y1))
    }

    /// A 10×10 square at (0,0), another overlapping it at (5,5), and a
    /// circle of radius 5 centered at (50,50).
    fn sample() -> (Document, [ShapeId; 3]) {
        let mut doc = Document::new();
        let layer = doc.add_layer("Cut", [0, 0, 0]).unwrap();
        let square = || ShapeKind::Rect(RectShape::new(10.0, 10.0, 0.0).unwrap());
        let a = doc
            .add_shape(square(), Transform::identity(), layer)
            .unwrap();
        let b = doc
            .add_shape(square(), Transform::translate(5.0, 5.0), layer)
            .unwrap();
        let circle = ShapeKind::Ellipse(EllipseShape::new(5.0, 5.0).unwrap());
        let c = doc
            .add_shape(circle, Transform::translate(50.0, 50.0), layer)
            .unwrap();
        (doc, [a, b, c])
    }

    #[test]
    fn pick_hits_outlines_within_tolerance_only() {
        let (doc, [a, _, c]) = sample();
        assert_eq!(pick_shape(&doc, Point::new(0.3, 2.0), 0.5), Some(a));
        assert_eq!(pick_shape(&doc, Point::new(0.6, 2.0), 0.5), None);
        // Inside the square but away from any outline.
        assert_eq!(pick_shape(&doc, Point::new(2.0, 2.0), 0.5), None);
        // On the curved outline of the circle.
        assert_eq!(pick_shape(&doc, Point::new(55.0, 50.0), 0.1), Some(c));
        assert_eq!(pick_shape(&doc, Point::new(50.0, 50.0), 0.1), None);
    }

    #[test]
    fn pick_prefers_the_topmost_shape() {
        let (doc, [a, b, _]) = sample();
        // (10, 5) is on the right edge of `a` and the bottom edge of `b`.
        assert_eq!(pick_shape(&doc, Point::new(10.0, 5.0), 0.2), Some(b));
        assert_eq!(pick_shape(&doc, Point::new(10.0, 2.0), 0.2), Some(a));
    }

    #[test]
    fn invalid_tolerance_hits_nothing() {
        let (doc, _) = sample();
        for tol in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(pick_shape(&doc, Point::new(0.0, 5.0), tol), None);
        }
    }

    #[test]
    fn window_needs_full_containment() {
        let (doc, [a, b, c]) = sample();
        assert_eq!(
            shapes_in_rect(&doc, rect(-1.0, -1.0, 11.0, 11.0), MarqueeMode::Window),
            vec![a]
        );
        assert_eq!(
            shapes_in_rect(&doc, rect(-1.0, -1.0, 60.0, 60.0), MarqueeMode::Window),
            vec![a, b, c]
        );
    }

    #[test]
    fn crossing_needs_the_outline_to_touch() {
        let (doc, [a, b, c]) = sample();
        // A thin box over the shared corner region touches both squares.
        assert_eq!(
            shapes_in_rect(&doc, rect(8.0, 4.0, 12.0, 6.0), MarqueeMode::Crossing),
            vec![a, b]
        );
        // Inside the circle but not touching its outline: its bounds
        // overlap, yet nothing is crossed.
        assert_eq!(
            shapes_in_rect(&doc, rect(48.0, 48.0, 52.0, 52.0), MarqueeMode::Crossing),
            Vec::<ShapeId>::new()
        );
        // A box straddling the circle's rightmost point.
        assert_eq!(
            shapes_in_rect(&doc, rect(54.0, 49.0, 56.0, 51.0), MarqueeMode::Crossing),
            vec![c]
        );
        // A line segment passing through a box without an endpoint inside.
        assert_eq!(
            shapes_in_rect(&doc, rect(-1.0, 3.0, 1.0, 4.0), MarqueeMode::Crossing),
            vec![a]
        );
    }

    #[test]
    fn selection_bounds_unites_selected_shapes() {
        let (doc, [a, b, c]) = sample();
        assert_eq!(selection_bounds(&doc, &[]), None);
        let ab = selection_bounds(&doc, &[b, a]).unwrap();
        assert!(ab.approx_eq(&rect(0.0, 0.0, 15.0, 15.0), 1e-9));
        let with_unknown = selection_bounds(&doc, &[c, ShapeId(99)]).unwrap();
        assert!(with_unknown.approx_eq(&rect(45.0, 45.0, 55.0, 55.0), 1e-9));
    }
}
