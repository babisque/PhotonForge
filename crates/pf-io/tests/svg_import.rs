// clippy's allow-unwrap-in-tests does not cover helper functions in test crates.
#![allow(clippy::unwrap_used)]

use pf_core::{Document, LayerMode, Point, Rect, Segment};
use pf_io::{ImportWarning, SvgImport, SvgImportOptions, import_svg};

/// The spec's size accuracy target, in mm.
const SIZE_TOL: f64 = 0.01;

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(path).unwrap()
}

fn import_at(name: &str, dpi: f64) -> SvgImport {
    import_svg(&fixture(name), &SvgImportOptions { dpi }).unwrap()
}

fn import(name: &str) -> SvgImport {
    import_svg(&fixture(name), &SvgImportOptions::default()).unwrap()
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
    Rect::new(Point::new(x0, y0), Point::new(x1, y1))
}

fn shape_bounds(doc: &Document) -> Vec<Rect> {
    doc.shapes().iter().map(|s| s.bounds().unwrap()).collect()
}

fn assert_bounds(actual: Rect, expected: Rect, tol: f64) {
    assert!(
        actual.approx_eq(&expected, tol),
        "bounds {actual:?}, expected {expected:?}"
    );
}

fn id(s: &str) -> Option<String> {
    Some(String::from(s))
}

// --- 3.1 Basic import ---

#[test]
fn basic_shapes_import_at_their_svg_positions() {
    let out = import("basic_shapes.svg");
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let expected = [
        rect(10.0, 70.0, 40.0, 90.0), // rect
        rect(60.0, 70.0, 80.0, 90.0), // circle
        rect(10.0, 35.0, 40.0, 45.0), // ellipse
        rect(50.0, 40.0, 90.0, 50.0), // line
        rect(10.0, 10.0, 30.0, 20.0), // polyline
        rect(60.0, 5.0, 90.0, 20.0),  // polygon
    ];
    let actual = shape_bounds(&out.document);
    assert_eq!(actual.len(), expected.len());
    for (a, e) in actual.into_iter().zip(expected) {
        assert_bounds(a, e, 1e-4);
    }
    assert_eq!(out.document.layers().len(), 1);
}

#[test]
fn open_and_closed_shapes_keep_their_closure() {
    let out = import("basic_shapes.svg");
    let closed: Vec<bool> = out
        .document
        .shapes()
        .iter()
        .map(|s| s.world_path().subpaths()[0].is_closed())
        .collect();
    assert_eq!(closed, vec![true, true, true, false, false, true]);
}

#[test]
fn path_commands_are_all_supported() {
    let out = import("path_commands.svg");
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let bounds = shape_bounds(&out.document);
    assert_eq!(bounds.len(), 5);
    assert_bounds(bounds[0], rect(10.0, 70.0, 30.0, 90.0), 1e-4); // relative square
    assert_bounds(bounds[3], rect(70.0, 50.0, 90.0, 60.0), 1e-3); // arc
    assert_bounds(bounds[4], rect(10.0, 15.0, 50.0, 25.0), 1e-4); // quad + T
}

#[test]
fn smooth_shorthand_matches_the_spelled_out_curve() {
    let out = import("path_commands.svg");
    let smooth = out.document.shapes()[1].world_path();
    let explicit = out.document.shapes()[2].world_path();
    let a = smooth.subpaths()[0].segments();
    let b = explicit.subpaths()[0].segments();
    assert_eq!(a.len(), b.len());
    for (sa, sb) in a.iter().zip(b) {
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            assert!(sa.point_at(t).approx_eq(sb.point_at(t), 1e-4));
        }
        assert!(matches!(sa, Segment::CubicBezier { .. }));
    }
}

#[test]
fn nested_transforms_and_use_are_applied() {
    let out = import("nested_transforms.svg");
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let bounds = shape_bounds(&out.document);
    assert_eq!(bounds.len(), 2, "the <defs> original is not drawn");
    assert_bounds(bounds[0], rect(10.0, 80.0, 30.0, 90.0), 1e-4);
    assert_bounds(bounds[1], rect(50.0, 55.0, 60.0, 60.0), 1e-4);
}

#[test]
fn import_is_deterministic() {
    assert_eq!(
        import("nested_transforms.svg"),
        import("nested_transforms.svg")
    );
}

// --- 3.2 Units and DPI ---

#[test]
fn inkscape_100x50mm_imports_at_100x50mm_at_any_dpi() {
    for dpi in [96.0, 72.0, 300.0] {
        let out = import_at("inkscape_100x50mm.svg", dpi);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_bounds(
            out.document.bounds().unwrap(),
            rect(0.0, 0.0, 100.0, 50.0),
            SIZE_TOL,
        );
    }
}

#[test]
fn illustrator_file_needs_72_dpi() {
    let out = import_at("illustrator_72dpi.svg", 72.0);
    assert_bounds(
        out.document.bounds().unwrap(),
        rect(0.0, 0.0, 100.0, 50.0),
        SIZE_TOL,
    );
    assert_eq!(
        out.warnings,
        vec![ImportWarning::NoPhysicalSize { assumed_dpi: 72.0 }]
    );

    let at_96 = import("illustrator_72dpi.svg");
    assert_bounds(
        at_96.document.bounds().unwrap(),
        rect(0.0, 0.0, 75.0, 37.5),
        SIZE_TOL,
    );
}

