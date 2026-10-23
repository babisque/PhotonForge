use thiserror::Error;

use crate::geometry::{Point, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Origin {
    #[default]
    BottomLeft,
    TopLeft,
    TopRight,
    BottomRight,
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum MachineError {
    #[error("invalid work area {width} x {height} mm: must be finite and greater than zero")]
    InvalidWorkArea { width: f64, height: f64 },
    #[error("invalid max S value {0}: must be greater than zero")]
    InvalidMaxSValue(u32),
    #[error("invalid max feed {0} mm/min: must be finite and greater than zero")]
    InvalidFeed(f64),
    #[error("invalid max power {0}%: must be greater than 0 and at most 100")]
    InvalidPowerLimit(f64),
    #[error("invalid frame power {power}%: must be between 0 and {max}")]
    InvalidFramePower { power: f64, max: f64 },
}

/// Machine limits. Fields are private so every profile passes validation.
/// The defaults are safety limits: never relax them.
#[derive(Debug, Clone, PartialEq)]
pub struct MachineProfile {
    work_area: Rect, // mm
    origin: Origin,
    max_s_value: u32, // GRBL $30
    max_feed_mm_min: f64,
    max_power_percent: f64,
    frame_power_percent: f64,
}

impl Default for MachineProfile {
    fn default() -> Self {
        MachineProfile {
            work_area: Rect::new(Point::ORIGIN, Point::new(300.0, 200.0)),
            origin: Origin::BottomLeft,
            max_s_value: 1000,
            max_feed_mm_min: 6000.0,
            max_power_percent: 100.0,
            frame_power_percent: 0.0,
        }
    }
}

impl MachineProfile {
    pub fn new(
        work_area: Rect,
        origin: Origin,
        max_s_value: u32,
        max_feed_mm_min: f64,
        max_power_percent: f64,
        frame_power_percent: f64,
    ) -> Result<MachineProfile, MachineError> {
        let corners_finite = [work_area.min, work_area.max]
            .iter()
            .all(|p| p.x.is_finite() && p.y.is_finite());
        let (width, height) = (work_area.width(), work_area.height());
        if !corners_finite || width <= 0.0 || height <= 0.0 {
            return Err(MachineError::InvalidWorkArea { width, height });
        }
        if max_s_value == 0 {
            return Err(MachineError::InvalidMaxSValue(max_s_value));
        }
        if !(max_feed_mm_min.is_finite() && max_feed_mm_min > 0.0) {
            return Err(MachineError::InvalidFeed(max_feed_mm_min));
        }
        if !(max_power_percent > 0.0 && max_power_percent <= 100.0) {
            return Err(MachineError::InvalidPowerLimit(max_power_percent));
        }
        if !(0.0..=max_power_percent).contains(&frame_power_percent) {
            return Err(MachineError::InvalidFramePower {
                power: frame_power_percent,
                max: max_power_percent,
            });
        }
        Ok(MachineProfile {
            work_area,
            origin,
            max_s_value,
            max_feed_mm_min,
            max_power_percent,
            frame_power_percent,
        })
    }

    pub fn work_area(&self) -> Rect {
        self.work_area
    }

    pub fn origin(&self) -> Origin {
        self.origin
    }

    pub fn max_s_value(&self) -> u32 {
        self.max_s_value
    }

    pub fn max_feed_mm_min(&self) -> f64 {
        self.max_feed_mm_min
    }

    pub fn max_power_percent(&self) -> f64 {
        self.max_power_percent
    }

    pub fn frame_power_percent(&self) -> f64 {
        self.frame_power_percent
    }
}

pub fn is_inside_work_area(profile: &MachineProfile, bounds: &Rect) -> bool {
    profile.work_area.contains_rect(bounds)
}

/// NaN maps to 0 so an invalid value can never turn the laser on.
pub fn clamp_power(profile: &MachineProfile, percent: f64) -> f64 {
    if percent.is_nan() {
        return 0.0;
    }
    percent.clamp(0.0, profile.max_power_percent)
}

/// NaN maps to 0.
pub fn clamp_feed(profile: &MachineProfile, mm_min: f64) -> f64 {
    if mm_min.is_nan() {
        return 0.0;
    }
    mm_min.clamp(0.0, profile.max_feed_mm_min)
}

/// GRBL `S` value for a power percentage, after clamping to the profile limit.
pub fn power_to_s(profile: &MachineProfile, percent: f64) -> u32 {
    let s = clamp_power(profile, percent) / 100.0 * f64::from(profile.max_s_value);
    // In range: clamped power is at most 100 %, so s <= max_s_value.
    s.round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{EPS, approx_eq};

    fn area(w: f64, h: f64) -> Rect {
        Rect::new(Point::ORIGIN, Point::new(w, h))
    }

    fn profile(max_power: f64, frame_power: f64) -> Result<MachineProfile, MachineError> {
        MachineProfile::new(
            area(300.0, 200.0),
            Origin::BottomLeft,
            1000,
            6000.0,
            max_power,
            frame_power,
        )
    }

    #[test]
    fn defaults_are_the_approved_safety_limits() {
        let p = MachineProfile::default();
        assert!(p.work_area().approx_eq(&area(300.0, 200.0), EPS));
        assert_eq!(p.origin(), Origin::BottomLeft);
        assert_eq!(p.max_s_value(), 1000);
        assert!(approx_eq(p.max_feed_mm_min(), 6000.0, EPS));
        assert!(approx_eq(p.max_power_percent(), 100.0, EPS));
        assert!(approx_eq(p.frame_power_percent(), 0.0, EPS));
    }

    #[test]
    fn default_passes_validation() {
        let d = MachineProfile::default();
        let validated = MachineProfile::new(
            d.work_area(),
            d.origin(),
            d.max_s_value(),
            d.max_feed_mm_min(),
            d.max_power_percent(),
            d.frame_power_percent(),
        );
        assert_eq!(validated, Ok(d));
    }

    #[test]
    fn invalid_work_area_is_rejected() {
        let bad_areas = [
            area(0.0, 200.0),
            area(300.0, 0.0),
            Rect::new(Point::ORIGIN, Point::new(f64::INFINITY, 200.0)),
            Rect::new(Point::new(f64::NAN, 0.0), Point::new(300.0, 200.0)),
        ];
        for work_area in bad_areas {
            let result =
                MachineProfile::new(work_area, Origin::BottomLeft, 1000, 6000.0, 100.0, 0.0);
            assert!(matches!(result, Err(MachineError::InvalidWorkArea { .. })));
        }
    }

    #[test]
    fn invalid_limits_are_rejected() {
        let zero_s = MachineProfile::new(area(10.0, 10.0), Origin::TopLeft, 0, 6000.0, 100.0, 0.0);
        assert_eq!(zero_s, Err(MachineError::InvalidMaxSValue(0)));

        for feed in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let result =
                MachineProfile::new(area(10.0, 10.0), Origin::TopLeft, 1000, feed, 100.0, 0.0);
            assert!(matches!(result, Err(MachineError::InvalidFeed(_))));
        }

        for max_power in [0.0, -5.0, 100.1, f64::NAN] {
            assert!(matches!(
                profile(max_power, 0.0),
                Err(MachineError::InvalidPowerLimit(_))
            ));
        }

        for frame_power in [-1.0, 50.1, f64::NAN] {
            assert!(matches!(
                profile(50.0, frame_power),
                Err(MachineError::InvalidFramePower { .. })
            ));
        }
        assert!(profile(50.0, 50.0).is_ok());
    }

    #[test]
    fn work_area_containment() {
        let p = MachineProfile::default();
        let inside = Rect::new(Point::new(10.0, 10.0), Point::new(50.0, 50.0));
        let touching = Rect::new(Point::new(250.0, 150.0), Point::new(300.0, 200.0));
        let outside = Rect::new(Point::new(280.0, 10.0), Point::new(301.0, 50.0));
        let negative = Rect::new(Point::new(-0.5, 10.0), Point::new(10.0, 20.0));
        assert!(is_inside_work_area(&p, &inside));
        assert!(is_inside_work_area(&p, &touching));
        assert!(is_inside_work_area(&p, &p.work_area()));
        assert!(!is_inside_work_area(&p, &outside));
        assert!(!is_inside_work_area(&p, &negative));
    }

    #[test]
    fn clamp_power_limits() {
        let p = profile(80.0, 0.0).unwrap();
        assert!(approx_eq(clamp_power(&p, 50.0), 50.0, EPS));
        assert!(approx_eq(clamp_power(&p, -10.0), 0.0, EPS));
        assert!(approx_eq(clamp_power(&p, 95.0), 80.0, EPS));
        assert!(approx_eq(clamp_power(&p, f64::INFINITY), 80.0, EPS));
        assert!(approx_eq(clamp_power(&p, f64::NAN), 0.0, EPS));
    }

    #[test]
    fn clamp_feed_limits() {
        let p = MachineProfile::default();
        assert!(approx_eq(clamp_feed(&p, 1500.0), 1500.0, EPS));
        assert!(approx_eq(clamp_feed(&p, 9000.0), 6000.0, EPS));
        assert!(approx_eq(clamp_feed(&p, -1.0), 0.0, EPS));
        assert!(approx_eq(clamp_feed(&p, f64::NAN), 0.0, EPS));
    }

    #[test]
    fn power_to_s_scales_and_clamps() {
        let p = MachineProfile::default();
        assert_eq!(power_to_s(&p, 0.0), 0);
        assert_eq!(power_to_s(&p, 50.0), 500);
        assert_eq!(power_to_s(&p, 100.0), 1000);
        assert_eq!(power_to_s(&p, 150.0), 1000);
        assert_eq!(power_to_s(&p, 12.34), 123);
        assert_eq!(power_to_s(&p, f64::NAN), 0);

        let limited = profile(50.0, 0.0).unwrap();
        assert_eq!(power_to_s(&limited, 100.0), 500);
    }
}
