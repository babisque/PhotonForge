//! Jog moves, without I/O: a requested step becomes a `$J=` command that
//! cannot leave the work area.
//!
//! Syntax: GRBL v1.1 `doc/markdown/jogging.md`. `$J=` takes axis words and
//! a feed, which is not modal; `G91`/`G21` apply to that one command only.
//! A jog carries GRBL's current spindle state and S (`gcode.c`, jog branch),
//! so the caller must know the laser is off (`M5`) before jogging.

use pf_core::MachineProfile;
use thiserror::Error;

/// A relative move in mm at a feed in mm/min.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JogMove {
    pub dx: f64,
    pub dy: f64,
    pub feed_mm_min: f64,
}

impl JogMove {
    /// `distance_mm` in direction (`-1`, `0` or `1` per axis).
    pub fn step(direction: [i8; 2], distance_mm: f64, feed_mm_min: f64) -> JogMove {
        JogMove {
            dx: f64::from(direction[0].signum()) * distance_mm,
            dy: f64::from(direction[1].signum()) * distance_mm,
            feed_mm_min,
        }
    }

    /// Toward the edge of the work area in direction (`-1`, `0` or `1` per
    /// axis), for a jog that runs while a button is held and is cancelled
    /// on release. Clamping stops it at the edge.
    pub fn to_edge(direction: [i8; 2], feed_mm_min: f64, profile: &MachineProfile) -> JogMove {
        let area = profile.work_area();
        let reach = area.width().max(area.height());
        JogMove {
            dx: f64::from(direction[0].signum()) * reach,
            dy: f64::from(direction[1].signum()) * reach,
            feed_mm_min,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum JogError {
    #[error("jog feed {0} mm/min is invalid: it must be a number above 0")]
    InvalidFeed(f64),
    #[error("jog step ({dx}, {dy}) mm is invalid")]
    InvalidStep { dx: f64, dy: f64 },
    #[error("the head is at the edge of the work area in that direction")]
    AtLimit,
}

/// `$J=G91 G21 …` for a jog that `plan_jog` checked. Only this module can
/// make one, so every jog written passed the work area clamp.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JogLine(String);

impl JogLine {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The step after clamping, and its command.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlannedJog {
    pub(crate) dx: f64,
    pub(crate) dy: f64,
    pub(crate) feed_mm_min: f64,
    pub(crate) line: JogLine,
}

/// Clamps `step` so the head, now at `position` (machine mm), stays inside
/// the profile's work area, and caps the feed at the profile's maximum. A
/// step never grows: from outside the area the head may only move back in.
pub(crate) fn plan_jog(
    step: JogMove,
    position: [f64; 2],
    profile: &MachineProfile,
) -> Result<PlannedJog, JogError> {
    if !(step.dx.is_finite() && step.dy.is_finite()) {
        return Err(JogError::InvalidStep {
            dx: step.dx,
            dy: step.dy,
        });
    }
    if !(step.feed_mm_min.is_finite() && step.feed_mm_min > 0.0) {
        return Err(JogError::InvalidFeed(step.feed_mm_min));
    }
    let area = profile.work_area();
    let dx = clamp_axis(step.dx, position[0], area.min.x, area.max.x);
    let dy = clamp_axis(step.dy, position[1], area.min.y, area.max.y);
    // Below 1 µm the formatted step would be zero.
    if dx.abs() < 0.001 && dy.abs() < 0.001 {
        return Err(JogError::AtLimit);
    }
    let feed_mm_min = step.feed_mm_min.min(profile.max_feed_mm_min());
    let mut line = String::from("$J=G91 G21");
    for (axis, value) in [('X', dx), ('Y', dy)] {
        if value.abs() >= 0.001 {
            line.push_str(&format!(" {axis}{}", format_mm(value)));
        }
    }
    line.push_str(&format!(" F{}", format_mm(feed_mm_min)));
    Ok(PlannedJog {
        dx,
        dy,
        feed_mm_min,
        line: JogLine(line),
    })
}

fn clamp_axis(delta: f64, position: f64, min: f64, max: f64) -> f64 {
    if delta > 0.0 {
        delta.min((max - position).max(0.0))
    } else {
        delta.max((min - position).min(0.0))
    }
}

/// 3 decimals, trailing zeros trimmed, rounded toward zero so a clamped
/// step never rounds past the edge.
fn format_mm(value: f64) -> String {
    let truncated = (value * 1000.0).trunc() / 1000.0;
    let mut text = format!("{truncated:.3}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.').len();
    text.truncate(trimmed);
    if text == "-0" {
        text = String::from("0");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::{Origin, Point, Rect};

    fn profile() -> MachineProfile {
        MachineProfile::new(
            Rect::new(Point::ORIGIN, Point::new(300.0, 200.0)),
            Origin::BottomLeft,
            1000,
            6000.0,
            100.0,
            0.0,
        )
        .unwrap()
    }

    fn step(dx: f64, dy: f64) -> JogMove {
        JogMove {
            dx,
            dy,
            feed_mm_min: 1500.0,
        }
    }

    fn line(step: JogMove, at: [f64; 2]) -> String {
        plan_jog(step, at, &profile())
            .unwrap()
            .line
            .as_str()
            .to_owned()
    }

    #[test]
    fn jog_lines_are_relative_millimeter_moves() {
        assert_eq!(
            line(step(10.0, -2.5), [50.0, 50.0]),
            "$J=G91 G21 X10 Y-2.5 F1500"
        );
        assert_eq!(line(step(0.1, 0.0), [50.0, 50.0]), "$J=G91 G21 X0.1 F1500");
        assert_eq!(line(step(0.0, 1.0), [50.0, 50.0]), "$J=G91 G21 Y1 F1500");
    }

    #[test]
    fn jogs_stop_at_every_edge_and_corner() {
        assert_eq!(line(step(50.0, 0.0), [280.0, 10.0]), "$J=G91 G21 X20 F1500");
        assert_eq!(
            line(step(-50.0, 0.0), [12.5, 10.0]),
            "$J=G91 G21 X-12.5 F1500"
        );
        assert_eq!(line(step(0.0, 50.0), [10.0, 199.0]), "$J=G91 G21 Y1 F1500");
        assert_eq!(line(step(0.0, -50.0), [10.0, 3.0]), "$J=G91 G21 Y-3 F1500");
        assert_eq!(
            line(step(100.0, 100.0), [290.0, 195.0]),
            "$J=G91 G21 X10 Y5 F1500"
        );
        assert_eq!(
            line(step(-100.0, -100.0), [1.0, 2.0]),
            "$J=G91 G21 X-1 Y-2 F1500"
        );
        for (s, at) in [
            (step(1.0, 0.0), [300.0, 50.0]),
            (step(-1.0, 0.0), [0.0, 50.0]),
            (step(0.0, 1.0), [50.0, 200.0]),
            (step(0.0, -1.0), [50.0, 0.0]),
        ] {
            assert_eq!(plan_jog(s, at, &profile()), Err(JogError::AtLimit));
        }
    }

    #[test]
    fn rounding_never_passes_the_edge() {
        let planned = plan_jog(step(10.0, 0.0), [299.99951, 10.0], &profile());
        assert_eq!(planned, Err(JogError::AtLimit));
        assert_eq!(
            line(step(10.0, 0.0), [299.9985, 10.0]),
            "$J=G91 G21 X0.001 F1500"
        );
    }

    #[test]
    fn from_outside_the_area_the_head_may_only_come_back() {
        // Further out: refused. Back in: allowed, and never more than asked.
        assert_eq!(
            plan_jog(step(-5.0, 0.0), [-3.0, 10.0], &profile()),
            Err(JogError::AtLimit)
        );
        assert_eq!(line(step(2.0, 0.0), [-3.0, 10.0]), "$J=G91 G21 X2 F1500");
    }

    #[test]
    fn feed_is_capped_and_bad_input_refused() {
        let fast = JogMove {
            dx: 1.0,
            dy: 0.0,
            feed_mm_min: 99_999.0,
        };
        assert_eq!(line(fast, [5.0, 5.0]), "$J=G91 G21 X1 F6000");
        for feed in [0.0, -10.0, f64::NAN, f64::INFINITY] {
            let jog = JogMove {
                dx: 1.0,
                dy: 0.0,
                feed_mm_min: feed,
            };
            assert!(matches!(
                plan_jog(jog, [5.0, 5.0], &profile()),
                Err(JogError::InvalidFeed(_))
            ));
        }
        assert!(matches!(
            plan_jog(step(f64::NAN, 0.0), [5.0, 5.0], &profile()),
            Err(JogError::InvalidStep { .. })
        ));
    }

    #[test]
    fn edge_jogs_reach_across_the_area_and_get_clamped() {
        let p = profile();
        let jog = JogMove::to_edge([1, -1], 800.0, &p);
        assert_eq!((jog.dx, jog.dy), (300.0, -300.0));
        assert_eq!(
            plan_jog(jog, [100.0, 50.0], &p).unwrap().line.as_str(),
            "$J=G91 G21 X200 Y-50 F800"
        );
        let jog = JogMove::to_edge([0, 5], 800.0, &p);
        assert_eq!((jog.dx, jog.dy), (0.0, 300.0));
        let jog = JogMove::step([-1, 0], 0.1, 800.0);
        assert_eq!((jog.dx, jog.dy, jog.feed_mm_min), (-0.1, 0.0, 800.0));
    }
}
