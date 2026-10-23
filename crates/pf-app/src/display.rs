//! Results kept between frames: flattened shape outlines, so the canvas only
//! maps points to the screen, and the job preview, so toolpaths are planned
//! once per change instead of once per frame.

use pf_core::{Document, MachineProfile, Polyline, Rect, ShapeId, flatten_path};
use pf_gcode::{DEFAULT_FLATTEN_TOLERANCE, GcodeError, JobOptions, JobPreview};

/// Same tolerance as the G-code output, so the canvas shows what gets cut.
const DISPLAY_TOLERANCE_MM: f64 = DEFAULT_FLATTEN_TOLERANCE;

#[derive(Debug, Clone, PartialEq)]
pub struct CachedShape {
    pub id: ShapeId,
    pub color: [u8; 3],
    pub polylines: Vec<Polyline>, // mm, world coordinates
    pub bounds: Rect,
}

#[derive(Debug, Default)]
pub struct DisplayCache {
    revision: Option<u64>,
    shapes: Vec<CachedShape>,
}

impl DisplayCache {
    /// Rebuilds the cache when `revision` differs from the one it was built for.
    pub fn sync(&mut self, doc: &Document, revision: u64) {
        if self.revision == Some(revision) {
            return;
        }
        self.shapes = build(doc);
        self.revision = Some(revision);
    }

    pub fn shapes(&self) -> &[CachedShape] {
        &self.shapes
    }
}

/// The job preview for one document revision, machine profile and job
/// options. It runs the same planning as G-code generation.
#[derive(Debug, Default)]
pub struct PreviewCache {
    key: Option<(u64, MachineProfile, JobOptions)>,
    result: Option<Result<JobPreview, GcodeError>>,
}

impl PreviewCache {
    pub fn sync(
        &mut self,
        doc: &Document,
        revision: u64,
        profile: &MachineProfile,
        options: &JobOptions,
    ) {
        let key = (revision, profile.clone(), options.clone());
        if self.key.as_ref() == Some(&key) {
            return;
        }
        let planned = pf_gcode::prepare_job(doc, profile, options);
        self.result = Some(planned.map(|toolpaths| pf_gcode::preview(&toolpaths, profile)));
        self.key = Some(key);
    }

    /// `None` until the first `sync`.
    pub fn result(&self) -> Option<&Result<JobPreview, GcodeError>> {
        self.result.as_ref()
    }
}

fn build(doc: &Document) -> Vec<CachedShape> {
    doc.shapes()
        .iter()
        .filter_map(|shape| {
            let color = doc.layer(shape.layer_id)?.color;
            // Only an invalid tolerance fails, and ours is a valid constant.
            let polylines = flatten_path(&shape.world_path(), DISPLAY_TOLERANCE_MM).ok()?;
            let bounds =
                Rect::from_points(polylines.iter().flat_map(|p| p.points.iter().copied()))?;
            Some(CachedShape {
                id: shape.id,
                color,
                polylines,
                bounds,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::{EPS, Path, Point, RectShape, ShapeKind, Transform};

    fn doc_with_rect() -> Document {
        let mut doc = Document::new();
        let layer = doc.add_layer("Cut", [255, 0, 0]).unwrap();
        let rect = ShapeKind::Rect(RectShape::new(10.0, 5.0, 0.0).unwrap());
        doc.add_shape(rect, Transform::translate(20.0, 30.0), layer)
            .unwrap();
        doc.add_shape(
            ShapeKind::Path(Path::default()),
            Transform::identity(),
            layer,
        )
        .unwrap();
        doc
    }

    #[test]
    fn caches_world_outlines_with_layer_color_and_skips_empty_shapes() {
        let mut cache = DisplayCache::default();
        cache.sync(&doc_with_rect(), 0);
        assert_eq!(cache.shapes().len(), 1);
        let shape = &cache.shapes()[0];
        assert_eq!(shape.color, [255, 0, 0]);
        assert_eq!(shape.polylines.len(), 1);
        assert!(shape.polylines[0].closed);
        let expected = Rect::new(Point::new(20.0, 30.0), Point::new(30.0, 35.0));
        assert!(shape.bounds.approx_eq(&expected, EPS));
    }

    #[test]
    fn rebuilds_only_when_the_revision_changes() {
        let mut cache = DisplayCache::default();
        cache.sync(&doc_with_rect(), 1);
        cache.sync(&Document::new(), 1);
        assert_eq!(cache.shapes().len(), 1);
        cache.sync(&Document::new(), 2);
        assert!(cache.shapes().is_empty());
    }

    #[test]
    fn preview_is_planned_once_per_change() {
        let doc = doc_with_rect();
        let profile = MachineProfile::default();
        let options = JobOptions::default();
        let mut cache = PreviewCache::default();
        assert!(cache.result().is_none());

        cache.sync(&doc, 1, &profile, &options);
        let first = cache.result().unwrap().as_ref().unwrap().clone();
        assert_eq!(first.cuts.len(), 1);

        // Same key: the cached result stays, even for another document.
        cache.sync(&Document::new(), 1, &profile, &options);
        assert!(cache.result().unwrap().is_ok());

        // A new revision replans.
        cache.sync(&Document::new(), 2, &profile, &options);
        assert_eq!(cache.result().unwrap(), &Err(GcodeError::NothingToOutput));

        // So does a changed option.
        let shifted = JobOptions {
            origin: pf_gcode::JobOrigin::UserOrigin(Point::new(25.0, 0.0)),
            ..JobOptions::default()
        };
        cache.sync(&doc, 2, &profile, &shifted);
        assert!(matches!(
            cache.result().unwrap(),
            Err(GcodeError::OutOfBounds { .. })
        ));
    }
}
