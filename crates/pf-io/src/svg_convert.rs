//! usvg tree → plain `pf-core` data. The only module that uses usvg types.

use pf_core::{GeometryError, LayerMode, Path, Point, Transform};
use usvg::roxmltree;
use usvg::tiny_skia_path::PathSegment;

use crate::error::{ImportWarning, IoError};

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const BLACK: [u8; 3] = [0, 0, 0];

pub(crate) struct ConvertedSvg {
    pub height_px: f64,
    /// Root `width` and `height` both use an absolute unit (mm, cm, in, pt, pc).
    pub has_physical_size: bool,
    pub paths: Vec<ConvertedPath>,
    pub warnings: Vec<ImportWarning>,
}

pub(crate) struct ConvertedPath {
    /// Absolute SVG user space: px, Y-down.
    pub path: Path,
    pub color: [u8; 3],
    pub mode: LayerMode,
}

pub(crate) fn convert(bytes: &[u8], dpi: f32) -> Result<ConvertedSvg, IoError> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        IoError::Parse(String::from(
            "the file is not UTF-8 text (compressed .svgz is not supported)",
        ))
    })?;
    let xml_options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    let xml = roxmltree::Document::parse_with_options(text, xml_options)
        .map_err(|e| IoError::Parse(e.to_string()))?;

    let options = usvg::Options {
        dpi,
        image_href_resolver: no_image_loading(),
        ..usvg::Options::default()
    };
    let tree =
        usvg::Tree::from_xmltree(&xml, &options).map_err(|e| IoError::Parse(e.to_string()))?;

    // usvg drops <text> and <image> without a trace in this configuration,
    // so they are found in the XML instead.
    let mut warnings = skipped_elements(&xml);
    let mut paths = Vec::new();
    walk(tree.root(), &mut paths, &mut warnings);

    Ok(ConvertedSvg {
        height_px: f64::from(tree.size().height()),
        has_physical_size: has_physical_size(xml.root_element()),
        paths,
        warnings,
    })
}

/// Keeps the import pure: linked or embedded images are never loaded.
fn no_image_loading() -> usvg::ImageHrefResolver<'static> {
    usvg::ImageHrefResolver {
        resolve_data: Box::new(|_, _, _| None),
        resolve_string: Box::new(|_, _| None),
    }
}

fn skipped_elements(xml: &roxmltree::Document) -> Vec<ImportWarning> {
    xml.descendants()
        .filter(|n| n.is_element() && n.tag_name().namespace() == Some(SVG_NS))
        .filter_map(|n| {
            let id = element_id(n.attribute("id").unwrap_or(""));
            match n.tag_name().name() {
                "text" => Some(ImportWarning::SkippedText { id }),
                "image" => Some(ImportWarning::SkippedImage { id }),
                _ => None,
            }
        })
        .collect()
}

fn has_physical_size(root: roxmltree::Node) -> bool {
    ["width", "height"]
        .iter()
        .all(|name| root.attribute(*name).is_some_and(is_physical_length))
}

fn is_physical_length(value: &str) -> bool {
    ["mm", "cm", "in", "pt", "pc"].iter().any(|unit| {
        value
            .trim()
            .strip_suffix(unit)
            .is_some_and(|number| number.trim().parse::<f64>().is_ok())
    })
}

fn walk(group: &usvg::Group, paths: &mut Vec<ConvertedPath>, warnings: &mut Vec<ImportWarning>) {
    for node in group.children() {
        match node {
            usvg::Node::Group(child) => {
                if child.clip_path().is_some() {
                    warnings.push(ImportWarning::IgnoredClipPath {
                        id: element_id(child.id()),
                    });
                }
                if child.mask().is_some() {
                    warnings.push(ImportWarning::IgnoredMask {
                        id: element_id(child.id()),
                    });
                }
                walk(child, paths, warnings);
            }
            usvg::Node::Path(path) => convert_path(path, paths, warnings),
            // Reported by `skipped_elements`.
            usvg::Node::Image(_) | usvg::Node::Text(_) => {}
        }
    }
}

