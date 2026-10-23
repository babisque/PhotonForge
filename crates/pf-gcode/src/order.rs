use pf_core::{Point, Rect};

use crate::toolpath::Toolpath;

/// Orders toolpaths within each layer and keeps the layer order. Anything
/// inside a closed toolpath is cut before it, so a part never drops out
/// before its holes are cut. Otherwise the nearest toolpath comes next,
/// starting from `start`; open toolpaths may be reversed. Ties go to the
/// earlier toolpath.
pub fn order_toolpaths(toolpaths: Vec<Toolpath>, start: Point) -> Vec<Toolpath> {
    let mut ordered = Vec::with_capacity(toolpaths.len());
    let mut position = start;
    let mut remaining = toolpaths.into_iter().peekable();
    while let Some(first) = remaining.next() {
        let layer = first.layer_id;
        let mut group = vec![first];
        while let Some(next) = remaining.next_if(|t| t.layer_id == layer) {
            group.push(next);
        }
        for toolpath in order_layer(group, position) {
            if let Some(&end) = toolpath.points.last() {
                position = end;
            }
            ordered.push(toolpath);
        }
    }
    ordered
}

fn order_layer(toolpaths: Vec<Toolpath>, start: Point) -> Vec<Toolpath> {
    let n = toolpaths.len();
    let bounds: Vec<Option<Rect>> = toolpaths
        .iter()
        .map(|t| Rect::from_points(t.points.iter().copied()))
        .collect();
    // uncut_inside[i]: uncut toolpaths inside i. outers[j]: toolpaths containing j.
    let mut uncut_inside = vec![0usize; n];
    let mut outers = vec![Vec::new(); n];
    for outer in 0..n {
        for inner in 0..n {
            if outer != inner
                && contains(
                    &toolpaths[outer],
                    bounds[outer],
                    &toolpaths[inner],
                    bounds[inner],
                )
            {
                uncut_inside[outer] += 1;
                outers[inner].push(outer);
            }
        }
    }

    let mut slots: Vec<Option<Toolpath>> = toolpaths.into_iter().map(Some).collect();
    let mut ordered = Vec::with_capacity(n);
    let mut position = start;
    while let Some((index, reverse)) = nearest_ready(&slots, &uncut_inside, position) {
        let Some(mut toolpath) = slots[index].take() else {
            break;
        };
        if reverse {
            toolpath.points.reverse();
        }
        for &outer in &outers[index] {
            uncut_inside[outer] -= 1;
        }
        if let Some(&end) = toolpath.points.last() {
            position = end;
        }
        ordered.push(toolpath);
    }
    // Unreachable while containment is acyclic; kept so nothing is ever dropped.
    ordered.extend(slots.into_iter().flatten());
    ordered
}

fn nearest_ready(
    slots: &[Option<Toolpath>],
    uncut_inside: &[usize],
    position: Point,
) -> Option<(usize, bool)> {
    let mut best: Option<(usize, bool, f64)> = None;
    for (index, slot) in slots.iter().enumerate() {
        let Some(toolpath) = slot else {
            continue;
        };
        if uncut_inside[index] > 0 {
            continue;
        }
        let (distance, reverse) = entry_distance(toolpath, position);
        if best.is_none_or(|(_, _, d)| distance < d) {
            best = Some((index, reverse, distance));
        }
    }
    best.map(|(index, reverse, _)| (index, reverse))
}

/// Distance to the nearest usable entry point, and whether entering there
/// means reversing the toolpath.
fn entry_distance(toolpath: &Toolpath, position: Point) -> (f64, bool) {
    let (Some(&first), Some(&last)) = (toolpath.points.first(), toolpath.points.last()) else {
        return (f64::INFINITY, false);
    };
    let forward = position.distance(first);
    if toolpath.closed {
        return (forward, false);
    }
    let backward = position.distance(last);
    if backward < forward {
        (backward, true)
    } else {
        (forward, false)
    }
}

/// Requiring a strictly smaller bounding box keeps containment acyclic, so
/// identical overlapping shapes never wait on each other.
fn contains(
    outer: &Toolpath,
    outer_bounds: Option<Rect>,
    inner: &Toolpath,
    inner_bounds: Option<Rect>,
) -> bool {
    let (Some(ob), Some(ib)) = (outer_bounds, inner_bounds) else {
        return false;
    };
    outer.closed
        && area(ib) < area(ob)
        && ob.contains_rect(&ib)
        && inner
            .points
            .first()
            .is_some_and(|&p| point_in_polygon(p, &outer.points))
}

fn area(rect: Rect) -> f64 {
    rect.width() * rect.height()
}

