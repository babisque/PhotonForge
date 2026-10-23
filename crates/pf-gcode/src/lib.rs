//! Toolpath to G-code generation for PhotonForge.
//!
//! Pure functions that turn a `pf-core` document into G-code text. No I/O.
//!
//! Pipeline: `plan_toolpaths` → `order_toolpaths` → place at the job origin
//! → `validate` → `emit`.

pub mod emit;
pub mod estimate;
pub mod options;
pub mod order;
pub mod preview;
pub mod toolpath;
pub mod validate;

use pf_core::{Document, GeometryError, LayerId, MachineProfile, Origin, Point, Rect};
use thiserror::Error;

pub use emit::{emit, format_coord};
pub use estimate::{JobEstimate, estimate};
pub use options::{DEFAULT_FLATTEN_TOLERANCE, JobOptions, JobOrigin};
pub use order::order_toolpaths;
pub use preview::{JobPreview, preview};
pub use toolpath::{Toolpath, plan_toolpaths};
pub use validate::{validate, validate_template};

#[derive(Debug, Clone, PartialEq, Error)]
pub enum GcodeError {
    #[error("layer {layer:?} has a toolpath outside the work area (toolpath bounds {bounds:?})")]
    OutOfBounds { layer: LayerId, bounds: Rect },
    #[error("layer {layer:?} power {power}% exceeds the machine limit of {max}%")]
    PowerExceedsLimit {
        layer: LayerId,
        power: f64,
        max: f64,
    },
    #[error("layer {layer:?} power {power}% is invalid: must be a number of at least 0")]
    InvalidPower { layer: LayerId, power: f64 },
    #[error("layer {layer:?} speed {speed} mm/min is invalid: must be between 1 and {max} mm/min")]
    InvalidSpeed {
        layer: LayerId,
        speed: f64,
        max: f64,
    },
    #[error("machine origin {0:?} is not supported yet; only BottomLeft is")]
    UnsupportedOrigin(Origin),
    #[error("custom {template} contains {ch:?}, which GRBL cannot safely receive")]
    InvalidTemplateChar { template: &'static str, ch: char },
    #[error("nothing to output: no enabled line layer has any shapes")]
    NothingToOutput,
    #[error(transparent)]
    Geometry(#[from] GeometryError),
}

/// G-code for the document's line layers. Identical input gives identical output.
pub fn generate(
    doc: &Document,
    profile: &MachineProfile,
    options: &JobOptions,
) -> Result<String, GcodeError> {
    let toolpaths = prepare_job(doc, profile, options)?;
    Ok(emit(&toolpaths, doc, profile, options))
}

/// Time estimate for the job `generate` would produce.
pub fn estimate_job(
    doc: &Document,
    profile: &MachineProfile,
    options: &JobOptions,
) -> Result<JobEstimate, GcodeError> {
    let toolpaths = prepare_job(doc, profile, options)?;
    Ok(estimate(&toolpaths, profile))
}

/// Toolpaths in machine coordinates and cut order, validated: exactly what
/// `generate` writes. Use it to preview a job.
pub fn prepare_job(
    doc: &Document,
    profile: &MachineProfile,
    options: &JobOptions,
) -> Result<Vec<Toolpath>, GcodeError> {
    if profile.origin() != Origin::BottomLeft {
        return Err(GcodeError::UnsupportedOrigin(profile.origin()));
    }
    if let Some(header) = &options.header {
        validate_template("header", header)?;
    }
    if let Some(footer) = &options.footer {
        validate_template("footer", footer)?;
    }
    let planned = plan_toolpaths(doc, options.flatten_tolerance)?;
    if planned.is_empty() {
        return Err(GcodeError::NothingToOutput);
    }
    let zero = options.origin.document_zero();
    let placed = place(order_toolpaths(planned, zero), zero);
    validate(&placed, profile)?;
    Ok(placed)
}

/// Moves document coordinates so `zero` lands on machine (0, 0).
fn place(mut toolpaths: Vec<Toolpath>, zero: Point) -> Vec<Toolpath> {
    for toolpath in &mut toolpaths {
        for point in &mut toolpath.points {
            *point = Point::new(point.x - zero.x, point.y - zero.y);
        }
    }
    toolpaths
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::{LayerMode, RectShape, ShapeKind, Transform};

    fn doc_with_rect(x: f64, y: f64) -> Document {
        let mut doc = Document::new();
        let layer = doc.add_layer("Cut", [0, 0, 0]).unwrap();
        let rect = ShapeKind::Rect(RectShape::new(20.0, 20.0, 0.0).unwrap());
        doc.add_shape(rect, Transform::translate(x, y), layer)
            .unwrap();
        doc
    }

    fn profile_with_origin(origin: Origin) -> MachineProfile {
        let d = MachineProfile::default();
        MachineProfile::new(
            d.work_area(),
            origin,
            d.max_s_value(),
            d.max_feed_mm_min(),
            d.max_power_percent(),
            d.frame_power_percent(),
        )
        .unwrap()
    }

    #[test]
    fn output_is_deterministic() {
        let doc = doc_with_rect(10.0, 10.0);
        let profile = MachineProfile::default();
        let options = JobOptions::default();
        assert_eq!(
            generate(&doc, &profile, &options).unwrap(),
            generate(&doc, &profile, &options).unwrap()
        );
    }

    #[test]
    fn laser_power_is_zero_until_after_the_first_rapid() {
        let out = generate(
            &doc_with_rect(10.0, 10.0),
            &MachineProfile::default(),
            &JobOptions::default(),
        )
        .unwrap();
        let before_first_rapid: Vec<&str> =
            out.lines().take_while(|l| !l.starts_with("G0 ")).collect();
        for line in before_first_rapid {
            assert!(!line.starts_with("G1"), "cut before first G0: {line}");
            if line.starts_with("M3") || line.starts_with("M4") {
                assert!(line.ends_with(" S0"), "laser on before first G0: {line}");
            }
        }
    }

    #[test]
    fn non_bottom_left_origins_are_rejected() {
        for origin in [Origin::TopLeft, Origin::TopRight, Origin::BottomRight] {
            assert_eq!(
                generate(
                    &doc_with_rect(10.0, 10.0),
                    &profile_with_origin(origin),
                    &JobOptions::default()
                ),
                Err(GcodeError::UnsupportedOrigin(origin))
            );
        }
    }

    #[test]
    fn nothing_to_output() {
        let profile = MachineProfile::default();
        let options = JobOptions::default();
        assert_eq!(
            generate(&Document::new(), &profile, &options),
            Err(GcodeError::NothingToOutput)
        );
        let mut doc = doc_with_rect(10.0, 10.0);
        let layer = doc.layers()[0].id;
        doc.update_layer(layer, |l| l.mode = LayerMode::Image)
            .unwrap();
        assert_eq!(
            generate(&doc, &profile, &options),
            Err(GcodeError::NothingToOutput)
        );
    }

    #[test]
    fn out_of_bounds_document_fails() {
        let result = generate(
            &doc_with_rect(290.0, 10.0),
            &MachineProfile::default(),
            &JobOptions::default(),
        );
        assert!(matches!(result, Err(GcodeError::OutOfBounds { .. })));
    }

    #[test]
    fn over_power_layer_fails() {
        let mut doc = doc_with_rect(10.0, 10.0);
        let layer = doc.layers()[0].id;
        doc.update_layer(layer, |l| l.power_percent = 100.5)
            .unwrap();
        let result = generate(&doc, &MachineProfile::default(), &JobOptions::default());
        assert!(matches!(result, Err(GcodeError::PowerExceedsLimit { .. })));
    }

    #[test]
    fn user_origin_is_validated_after_placement() {
        let profile = MachineProfile::default();
        // In bounds as a document, but the origin shift pushes it below machine zero.
        let options = JobOptions {
            origin: JobOrigin::UserOrigin(Point::new(50.0, 50.0)),
            ..JobOptions::default()
        };
        assert!(matches!(
            generate(&doc_with_rect(10.0, 10.0), &profile, &options),
            Err(GcodeError::OutOfBounds { .. })
        ));
        // Off the bed as a document, but shifted into the work area.
        let options = JobOptions {
            origin: JobOrigin::UserOrigin(Point::new(300.0, 0.0)),
            ..JobOptions::default()
        };
        assert!(generate(&doc_with_rect(310.0, 10.0), &profile, &options).is_ok());
    }

    #[test]
    fn unsafe_templates_are_rejected() {
        let options = JobOptions {
            footer: Some(String::from("M9 ; done!")),
            ..JobOptions::default()
        };
        assert_eq!(
            generate(
                &doc_with_rect(10.0, 10.0),
                &MachineProfile::default(),
                &options
            ),
            Err(GcodeError::InvalidTemplateChar {
                template: "footer",
                ch: '!'
            })
        );
    }

    #[test]
    fn invalid_tolerance_surfaces_as_geometry_error() {
        let options = JobOptions {
            flatten_tolerance: -1.0,
            ..JobOptions::default()
        };
        assert_eq!(
            generate(
                &doc_with_rect(10.0, 10.0),
                &MachineProfile::default(),
                &options
            ),
            Err(GcodeError::Geometry(GeometryError::InvalidTolerance(-1.0)))
        );
    }

    #[test]
    fn preview_of_prepared_job_matches_estimate_job() {
        let doc = doc_with_rect(10.0, 10.0);
        let profile = MachineProfile::default();
        let options = JobOptions::default();
        let toolpaths = prepare_job(&doc, &profile, &options).unwrap();
        let result = preview(&toolpaths, &profile);
        assert_eq!(
            result.estimate,
            estimate_job(&doc, &profile, &options).unwrap()
        );
        assert_eq!(result.cuts.len(), 1);
        assert_eq!(
            result.travels,
            vec![(Point::ORIGIN, Point::new(10.0, 10.0))]
        );
    }

    #[test]
    fn estimate_job_matches_the_generated_job() {
        // 80 mm at 1000 mm/min = 4.8 s; travel to (10, 10) at 6000 mm/min.
        let estimate = estimate_job(
            &doc_with_rect(10.0, 10.0),
            &MachineProfile::default(),
            &JobOptions::default(),
        )
        .unwrap();
        let travel = 200.0_f64.sqrt();
        assert!((estimate.cut_mm - 80.0).abs() < 1e-9);
        assert!((estimate.travel_mm - travel).abs() < 1e-9);
        assert!((estimate.seconds - (4.8 + travel / 100.0)).abs() < 1e-9);
    }
}