fn convert_path(
    path: &usvg::Path,
    paths: &mut Vec<ConvertedPath>,
    warnings: &mut Vec<ImportWarning>,
) {
    let id = element_id(path.id());
    // usvg marks paths with neither fill nor stroke as not visible.
    let (paint, mode) = match (path.stroke(), path.fill()) {
        _ if !path.is_visible() => {
            warnings.push(ImportWarning::HiddenShape { id });
            return;
        }
        (Some(stroke), _) => (stroke.paint(), LayerMode::Line),
        (None, Some(fill)) => {
            warnings.push(ImportWarning::FillOnlyShape { id: id.clone() });
            (fill.paint(), LayerMode::Fill)
        }
        (None, None) => {
            warnings.push(ImportWarning::HiddenShape { id });
            return;
        }
    };
    let color = match paint {
        usvg::Paint::Color(c) => [c.red, c.green, c.blue],
        _ => {
            warnings.push(ImportWarning::UnsupportedPaint { id: id.clone() });
            BLACK
        }
    };
    match to_core_path(path.data(), &to_core_transform(path.abs_transform())) {
        Ok(core) if core.is_empty() => warnings.push(ImportWarning::InvalidGeometry {
            id,
            reason: String::from("the path has no drawable segments"),
        }),
        Ok(core) => paths.push(ConvertedPath {
            path: core,
            color,
            mode,
        }),
        Err(e) => warnings.push(ImportWarning::InvalidGeometry {
            id,
            reason: e.to_string(),
        }),
    }
}

fn to_core_transform(t: usvg::Transform) -> Transform {
    Transform {
        a: f64::from(t.sx),
        b: f64::from(t.ky),
        c: f64::from(t.kx),
        d: f64::from(t.sy),
        e: f64::from(t.tx),
        f: f64::from(t.ty),
    }
}

/// Maps every point through `t`; exact for lines and Béziers.
fn to_core_path(data: &usvg::tiny_skia_path::Path, t: &Transform) -> Result<Path, GeometryError> {
    let map =
        |p: usvg::tiny_skia_path::Point| t.apply_point(Point::new(f64::from(p.x), f64::from(p.y)));
    let mut builder = Path::builder();
    let mut drawing = false; // the open sub-path has at least one segment
    for segment in data.segments() {
        builder = match segment {
            PathSegment::MoveTo(p) => builder.move_to(map(p))?,
            PathSegment::LineTo(p) => builder.line_to(map(p))?,
            PathSegment::QuadTo(c, p) => builder.quad_to(map(c), map(p))?,
            PathSegment::CubicTo(c1, c2, p) => builder.cubic_to(map(c1), map(c2), map(p))?,
            // A lone `M … Z` draws nothing; the next `move_to` discards it.
            PathSegment::Close if !drawing => continue,
            PathSegment::Close => builder.close()?,
        };
        drawing = !matches!(segment, PathSegment::MoveTo(_) | PathSegment::Close);
    }
    builder.build()
}

fn element_id(id: &str) -> Option<String> {
    if id.is_empty() {
        None
    } else {
        Some(id.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_lengths() {
        for value in ["100mm", "10.5cm", " 4in ", "72pt", "6pc", "1e2mm"] {
            assert!(is_physical_length(value), "{value}");
        }
        for value in ["100", "100px", "50%", "mm", "", "10em", "abcmm"] {
            assert!(!is_physical_length(value), "{value}");
        }
    }

    #[test]
    fn non_utf8_input_is_a_parse_error() {
        let gzip_magic = [0x1f, 0x8b, 0x08, 0x00, 0xff];
        assert!(matches!(convert(&gzip_magic, 96.0), Err(IoError::Parse(_))));
    }

    #[test]
    fn lone_move_close_is_reported_not_kept() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 10 10">
            <path id="dot" d="M 5 5 Z" stroke="#000"/>
        </svg>"##;
        let converted = convert(svg, 96.0).unwrap();
        assert!(converted.paths.is_empty());
        assert_eq!(
            converted.warnings,
            vec![ImportWarning::InvalidGeometry {
                id: Some(String::from("dot")),
                reason: String::from("the path has no drawable segments"),
            }]
        );
    }
}