#[test]
fn viewbox_scales_user_units_to_the_page() {
    let out = import("viewbox_scaling.svg");
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let bounds = shape_bounds(&out.document);
    assert_bounds(bounds[0], rect(0.0, 0.0, 50.0, 25.0), SIZE_TOL);
    assert_bounds(bounds[1], rect(25.0, 0.0, 50.0, 12.5), SIZE_TOL);
}

#[test]
fn viewbox_origin_maps_to_the_page_top_left() {
    let out = import("viewbox_offset.svg");
    assert_bounds(
        out.document.bounds().unwrap(),
        rect(0.0, 40.0, 10.0, 50.0),
        SIZE_TOL,
    );
}

#[test]
fn inch_root_size() {
    let out = import("inches_root.svg");
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_bounds(
        out.document.bounds().unwrap(),
        rect(0.0, 0.0, 101.6, 50.8),
        SIZE_TOL,
    );
}

#[test]
fn px_root_size_depends_on_dpi_and_warns() {
    let out = import("px_root.svg");
    assert_bounds(
        out.document.bounds().unwrap(),
        rect(0.0, 0.0, 100.0, 50.0),
        SIZE_TOL,
    );
    assert_eq!(
        out.warnings,
        vec![ImportWarning::NoPhysicalSize { assumed_dpi: 96.0 }]
    );

    let at_72 = import_at("px_root.svg", 72.0);
    assert_bounds(
        at_72.document.bounds().unwrap(),
        rect(0.0, 0.0, 100.0 * 96.0 / 72.0, 50.0 * 96.0 / 72.0),
        SIZE_TOL,
    );
}

#[test]
fn missing_size_falls_back_to_the_viewbox_and_warns() {
    let out = import("no_size.svg");
    let mm = 25.4 / 96.0;
    assert_bounds(
        out.document.bounds().unwrap(),
        rect(0.0, 0.0, 100.0 * mm, 50.0 * mm),
        SIZE_TOL,
    );
    assert_eq!(
        out.warnings,
        vec![ImportWarning::NoPhysicalSize { assumed_dpi: 96.0 }]
    );
}

// --- 3.3 Color → layer mapping ---

#[test]
fn distinct_stroke_colors_map_to_distinct_layers() {
    let out = import("multicolor.svg");
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let doc = &out.document;

    let layers: Vec<(&str, [u8; 3])> = doc
        .layers()
        .iter()
        .map(|l| (l.name.as_str(), l.color))
        .collect();
    assert_eq!(
        layers,
        vec![
            ("#FF0000", [255, 0, 0]),
            ("#0000FF", [0, 0, 255]),
            ("#00A000", [0, 160, 0]),
            ("#FF8800", [255, 136, 0]),
            ("#FE0000", [254, 0, 0]),
        ]
    );
    assert!(doc.layers().iter().all(|l| l.mode == LayerMode::Line));

    let layer_of_each_shape: Vec<usize> = doc
        .shapes()
        .iter()
        .map(|s| doc.layer_index(s.layer_id).unwrap())
        .collect();
    // red1, blue1, red2, green1, red3, orange1, nearred1
    assert_eq!(layer_of_each_shape, vec![0, 1, 0, 2, 0, 3, 4]);
}

// --- 3.4 Warnings ---

#[test]
fn unsupported_content_is_reported_never_silently_dropped() {
    let out = import("unsupported.svg");
    assert_eq!(
        out.warnings,
        vec![
            ImportWarning::SkippedText { id: id("label") },
            ImportWarning::SkippedImage { id: id("photo") },
            ImportWarning::FillOnlyShape { id: id("filled") },
            ImportWarning::UnsupportedPaint { id: id("gradient") },
            ImportWarning::IgnoredClipPath { id: id("clipped") },
            ImportWarning::IgnoredMask { id: id("masked") },
            ImportWarning::HiddenShape { id: id("hidden") },
        ]
    );

    let doc = &out.document;
    // filled, gradient, inside_clip, inside_mask, ok
    assert_eq!(doc.shapes().len(), 5);
    let layers: Vec<(&str, LayerMode)> = doc
        .layers()
        .iter()
        .map(|l| (l.name.as_str(), l.mode))
        .collect();
    assert_eq!(
        layers,
        vec![
            ("#0000FF fill", LayerMode::Fill),
            ("#000000", LayerMode::Line)
        ]
    );
}

#[test]
fn perf_fixture_has_ten_thousand_segments_inside_the_default_work_area() {
    let svg = import("perf_10k.svg");
    let segments: usize = svg
        .document
        .shapes()
        .iter()
        .flat_map(|s| s.to_path().subpaths().to_vec())
        .map(|sub| sub.segments().len())
        .sum();
    assert_eq!(svg.document.shapes().len(), 100);
    assert_eq!(segments, 10_000);
    assert!(svg.warnings.is_empty());
    let bounds = svg.document.bounds().unwrap();
    assert!(rect(0.0, 0.0, 300.0, 200.0).contains_rect(&bounds));
}
