//! Framing: the head traces the job's bounding box so the user can see
//! where it will land. The laser is off unless the profile allows a low
//! framing power.

use pf_core::{MachineProfile, Point, Rect, power_to_s};

use crate::GcodeError;
use crate::emit::format_coord;
use crate::toolpath::Toolpath;

/// Framing speed, capped at the profile's maximum feed.
pub const FRAME_SPEED_MM_MIN: f64 = 3000.0;

/// The box around every point of `toolpaths` (machine coordinates, as
/// `prepare_job` returns them). `None` when there are no points.
pub fn job_bounds(toolpaths: &[Toolpath]) -> Option<Rect> {
    Rect::from_points(toolpaths.iter().flat_map(|t| t.points.iter().copied()))
}

/// G-code that traces `bounds` once, counter-clockwise from its bottom-left
/// corner. With `power_percent` 0 the laser stays off (`M5`); above 0 it
/// runs in dynamic mode at that power, which may not exceed the profile's
/// frame power.
pub fn frame(
    bounds: Rect,
    profile: &MachineProfile,
    power_percent: f64,
) -> Result<String, GcodeError> {
    let max = profile.frame_power_percent();
    if !(power_percent >= 0.0 && power_percent <= max) {
        return Err(GcodeError::InvalidFramePower {
            power: power_percent,
            max,
        });
    }
    if !profile.work_area().contains_rect(&bounds) {
        return Err(GcodeError::FrameOutOfBounds { bounds });
    }
    // In range: the profile's max feed is finite and positive.
    let feed = FRAME_SPEED_MM_MIN.min(profile.max_feed_mm_min()).round() as u32;
    let s = power_to_s(profile, power_percent);
    let corners = [
        bounds.min,
        Point::new(bounds.max.x, bounds.min.y),
        bounds.max,
        Point::new(bounds.min.x, bounds.max.y),
        bounds.min,
    ];

    let mut out = String::from("; PhotonForge frame\nG21 G90\n");
    // Dynamic power: the laser goes dark whenever the head stops.
    out.push_str(if s == 0 { "M5\n" } else { "M4 S0\n" });
    out.push_str(&format!("G0 {}\n", xy(corners[0])));
    for (i, corner) in corners[1..].iter().enumerate() {
        out.push_str(&format!("G1 {}", xy(*corner)));
        if i == 0 {
            out.push_str(&format!(" F{feed}"));
            if s > 0 {
                out.push_str(&format!(" S{s}"));
            }
        }
        out.push('\n');
    }
    out.push_str("M5\n");
    Ok(out)
}

fn xy(point: Point) -> String {
    format!("X{} Y{}", format_coord(point.x), format_coord(point.y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::{LayerId, Origin, PowerMode};

    fn profile(frame_power: f64) -> MachineProfile {
        MachineProfile::new(
            Rect::new(Point::ORIGIN, Point::new(300.0, 200.0)),
            Origin::BottomLeft,
            1000,
            6000.0,
            100.0,
            frame_power,
        )
        .unwrap()
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect::new(Point::new(x0, y0), Point::new(x1, y1))
    }

    #[test]
    fn frame_traces_the_box_with_the_laser_off() {
        let gcode = frame(rect(10.0, 20.0, 60.5, 45.0), &profile(0.0), 0.0).unwrap();
        assert_eq!(
            gcode,
            "; PhotonForge frame\nG21 G90\nM5\nG0 X10 Y20\nG1 X60.5 Y20 F3000\n\
             G1 X60.5 Y45\nG1 X10 Y45\nG1 X10 Y20\nM5\n"
        );
        assert!(!gcode.contains('S'));
    }

    #[test]
    fn frame_power_is_limited_to_the_profile() {
        let gcode = frame(rect(0.0, 0.0, 10.0, 10.0), &profile(2.0), 1.5).unwrap();
        assert!(
            gcode.contains("M4 S0\nG0 X0 Y0\nG1 X10 Y0 F3000 S15\n"),
            "{gcode}"
        );
        for power in [2.5, -1.0, f64::NAN] {
            assert!(matches!(
                frame(rect(0.0, 0.0, 10.0, 10.0), &profile(2.0), power),
                Err(GcodeError::InvalidFramePower { .. })
            ));
        }
        // The default profile allows no framing power at all.
        assert!(frame(rect(0.0, 0.0, 10.0, 10.0), &MachineProfile::default(), 0.1).is_err());
    }

    #[test]
    fn frame_stays_inside_the_work_area_and_feed() {
        assert!(matches!(
            frame(rect(-1.0, 0.0, 10.0, 10.0), &profile(0.0), 0.0),
            Err(GcodeError::FrameOutOfBounds { .. })
        ));
        let slow = MachineProfile::new(
            Rect::new(Point::ORIGIN, Point::new(300.0, 200.0)),
            Origin::BottomLeft,
            1000,
            1200.0,
            100.0,
            0.0,
        )
        .unwrap();
        assert!(
            frame(rect(0.0, 0.0, 1.0, 1.0), &slow, 0.0)
                .unwrap()
                .contains("F1200")
        );
    }

    #[test]
    fn job_bounds_cover_every_point() {
        let toolpath = |points: Vec<Point>| Toolpath {
            layer_id: LayerId(0),
            points,
            closed: false,
            power: 50.0,
            speed: 1000.0,
            passes: 1,
            power_mode: PowerMode::M4Dynamic,
        };
        assert_eq!(job_bounds(&[]), None);
        let bounds = job_bounds(&[
            toolpath(vec![Point::new(5.0, 7.0), Point::new(9.0, 1.0)]),
            toolpath(vec![Point::new(2.0, 3.0)]),
        ]);
        assert_eq!(bounds, Some(rect(2.0, 1.0, 9.0, 7.0)));
    }
}
