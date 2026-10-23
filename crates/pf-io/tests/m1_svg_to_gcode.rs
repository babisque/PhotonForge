// clippy's allow-unwrap-in-tests does not cover helper functions in test crates.
#![allow(clippy::unwrap_used)]

//! M1: the first vertical slice, SVG file → document → G-code → disk.

mod support;

use pf_core::{MachineProfile, Point, Rect};
use pf_gcode::{JobOptions, generate};
use pf_io::{SvgImportOptions, WriteMode, export_gcode, import_svg};
use support::assert_golden;

#[test]
fn m1_sample_svg_to_gcode() {
    let svg = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/m1_sample.svg"),
    )
    .unwrap();
    let import = import_svg(&svg, &SvgImportOptions::default()).unwrap();
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);

    let doc = &import.document;
    let names: Vec<&str> = doc.layers().iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, vec!["#FF0000", "#0000FF"]);
    assert_eq!(doc.shapes().len(), 4);
    let bounds = doc.bounds().unwrap();
    let page = Rect::new(Point::ORIGIN, Point::new(100.0, 50.0));
    assert!(page.contains_rect(&bounds), "{bounds:?} is off the page");

    let gcode = generate(doc, &MachineProfile::default(), &JobOptions::default()).unwrap();
    assert_golden("m1_sample.gcode", &gcode);

    let dir = std::env::temp_dir().join(format!("pf-io-m1-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("m1_sample.gcode");
    export_gcode(&out, &gcode, WriteMode::CreateNew).unwrap();
    assert_eq!(std::fs::read_to_string(&out).unwrap(), gcode);
    std::fs::remove_dir_all(dir).unwrap();
}
