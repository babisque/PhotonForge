use pf_core::{LayerId, MachineProfile, Point};

use crate::estimate::{JobEstimate, estimate};
use crate::toolpath::Toolpath;

/// What a job will do, for drawing: laser-on polylines, laser-off moves
/// between them and the time estimate. Machine coordinates (mm).
#[derive(Debug, Clone, PartialEq)]
pub struct JobPreview {
    /// One entry per toolpath; repeated passes are not repeated here.
    pub cuts: Vec<(LayerId, Vec<Point>)>,
    /// Rapid moves in execution order, passes included. Zero-length moves
    /// are left out.
    pub travels: Vec<(Point, Point)>,
    pub estimate: JobEstimate,
}

/// Follows toolpaths from `prepare_job` in the order `emit` writes them,
/// starting at machine (0, 0) like `estimate`.
pub fn preview(toolpaths: &[Toolpath], profile: &MachineProfile) -> JobPreview {
    let cuts = toolpaths
        .iter()
        .map(|t| (t.layer_id, t.points.clone()))
        .collect();

    let mut travels = Vec::new();
    let mut position = Point::ORIGIN;
    for layer_paths in toolpaths.chunk_by(|a, b| a.layer_id == b.layer_id) {
        let passes = layer_paths.first().map_or(0, |t| t.passes);
        for _ in 0..passes {
            for toolpath in layer_paths {
                let (Some(&start), Some(&end)) = (toolpath.points.first(), toolpath.points.last())
                else {
                    continue;
                };
                if position != start {
                    travels.push((position, start));
                }
                position = end;
            }
        }
    }

    JobPreview {
        cuts,
        travels,
        estimate: estimate(toolpaths, profile),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::PowerMode;

    fn path(layer: u32, points: &[(f64, f64)], passes: u32) -> Toolpath {
        Toolpath {
            layer_id: LayerId(layer),
            points: points.iter().map(|&(x, y)| Point::new(x, y)).collect(),
            closed: false,
            speed: 600.0,
            power: 10.0,
            passes,
            power_mode: PowerMode::M4Dynamic,
        }
    }

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn travels_follow_emit_order_with_passes() {
        // Layer 0: two lines, two passes. Layer 1: one line, one pass.
        let toolpaths = [
            path(0, &[(1.0, 0.0), (2.0, 0.0)], 2),
            path(0, &[(5.0, 0.0), (5.0, 3.0)], 2),
            path(1, &[(5.0, 3.0), (9.0, 3.0)], 1),
        ];
        let result = preview(&toolpaths, &MachineProfile::default());
        assert_eq!(
            result.travels,
            vec![
                (p(0.0, 0.0), p(1.0, 0.0)),
                (p(2.0, 0.0), p(5.0, 0.0)),
                // Second pass returns to the first line.
                (p(5.0, 3.0), p(1.0, 0.0)),
                (p(2.0, 0.0), p(5.0, 0.0)),
                // Layer 1 starts where layer 0 ended: no travel.
            ]
        );
        assert_eq!(result.cuts.len(), 3);
        assert_eq!(result.cuts[2], (LayerId(1), vec![p(5.0, 3.0), p(9.0, 3.0)]));
    }

    #[test]
    fn estimate_matches_the_estimate_function() {
        let toolpaths = [
            path(0, &[(3.0, 4.0), (13.0, 4.0)], 3),
            path(1, &[(20.0, 0.0), (20.0, 10.0)], 1),
        ];
        let profile = MachineProfile::default();
        let result = preview(&toolpaths, &profile);
        assert_eq!(result.estimate, estimate(&toolpaths, &profile));
        // Travel distance equals the sum of the drawn travel moves.
        let drawn: f64 = result.travels.iter().map(|(a, b)| a.distance(*b)).sum();
        assert!((drawn - result.estimate.travel_mm).abs() < 1e-9);
    }

    #[test]
    fn empty_and_zero_pass_toolpaths_add_no_travel() {
        let toolpaths = [path(0, &[], 1), path(1, &[(4.0, 3.0), (5.0, 3.0)], 0)];
        let result = preview(&toolpaths, &MachineProfile::default());
        assert!(result.travels.is_empty());
        assert_eq!(result.cuts.len(), 2);
        assert_eq!(result.estimate.travel_mm, 0.0);
    }
}
