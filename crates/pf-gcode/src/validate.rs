use pf_core::{MachineProfile, Rect};

use crate::GcodeError;
use crate::toolpath::Toolpath;

/// Rejects anything that must never reach the machine. Run it on toolpaths
/// in machine coordinates, exactly as they will be emitted.
pub fn validate(toolpaths: &[Toolpath], profile: &MachineProfile) -> Result<(), GcodeError> {
    let work_area = profile.work_area();
    let max_power = profile.max_power_percent();
    let max_feed = profile.max_feed_mm_min();
    for t in toolpaths {
        let layer = t.layer_id;
        if t.power.is_nan() || t.power < 0.0 {
            return Err(GcodeError::InvalidPower {
                layer,
                power: t.power,
            });
        }
        if t.power > max_power {
            return Err(GcodeError::PowerExceedsLimit {
                layer,
                power: t.power,
                max: max_power,
            });
        }
        // F is emitted as an integer, so below 1 mm/min it would round to F0.
        if !(t.speed.is_finite() && t.speed >= 1.0 && t.speed <= max_feed) {
            return Err(GcodeError::InvalidSpeed {
                layer,
                speed: t.speed,
                max: max_feed,
            });
        }
        // The work area is convex, so travel moves between checked points stay inside too.
        if let Some(bounds) = Rect::from_points(t.points.iter().copied())
            && !t.points.iter().all(|&p| work_area.contains_point(p))
        {
            return Err(GcodeError::OutOfBounds { layer, bounds });
        }
    }
    Ok(())
}

/// GRBL acts on `?`, `!`, `~`, 0x18 and bytes above 0x7F the moment they
/// arrive, even inside comments (grbl `serial.c`, real-time commands in
/// `config.h`), so a template may only hold printable ASCII without them.
pub fn validate_template(template: &'static str, text: &str) -> Result<(), GcodeError> {
    match text.chars().find(|&c| !is_safe_template_char(c)) {
        Some(ch) => Err(GcodeError::InvalidTemplateChar { template, ch }),
        None => Ok(()),
    }
}

fn is_safe_template_char(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\t' | ' ') || (c.is_ascii_graphic() && !matches!(c, '?' | '!' | '~'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::{LayerId, Origin, Point, PowerMode};

    fn toolpath(points: Vec<Point>) -> Toolpath {
        Toolpath {
            layer_id: LayerId(3),
            points,
            closed: false,
            speed: 1000.0,
            power: 50.0,
            passes: 1,
            power_mode: PowerMode::M4Dynamic,
        }
    }

    fn inside() -> Toolpath {
        toolpath(vec![Point::new(10.0, 10.0), Point::new(20.0, 20.0)])
    }

    fn limited_profile(max_power: f64) -> MachineProfile {
        MachineProfile::new(
            Rect::new(Point::ORIGIN, Point::new(300.0, 200.0)),
            Origin::BottomLeft,
            1000,
            6000.0,
            max_power,
            0.0,
        )
        .unwrap()
    }

    #[test]
    fn valid_toolpath_passes() {
        assert_eq!(validate(&[inside()], &MachineProfile::default()), Ok(()));
    }

    #[test]
    fn toolpath_on_the_edge_passes() {
        let t = toolpath(vec![Point::new(0.0, 0.0), Point::new(300.0, 200.0)]);
        assert_eq!(validate(&[t], &MachineProfile::default()), Ok(()));
    }

    #[test]
    fn toolpath_past_the_right_edge_is_rejected() {
        let t = toolpath(vec![Point::new(290.0, 10.0), Point::new(300.5, 10.0)]);
        let err = validate(&[inside(), t], &MachineProfile::default()).unwrap_err();
        assert_eq!(
            err,
            GcodeError::OutOfBounds {
                layer: LayerId(3),
                bounds: Rect::new(Point::new(290.0, 10.0), Point::new(300.5, 10.0)),
            }
        );
    }

    #[test]
    fn negative_coordinate_is_rejected() {
        let t = toolpath(vec![Point::new(-0.1, 10.0), Point::new(10.0, 10.0)]);
        assert!(matches!(
            validate(&[t], &MachineProfile::default()),
            Err(GcodeError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn nan_coordinate_is_rejected() {
        let t = toolpath(vec![Point::new(f64::NAN, 10.0), Point::new(10.0, 10.0)]);
        assert!(matches!(
            validate(&[t], &MachineProfile::default()),
            Err(GcodeError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn power_over_the_limit_is_rejected_not_clamped() {
        let mut t = inside();
        t.power = 80.1;
        assert_eq!(
            validate(&[t], &limited_profile(80.0)),
            Err(GcodeError::PowerExceedsLimit {
                layer: LayerId(3),
                power: 80.1,
                max: 80.0
            })
        );
        let mut t = inside();
        t.power = 80.0;
        assert_eq!(validate(&[t], &limited_profile(80.0)), Ok(()));
    }

    #[test]
    fn invalid_power_is_rejected() {
        for power in [f64::NAN, -1.0] {
            let mut t = inside();
            t.power = power;
            assert!(matches!(
                validate(&[t], &MachineProfile::default()),
                Err(GcodeError::InvalidPower { .. })
            ));
        }
        let mut t = inside();
        t.power = f64::INFINITY;
        assert!(matches!(
            validate(&[t], &MachineProfile::default()),
            Err(GcodeError::PowerExceedsLimit { .. })
        ));
    }

    #[test]
    fn invalid_speed_is_rejected() {
        for speed in [0.0, 0.5, -10.0, 6000.5, f64::NAN, f64::INFINITY] {
            let mut t = inside();
            t.speed = speed;
            assert!(matches!(
                validate(&[t], &MachineProfile::default()),
                Err(GcodeError::InvalidSpeed { .. })
            ));
        }
    }

    #[test]
    fn templates_reject_grbl_realtime_characters() {
        assert_eq!(validate_template("header", "M8\nG4 P0.5 ; air\r\n"), Ok(()));
        for ch in ['?', '!', '~', '\u{18}', '\u{85}', 'é'] {
            let text = format!("M8 ; {ch}");
            assert_eq!(
                validate_template("footer", &text),
                Err(GcodeError::InvalidTemplateChar {
                    template: "footer",
                    ch
                })
            );
        }
    }
}
