use pf_core::{Document, GeometryError, LayerId, LayerMode, Point, PowerMode, flatten_path};

/// One continuous laser-on polyline with the settings of its layer.
#[derive(Debug, Clone, PartialEq)]
pub struct Toolpath {
    pub layer_id: LayerId,
    /// mm. For a closed toolpath the last point repeats the first.
    pub points: Vec<Point>,
    pub closed: bool,
    pub speed: f64, // mm/min
    pub power: f64, // % of the machine maximum
    pub passes: u32,
    pub power_mode: PowerMode,
}

impl Toolpath {
    pub fn length(&self) -> f64 {
        self.points.windows(2).map(|w| w[0].distance(w[1])).sum()
    }
}

/// Toolpaths for every enabled line layer, grouped by layer in cut order and
/// in shape insertion order within a layer.
pub fn plan_toolpaths(doc: &Document, tolerance: f64) -> Result<Vec<Toolpath>, GeometryError> {
    let mut toolpaths = Vec::new();
    for layer in doc.layers() {
        if layer.mode != LayerMode::Line || !layer.output_enabled || layer.passes == 0 {
            continue;
        }
        for shape in doc.shapes_in_layer(layer.id) {
            for polyline in flatten_path(&shape.world_path(), tolerance)? {
                if polyline.points.len() < 2 {
                    continue;
                }
                toolpaths.push(Toolpath {
                    layer_id: layer.id,
                    points: polyline.points,
                    closed: polyline.closed,
                    speed: layer.speed_mm_min,
                    power: layer.power_percent,
                    passes: layer.passes,
                    power_mode: layer.power_mode,
                });
            }
        }
    }
    Ok(toolpaths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::{EPS, PolygonShape, RectShape, ShapeKind, Transform, approx_eq};

    fn rect(w: f64, h: f64) -> ShapeKind {
        ShapeKind::Rect(RectShape::new(w, h, 0.0).unwrap())
    }

    #[test]
    fn rectangle_becomes_one_closed_toolpath_with_layer_settings() {
        let mut doc = Document::new();
        let layer = doc.add_layer("Cut", [255, 0, 0]).unwrap();
        doc.update_layer(layer, |l| {
            l.speed_mm_min = 1200.0;
            l.power_percent = 50.0;
            l.passes = 2;
            l.power_mode = PowerMode::M3Constant;
        })
        .unwrap();
        doc.add_shape(rect(50.0, 30.0), Transform::translate(10.0, 10.0), layer)
            .unwrap();

        let toolpaths = plan_toolpaths(&doc, 0.01).unwrap();
        assert_eq!(toolpaths.len(), 1);
        let t = &toolpaths[0];
        assert_eq!(t.layer_id, layer);
        assert!(t.closed);
        assert_eq!(t.points.len(), 5);
        assert_eq!(t.points.first(), t.points.last());
        assert!(t.points[0].approx_eq(Point::new(10.0, 10.0), EPS));
        assert!(approx_eq(t.speed, 1200.0, EPS));
        assert!(approx_eq(t.power, 50.0, EPS));
        assert_eq!(t.passes, 2);
        assert_eq!(t.power_mode, PowerMode::M3Constant);
        assert!(approx_eq(t.length(), 160.0, 1e-9));
    }

    #[test]
    fn open_polygon_stays_open() {
        let mut doc = Document::new();
        let layer = doc.add_layer("Score", [0, 0, 0]).unwrap();
        let line =
            PolygonShape::new(vec![Point::new(0.0, 0.0), Point::new(10.0, 0.0)], false).unwrap();
        doc.add_shape(ShapeKind::Polygon(line), Transform::IDENTITY, layer)
            .unwrap();
        let toolpaths = plan_toolpaths(&doc, 0.01).unwrap();
        assert_eq!(toolpaths.len(), 1);
        assert!(!toolpaths[0].closed);
        assert_eq!(toolpaths[0].points.len(), 2);
    }

    #[test]
    fn skips_non_line_disabled_and_zero_pass_layers() {
        let mut doc = Document::new();
        let fill = doc.add_layer("Fill", [0, 0, 0]).unwrap();
        let off = doc.add_layer("Off", [0, 0, 0]).unwrap();
        let zero = doc.add_layer("Zero", [0, 0, 0]).unwrap();
        doc.update_layer(fill, |l| l.mode = LayerMode::Fill)
            .unwrap();
        doc.update_layer(off, |l| l.output_enabled = false).unwrap();
        doc.update_layer(zero, |l| l.passes = 0).unwrap();
        for layer in [fill, off, zero] {
            doc.add_shape(rect(5.0, 5.0), Transform::IDENTITY, layer)
                .unwrap();
        }
        assert!(plan_toolpaths(&doc, 0.01).unwrap().is_empty());
    }

    #[test]
    fn follows_layer_order_not_shape_order() {
        let mut doc = Document::new();
        let first = doc.add_layer("First", [0, 0, 0]).unwrap();
        let second = doc.add_layer("Second", [0, 0, 0]).unwrap();
        doc.add_shape(rect(5.0, 5.0), Transform::IDENTITY, second)
            .unwrap();
        doc.add_shape(rect(5.0, 5.0), Transform::IDENTITY, first)
            .unwrap();
        let layers: Vec<LayerId> = plan_toolpaths(&doc, 0.01)
            .unwrap()
            .iter()
            .map(|t| t.layer_id)
            .collect();
        assert_eq!(layers, vec![first, second]);
    }

    #[test]
    fn invalid_tolerance_is_an_error() {
        let mut doc = Document::new();
        let layer = doc.add_layer("Cut", [0, 0, 0]).unwrap();
        doc.add_shape(rect(5.0, 5.0), Transform::IDENTITY, layer)
            .unwrap();
        assert_eq!(
            plan_toolpaths(&doc, 0.0),
            Err(GeometryError::InvalidTolerance(0.0))
        );
    }
}
