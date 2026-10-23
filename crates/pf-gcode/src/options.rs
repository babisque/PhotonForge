use pf_core::Point;

/// Default chord tolerance for flattening curves (mm).
pub const DEFAULT_FLATTEN_TOLERANCE: f64 = 0.01;

/// Where the job sits relative to machine coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum JobOrigin {
    /// Document coordinates are machine coordinates.
    #[default]
    Absolute,
    /// This document point becomes machine (0, 0).
    UserOrigin(Point),
}

impl JobOrigin {
    /// The document point that maps to machine (0, 0).
    pub fn document_zero(self) -> Point {
        match self {
            JobOrigin::Absolute => Point::ORIGIN,
            JobOrigin::UserOrigin(point) => point,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobOptions {
    pub origin: JobOrigin,
    /// Emitted verbatim after the built-in header.
    pub header: Option<String>,
    /// Emitted verbatim before the final `M5`.
    pub footer: Option<String>,
    pub flatten_tolerance: f64, // mm
}

impl Default for JobOptions {
    fn default() -> Self {
        JobOptions {
            origin: JobOrigin::Absolute,
            header: None,
            footer: None,
            flatten_tolerance: DEFAULT_FLATTEN_TOLERANCE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_zero_per_origin() {
        assert_eq!(JobOrigin::Absolute.document_zero(), Point::ORIGIN);
        let p = Point::new(12.5, 40.0);
        assert_eq!(JobOrigin::UserOrigin(p).document_zero(), p);
    }

    #[test]
    fn defaults() {
        let options = JobOptions::default();
        assert_eq!(options.origin, JobOrigin::Absolute);
        assert_eq!(options.header, None);
        assert_eq!(options.footer, None);
        assert_eq!(options.flatten_tolerance, DEFAULT_FLATTEN_TOLERANCE);
    }
}
