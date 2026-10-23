use std::f64::consts::FRAC_PI_4;

use pf_core::{
    Document, EllipseShape, MachineProfile, Path, Point, RectShape, ShapeKind, Transform,
    flatten_path, is_inside_work_area,
};

#[test]
fn build_transform_flatten_and_check_bounds() {
    let profile = MachineProfile::default();
    let mut doc = Document::new();
    let cut = doc.add_layer("Cut", [255, 0, 0]).unwrap();
    let engrave = doc.add_layer("Engrave", [0, 0, 255]).unwrap();

    let rect = ShapeKind::Rect(RectShape::new(40.0, 20.0, 3.0).unwrap());
    let ellipse = ShapeKind::Ellipse(EllipseShape::new(15.0, 8.0).unwrap());
    let wave = ShapeKind::Path(
        Path::builder()
            .move_to(Point::new(0.0, 0.0))
            .unwrap()
            .cubic_to(
                Point::new(10.0, 20.0),
                Point::new(30.0, -20.0),
                Point::new(40.0, 0.0),
            )
            .unwrap()
            .build()
            .unwrap(),
    );
    doc.add_shape(rect, Transform::translate(10.0, 10.0), cut)
        .unwrap();
    doc.add_shape(
        ellipse,
        Transform::rotate(FRAC_PI_4).then(Transform::translate(150.0, 100.0)),
        engrave,
    )
    .unwrap();
    doc.add_shape(wave, Transform::translate(200.0, 50.0), cut)
        .unwrap();

    assert_eq!(doc.shapes_in_layer(cut).count(), 2);
    assert_eq!(doc.shapes_in_layer(engrave).count(), 1);

    let bounds = doc.bounds().unwrap();
    assert!(is_inside_work_area(&profile, &bounds));

    let tolerance = 0.01;
    for shape in doc.shapes() {
        for polyline in flatten_path(&shape.world_path(), tolerance).unwrap() {
            assert!(polyline.points.len() >= 2);
            for point in polyline.points {
                assert!(bounds.contains_point(point));
                assert!(profile.work_area().contains_point(point));
            }
        }
    }
}

#[test]
fn shape_moved_off_the_bed_is_detected() {
    let profile = MachineProfile::default();
    let mut doc = Document::new();
    let layer = doc.add_layer("Cut", [0, 0, 0]).unwrap();
    let rect = ShapeKind::Rect(RectShape::new(50.0, 50.0, 0.0).unwrap());
    let id = doc
        .add_shape(rect, Transform::translate(10.0, 10.0), layer)
        .unwrap();
    assert!(is_inside_work_area(&profile, &doc.bounds().unwrap()));

    doc.set_transform(id, Transform::translate(270.0, 10.0))
        .unwrap();
    assert!(!is_inside_work_area(&profile, &doc.bounds().unwrap()));
}
