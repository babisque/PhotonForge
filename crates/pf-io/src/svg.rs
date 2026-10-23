use pf_core::units::MM_PER_INCH;
use pf_core::{Document, ShapeKind, Transform, UnitsError};

use crate::color_layers::ColorLayers;
use crate::error::{ImportWarning, IoError};
use crate::svg_convert;

/// Inkscape and the CSS reference pixel. Illustrator files need 72.
pub const DEFAULT_SVG_DPI: f64 = 96.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SvgImportOptions {
    /// Pixels per inch for user units. Only matters when the root size has
    /// no physical unit.
    pub dpi: f64,
}

impl Default for SvgImportOptions {
    fn default() -> Self {
        SvgImportOptions {
            dpi: DEFAULT_SVG_DPI,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SvgImport {
    pub document: Document,
    pub warnings: Vec<ImportWarning>,
}

/// Imports an SVG into a new document. The page's bottom-left corner lands
/// on document (0, 0); Y points up. Shapes go on one layer per stroke color.
pub fn import_svg(bytes: &[u8], opts: &SvgImportOptions) -> Result<SvgImport, IoError> {
    let dpi = usvg_dpi(opts.dpi)?;
    let svg = svg_convert::convert(bytes, dpi)?;

    // usvg turned physical units into px with this same f32 DPI, so they
    // cancel out exactly here.
    let mm_per_px = MM_PER_INCH / f64::from(dpi);
    let to_document = Transform::scale(mm_per_px, -mm_per_px)
        .then(Transform::translate(0.0, svg.height_px * mm_per_px));

    let mut warnings = Vec::with_capacity(svg.warnings.len() + 1);
    if !svg.has_physical_size {
        warnings.push(ImportWarning::NoPhysicalSize {
            assumed_dpi: opts.dpi,
        });
    }
    warnings.extend(svg.warnings);

    let mut document = Document::new();
    let mut layers = ColorLayers::default();
    for item in svg.paths {
        let layer = layers.layer_for(&mut document, item.color, item.mode)?;
        let path = item.path.transform(&to_document);
        document.add_shape(ShapeKind::Path(path), Transform::identity(), layer)?;
    }
    Ok(SvgImport { document, warnings })
}

fn usvg_dpi(dpi: f64) -> Result<f32, UnitsError> {
    let narrow = dpi as f32;
    if dpi.is_finite() && dpi > 0.0 && narrow.is_finite() && narrow > 0.0 {
        Ok(narrow)
    } else {
        Err(UnitsError::InvalidDpi(dpi))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::{LayerMode, Point, Rect};

    fn import(svg: &str) -> SvgImport {
        import_svg(svg.as_bytes(), &SvgImportOptions::default()).unwrap()
    }

    #[test]
    fn y_axis_is_flipped_to_the_page_bottom() {
        let out = import(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="50mm" viewBox="0 0 100 50">
                <path d="M 10 0 L 20 10" stroke="#000"/>
            </svg>"##,
        );
        let path = out.document.shapes()[0].world_path();
        let segment = path.subpaths()[0].segments()[0];
        assert!(segment.start().approx_eq(Point::new(10.0, 50.0), 1e-4));
        assert!(segment.end().approx_eq(Point::new(20.0, 40.0), 1e-4));
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn shapes_are_imported_with_identity_transform() {
        let out = import(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 10 10">
                <rect x="1" y="1" width="2" height="3" stroke="#000" fill="none"/>
            </svg>"##,
        );
        let shape = &out.document.shapes()[0];
        assert!(shape.transform.is_identity());
        let expected = Rect::new(Point::new(1.0, 6.0), Point::new(3.0, 9.0));
        assert!(shape.bounds().unwrap().approx_eq(&expected, 1e-4));
        assert_eq!(out.document.layers()[0].mode, LayerMode::Line);
    }

    #[test]
    fn invalid_dpi_is_rejected() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm"/>"##;
        for dpi in [0.0, -96.0, f64::NAN, f64::INFINITY, 1e300, 1e-300] {
            let result = import_svg(svg, &SvgImportOptions { dpi });
            assert!(
                matches!(result, Err(IoError::InvalidDpi(_))),
                "dpi {dpi} gave {result:?}"
            );
        }
    }

    #[test]
    fn malformed_xml_is_a_parse_error_with_position() {
        let result = import_svg(b"<svg><g></svg>", &SvgImportOptions::default());
        match result {
            Err(IoError::Parse(message)) => assert!(message.contains(':'), "{message}"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn empty_svg_gives_an_empty_document() {
        let out =
            import(r##"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm"/>"##);
        assert!(out.document.shapes().is_empty());
        assert!(out.document.layers().is_empty());
    }
}
