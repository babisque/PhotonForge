use pf_core::{MachineProfile, Point};

use crate::toolpath::Toolpath;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JobEstimate {
    pub cut_mm: f64,
    pub travel_mm: f64,
    pub seconds: f64,
}

/// Distance over feed for validated toolpaths in machine coordinates, in the
/// same order and with the same passes as `emit`. The job starts at machine
/// (0, 0), travel runs at the machine max feed and acceleration is ignored,
/// so real jobs take longer.
pub fn estimate(toolpaths: &[Toolpath], profile: &MachineProfile) -> JobEstimate {
    let mut position = Point::ORIGIN;
    let mut cut_mm = 0.0;
    let mut travel_mm = 0.0;
    let mut cut_minutes = 0.0;
    for layer_paths in toolpaths.chunk_by(|a, b| a.layer_id == b.layer_id) {
        let passes = layer_paths.first().map_or(0, |t| t.passes);
        for _ in 0..passes {
            for toolpath in layer_paths {
                let (Some(&start), Some(&end)) = (toolpath.points.first(), toolpath.points.last())
                else {
                    continue;
                };
                travel_mm += position.distance(start);
                let length = toolpath.length();
                cut_mm += length;
                cut_minutes += length / toolpath.speed;
                position = end;
            }
        }
    }
    let travel_minutes = travel_mm / profile.max_feed_mm_min();
    JobEstimate {
        cut_mm,
        travel_mm,
        seconds: (cut_minutes + travel_minutes) * 60.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::{LayerId, PowerMode, approx_eq};

    fn square(layer: u32, x: f64, y: f64, size: f64, speed: f64, passes: u32) -> Toolpath {
        Toolpath {
            layer_id: LayerId(layer),
            points: vec![
                Point::new(x, y),
                Point::new(x + size, y),
                Point::new(x + size, y + size),
                Point::new(x, y + size),
                Point::new(x, y),
            ],
            closed: true,
            speed,
            power: 10.0,
            passes,
            power_mode: PowerMode::M4Dynamic,
        }
    }

    #[test]
    fn single_square() {
        // 40 mm perimeter at 600 mm/min = 4 s; travel 5 mm at 6000 mm/min = 0.05 s.
        let e = estimate(
            &[square(0, 3.0, 4.0, 10.0, 600.0, 1)],
            &MachineProfile::default(),
        );
        assert!(approx_eq(e.cut_mm, 40.0, 1e-9));
        assert!(approx_eq(e.travel_mm, 5.0, 1e-9));
        assert!(approx_eq(e.seconds, 4.05, 1e-9));
    }

    #[test]
    fn passes_multiply_cutting_but_not_the_first_travel() {
        let e = estimate(
            &[square(0, 3.0, 4.0, 10.0, 600.0, 2)],
            &MachineProfile::default(),
        );
        assert!(approx_eq(e.cut_mm, 80.0, 1e-9));
        assert!(approx_eq(e.travel_mm, 5.0, 1e-9));
        assert!(approx_eq(e.seconds, 8.05, 1e-9));
    }

    #[test]
    fn travel_between_toolpaths_and_layers() {
        // a starts at machine zero (no travel); then 10 mm of travel to b.
        let a = square(0, 0.0, 0.0, 1.0, 60.0, 1);
        let b = square(1, 10.0, 0.0, 1.0, 120.0, 1);
        let e = estimate(&[a, b], &MachineProfile::default());
        assert!(approx_eq(e.cut_mm, 8.0, 1e-9));
        assert!(approx_eq(e.travel_mm, 10.0, 1e-9));
        // 4 s + 2 s cutting, 0.1 s travel.
        assert!(approx_eq(e.seconds, 6.1, 1e-9));
    }

    #[test]
    fn empty_job_takes_no_time() {
        let e = estimate(&[], &MachineProfile::default());
        assert_eq!(
            e,
            JobEstimate {
                cut_mm: 0.0,
                travel_mm: 0.0,
                seconds: 0.0
            }
        );
    }
}
