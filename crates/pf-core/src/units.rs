use thiserror::Error;

pub const MM_PER_INCH: f64 = 25.4;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum UnitsError {
    #[error("invalid DPI {0}: must be finite and greater than zero")]
    InvalidDpi(f64),
}

pub fn mm_to_inch(mm: f64) -> f64 {
    mm / MM_PER_INCH
}

pub fn inch_to_mm(inch: f64) -> f64 {
    inch * MM_PER_INCH
}

pub fn px_to_mm(px: f64, dpi: f64) -> Result<f64, UnitsError> {
    let dpi = validate_dpi(dpi)?;
    Ok(px / dpi * MM_PER_INCH)
}

pub fn mm_to_px(mm: f64, dpi: f64) -> Result<f64, UnitsError> {
    let dpi = validate_dpi(dpi)?;
    Ok(mm / MM_PER_INCH * dpi)
}

pub fn deg_to_rad(deg: f64) -> f64 {
    deg.to_radians()
}

pub fn rad_to_deg(rad: f64) -> f64 {
    rad.to_degrees()
}

fn validate_dpi(dpi: f64) -> Result<f64, UnitsError> {
    if dpi.is_finite() && dpi > 0.0 {
        Ok(dpi)
    } else {
        Err(UnitsError::InvalidDpi(dpi))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{EPS, approx_eq};
    use std::f64::consts::PI;

    #[test]
    fn inch_mm_conversions() {
        assert!(approx_eq(inch_to_mm(1.0), 25.4, EPS));
        assert!(approx_eq(mm_to_inch(25.4), 1.0, EPS));
        assert!(approx_eq(mm_to_inch(inch_to_mm(3.7)), 3.7, EPS));
    }

    #[test]
    fn px_mm_conversions() {
        assert!(approx_eq(px_to_mm(96.0, 96.0).unwrap(), 25.4, EPS));
        assert!(approx_eq(mm_to_px(25.4, 300.0).unwrap(), 300.0, EPS));
        let round_trip = px_to_mm(mm_to_px(12.5, 254.0).unwrap(), 254.0).unwrap();
        assert!(approx_eq(round_trip, 12.5, EPS));
    }

    #[test]
    fn invalid_dpi_is_rejected() {
        for dpi in [0.0, -96.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                px_to_mm(10.0, dpi),
                Err(UnitsError::InvalidDpi(_))
            ));
            assert!(matches!(
                mm_to_px(10.0, dpi),
                Err(UnitsError::InvalidDpi(_))
            ));
        }
    }

    #[test]
    fn angle_conversions() {
        assert!(approx_eq(deg_to_rad(180.0), PI, EPS));
        assert!(approx_eq(rad_to_deg(PI / 2.0), 90.0, EPS));
        assert!(approx_eq(rad_to_deg(deg_to_rad(33.3)), 33.3, EPS));
    }
}
