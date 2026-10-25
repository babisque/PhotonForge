// clippy's allow-unwrap-in-tests does not cover helper functions in test crates.
#![allow(clippy::unwrap_used)]

//! M3: the sample cut on real hardware (`docs/hardware/machine-control-test.md`).
//! It must import cleanly, stay small and frame inside the default work area.

mod support;

use pf_core::{MachineProfile, Point, Rect};
use pf_gcode::{JobOptions, generate, generate_frame};
use pf_io::{SvgImportOptions, import_svg};
use support::assert_golden;

#[test]
fn m3_sample_imports_and_generates_a_small_job() {
    let svg = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/m3_sample.svg"),
    )
    .unwrap();
    let import = import_svg(&svg, &SvgImportOptions::default()).unwrap();
    assert!(import.warnings.is_empty(), "{:?}", import.warnings);

    let doc = &import.document;
    let names: Vec<&str> = doc.layers().iter().map(|l| l.name.as_str()).collect();
    // Inner score before the outline cut that frees the piece.
    assert_eq!(names, vec!["#0000FF", "#FF0000"]);
    assert_eq!(doc.shapes().len(), 2);
    let bounds = doc.bounds().unwrap();
    let page = Rect::new(Point::ORIGIN, Point::new(40.0, 40.0));
    assert!(page.contains_rect(&bounds), "{bounds:?} is off the page");

    let profile = MachineProfile::default();
    let gcode = generate(doc, &profile, &JobOptions::default()).unwrap();
    assert_golden("m3_sample.gcode", &gcode);

    let frame = generate_frame(doc, &profile, &JobOptions::default(), 0.0).unwrap();
    assert!(frame.contains("M5\nG0 X5 Y5\nG1 X35 Y5 F3000\n"), "{frame}");
    assert!(!frame.contains('S'));
}
