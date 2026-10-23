// clippy's allow-unwrap-in-tests does not cover helper functions in test crates.
#![allow(clippy::unwrap_used)]

//! M2: the GUI walkthrough without the GUI. Import an SVG, arrange shapes
//! with the canvas handle math, edit layers, undo and redo everything, then
//! generate G-code. Every edit goes through `History`, as in the app.

mod support;

use pf_core::{
    Command, Document, Handle, History, MachineProfile, Point, PowerMode, ShapeId, Transform,
    move_delta, rotate_delta, scale_delta,
};
use pf_gcode::{JobOptions, estimate_job, generate, prepare_job, preview};
use pf_io::{SvgImportOptions, import_svg};
use support::assert_golden;

/// Applies `delta` on top of one shape's transform, as the canvas does.
fn transform(doc: &Document, id: ShapeId, delta: Transform) -> Command {
    let current = doc.shape(id).unwrap().transform;
    Command::SetTransforms(vec![(id, current.then(delta))])
}

#[test]
fn m2_import_arrange_edit_undo_redo_and_generate() {
    let svg = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/m1_sample.svg"),
    )
    .unwrap();
    let import = import_svg(&svg, &SvgImportOptions::default()).unwrap();

    let mut doc = Document::new();
    let mut history = History::new();
    history
        .apply(&mut doc, Command::ImportDocument(import.document))
        .unwrap();
    let imported = doc.clone();
    let ids: Vec<ShapeId> = doc.shapes().iter().map(|s| s.id).collect();
    let [square, circle, triangle, _arch] = ids[..] else {
        panic!("expected 4 shapes, got {}", ids.len());
    };
    let (red, blue) = (doc.layers()[0].id, doc.layers()[1].id);

    // Arrange: move the square, grow the circle from its NE handle with the
    // aspect ratio kept, and turn the triangle a quarter turn.
    let moved = move_delta(Point::new(25.0, 25.0), Point::new(35.0, 30.0));
    let circle_bounds = doc.shape(circle).unwrap().bounds().unwrap();
    let grown = scale_delta(
        circle_bounds,
        Handle::NE,
        circle_bounds.max,
        Point::new(circle_bounds.max.x + 10.0, circle_bounds.max.y + 4.0),
        true,
    );
    let center = doc.shape(triangle).unwrap().bounds().unwrap().center();
    let turned = rotate_delta(
        center,
        Point::new(center.x + 1.0, center.y),
        Point::new(center.x, center.y + 1.0),
        None,
    );

    let mut commands = vec![
        transform(&doc, square, moved),
        transform(&doc, circle, grown),
        transform(&doc, triangle, turned),
    ];
    commands.extend([
        Command::SetLayerSpeed {
            id: red,
            speed: 2000.0,
        },
        Command::SetLayerPower {
            id: red,
            power: 35.0,
        },
        Command::SetLayerPowerMode {
            id: red,
            mode: PowerMode::M3Constant,
        },
        Command::SetLayerPasses {
            id: blue,
            passes: 2,
        },
        Command::MoveLayer { id: blue, to: 0 },
    ]);
    let steps = commands.len();
    for command in commands {
        history.apply(&mut doc, command).unwrap();
    }
    let edited = doc.clone();

    // Undo every step back to the import, then redo all of it.
    for _ in 0..steps {
        assert!(history.undo(&mut doc).unwrap().is_some());
    }
    assert_eq!(doc.layers(), imported.layers());
    assert_eq!(doc.shapes(), imported.shapes());
    for _ in 0..steps {
        assert!(history.redo(&mut doc).unwrap().is_some());
    }
    assert_eq!(doc, edited);

    let profile = MachineProfile::default();
    let options = JobOptions::default();
    let gcode = generate(&doc, &profile, &options).unwrap();
    assert_golden("m2_edit_flow.gcode", &gcode);

    // The preview the app draws comes from the same planning.
    let planned = preview(&prepare_job(&doc, &profile, &options).unwrap(), &profile);
    assert_eq!(
        planned.estimate,
        estimate_job(&doc, &profile, &options).unwrap()
    );
    assert_eq!(planned.cuts.len(), 4);
}
