// clippy's allow-unwrap-in-tests does not cover helper functions in test crates.
#![allow(clippy::unwrap_used)]

mod support;

use pf_core::{
    Document, LayerId, MachineProfile, Point, PolygonShape, PowerMode, RectShape, ShapeKind,
    Transform,
};
use pf_gcode::{JobOptions, JobOrigin, generate};
use support::assert_golden;

fn layer(doc: &mut Document, name: &str, speed: f64, power: f64, passes: u32) -> LayerId {
    let id = doc.add_layer(name, [0, 0, 0]).unwrap();
    doc.update_layer(id, |l| {
        l.speed_mm_min = speed;
        l.power_percent = power;
        l.passes = passes;
    })
    .unwrap();
    id
}

fn rect(doc: &mut Document, layer: LayerId, x: f64, y: f64, w: f64, h: f64) {
    let shape = ShapeKind::Rect(RectShape::new(w, h, 0.0).unwrap());
    doc.add_shape(shape, Transform::translate(x, y), layer)
        .unwrap();
}

fn open_line(doc: &mut Document, layer: LayerId, from: Point, to: Point) {
    let shape = ShapeKind::Polygon(PolygonShape::new(vec![from, to], false).unwrap());
    doc.add_shape(shape, Transform::IDENTITY, layer).unwrap();
}

fn generate_with(doc: &Document, options: &JobOptions) -> String {
    generate(doc, &MachineProfile::default(), options).unwrap()
}

#[test]
fn single_rectangle() {
    let mut doc = Document::new();
    let cut = layer(&mut doc, "Cut", 1200.0, 50.0, 1);
    rect(&mut doc, cut, 10.0, 10.0, 50.0, 30.0);
    assert_golden(
        "single_rectangle.gcode",
        &generate_with(&doc, &JobOptions::default()),
    );
}

#[test]
fn multi_pass() {
    let mut doc = Document::new();
    let cut = layer(&mut doc, "Cut", 600.0, 80.0, 3);
    rect(&mut doc, cut, 5.0, 5.0, 20.0, 20.0);
    assert_golden(
        "multi_pass.gcode",
        &generate_with(&doc, &JobOptions::default()),
    );
}

#[test]
fn multi_layer() {
    let mut doc = Document::new();
    let score = layer(&mut doc, "Score", 3000.0, 15.0, 1);
    let cut = layer(&mut doc, "Cut", 400.0, 90.0, 1);
    // Added to the cut layer first: the layer order must still win.
    rect(&mut doc, cut, 100.0, 100.0, 30.0, 30.0);
    open_line(
        &mut doc,
        score,
        Point::new(100.0, 140.0),
        Point::new(130.0, 140.0),
    );
    assert_golden(
        "multi_layer.gcode",
        &generate_with(&doc, &JobOptions::default()),
    );
}

#[test]
fn m3_layer() {
    let mut doc = Document::new();
    let engrave = layer(&mut doc, "Engrave", 2000.0, 20.0, 1);
    let cut = layer(&mut doc, "Cut", 500.0, 70.0, 1);
    doc.update_layer(engrave, |l| l.power_mode = PowerMode::M3Constant)
        .unwrap();
    rect(&mut doc, engrave, 10.0, 10.0, 10.0, 10.0);
    rect(&mut doc, cut, 30.0, 10.0, 10.0, 10.0);
    assert_golden(
        "m3_layer.gcode",
        &generate_with(&doc, &JobOptions::default()),
    );
}

#[test]
fn nested_shapes() {
    let mut doc = Document::new();
    let cut = layer(&mut doc, "Cut", 800.0, 60.0, 1);
    // The outer part is nearest to the start but must be cut after its holes.
    rect(&mut doc, cut, 10.0, 10.0, 100.0, 100.0);
    rect(&mut doc, cut, 200.0, 150.0, 10.0, 10.0);
    rect(&mut doc, cut, 80.0, 80.0, 20.0, 20.0);
    rect(&mut doc, cut, 20.0, 20.0, 20.0, 20.0);
    // Its end is nearer to the previous shape, so it is cut in reverse.
    open_line(
        &mut doc,
        cut,
        Point::new(290.0, 190.0),
        Point::new(250.0, 190.0),
    );
    assert_golden(
        "nested_shapes.gcode",
        &generate_with(&doc, &JobOptions::default()),
    );
}

#[test]
fn user_origin() {
    let mut doc = Document::new();
    let cut = layer(&mut doc, "Cut", 1000.0, 40.0, 1);
    rect(&mut doc, cut, 110.0, 110.0, 40.0, 40.0);
    let options = JobOptions {
        origin: JobOrigin::UserOrigin(Point::new(100.0, 100.0)),
        ..JobOptions::default()
    };
    assert_golden("user_origin.gcode", &generate_with(&doc, &options));
}

#[test]
fn custom_header_footer() {
    let mut doc = Document::new();
    let cut = layer(&mut doc, "Cut", 1000.0, 40.0, 1);
    rect(&mut doc, cut, 10.0, 10.0, 10.0, 10.0);
    let options = JobOptions {
        header: Some(String::from("M8 ; air assist on\n")),
        footer: Some(String::from("M9 ; air assist off\nG0 X0 Y0\n")),
        ..JobOptions::default()
    };
    assert_golden("custom_header_footer.gcode", &generate_with(&doc, &options));
}