/// Even-odd rule. `polygon` must repeat its first point at the end.
fn point_in_polygon(point: Point, polygon: &[Point]) -> bool {
    let mut inside = false;
    for edge in polygon.windows(2) {
        let (a, b) = (edge[0], edge[1]);
        if (a.y > point.y) != (b.y > point.y) {
            let x = a.x + (point.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if point.x < x {
                inside = !inside;
            }
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::{LayerId, PowerMode};

    fn square(layer: u32, x: f64, y: f64, size: f64) -> Toolpath {
        let points = vec![
            Point::new(x, y),
            Point::new(x + size, y),
            Point::new(x + size, y + size),
            Point::new(x, y + size),
            Point::new(x, y),
        ];
        toolpath(layer, points, true)
    }

    fn line(layer: u32, from: Point, to: Point) -> Toolpath {
        toolpath(layer, vec![from, to], false)
    }

    fn toolpath(layer: u32, points: Vec<Point>, closed: bool) -> Toolpath {
        Toolpath {
            layer_id: LayerId(layer),
            points,
            closed,
            speed: 1000.0,
            power: 10.0,
            passes: 1,
            power_mode: PowerMode::M4Dynamic,
        }
    }

    fn starts(toolpaths: &[Toolpath]) -> Vec<Point> {
        toolpaths.iter().map(|t| t.points[0]).collect()
    }

    #[test]
    fn inner_is_cut_before_outer_even_when_outer_is_nearer() {
        let outer = square(0, 0.0, 0.0, 100.0);
        let inner = square(0, 40.0, 40.0, 20.0);
        let ordered = order_toolpaths(vec![outer, inner], Point::ORIGIN);
        assert_eq!(
            starts(&ordered),
            vec![Point::new(40.0, 40.0), Point::new(0.0, 0.0)]
        );
    }

    #[test]
    fn three_levels_of_nesting_cut_inside_out() {
        let a = square(0, 0.0, 0.0, 100.0);
        let b = square(0, 10.0, 10.0, 80.0);
        let c = square(0, 20.0, 20.0, 10.0);
        let ordered = order_toolpaths(vec![a, b, c], Point::ORIGIN);
        assert_eq!(
            starts(&ordered),
            vec![
                Point::new(20.0, 20.0),
                Point::new(10.0, 10.0),
                Point::new(0.0, 0.0)
            ]
        );
    }

    #[test]
    fn separate_shapes_follow_nearest_neighbor() {
        let far = square(0, 200.0, 0.0, 10.0);
        let near = square(0, 10.0, 0.0, 10.0);
        let middle = square(0, 100.0, 0.0, 10.0);
        let ordered = order_toolpaths(vec![far, near, middle], Point::ORIGIN);
        assert_eq!(
            starts(&ordered),
            vec![
                Point::new(10.0, 0.0),
                Point::new(100.0, 0.0),
                Point::new(200.0, 0.0)
            ]
        );
    }

    #[test]
    fn open_path_is_reversed_when_its_end_is_nearer() {
        let open = line(0, Point::new(50.0, 0.0), Point::new(5.0, 0.0));
        let ordered = order_toolpaths(vec![open], Point::ORIGIN);
        assert_eq!(
            ordered[0].points,
            vec![Point::new(5.0, 0.0), Point::new(50.0, 0.0)]
        );
    }

    #[test]
    fn open_path_inside_closed_is_cut_first() {
        let outer = square(0, 0.0, 0.0, 100.0);
        let inside = line(0, Point::new(40.0, 50.0), Point::new(60.0, 50.0));
        let ordered = order_toolpaths(vec![outer, inside], Point::ORIGIN);
        assert!(!ordered[0].closed);
        assert!(ordered[1].closed);
    }

    #[test]
    fn identical_shapes_do_not_block_each_other() {
        let a = square(0, 10.0, 10.0, 10.0);
        let b = square(0, 10.0, 10.0, 10.0);
        assert_eq!(order_toolpaths(vec![a, b], Point::ORIGIN).len(), 2);
    }

    #[test]
    fn layers_keep_their_order_and_position_carries_over() {
        let first_layer = square(0, 200.0, 0.0, 10.0);
        let near = square(1, 10.0, 0.0, 10.0);
        let near_far_end = square(1, 190.0, 0.0, 10.0);
        let ordered = order_toolpaths(vec![first_layer, near, near_far_end], Point::ORIGIN);
        assert_eq!(
            starts(&ordered),
            vec![
                Point::new(200.0, 0.0),
                Point::new(190.0, 0.0),
                Point::new(10.0, 0.0)
            ]
        );
    }

    #[test]
    fn ties_keep_input_order() {
        let a = square(0, 10.0, 0.0, 5.0);
        let b = square(0, 0.0, 10.0, 5.0);
        let ordered = order_toolpaths(vec![a.clone(), b.clone()], Point::ORIGIN);
        assert_eq!(ordered[0], a);
        let ordered = order_toolpaths(vec![b.clone(), a], Point::ORIGIN);
        assert_eq!(ordered[0], b);
    }

    #[test]
    fn point_in_polygon_basics() {
        let sq = square(0, 0.0, 0.0, 10.0).points;
        assert!(point_in_polygon(Point::new(5.0, 5.0), &sq));
        assert!(!point_in_polygon(Point::new(15.0, 5.0), &sq));
        assert!(!point_in_polygon(Point::new(5.0, -1.0), &sq));
    }
}
