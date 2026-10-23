use thiserror::Error;

use crate::geometry::Rect;
use crate::layer::{Layer, LayerId};
use crate::shape::{Shape, ShapeId, ShapeKind};
use crate::transform::Transform;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum DocumentError {
    #[error("unknown layer {0:?}")]
    UnknownLayer(LayerId),
    #[error("unknown shape {0:?}")]
    UnknownShape(ShapeId),
    #[error("layer {layer:?} still has {shapes} shape(s) assigned")]
    LayerInUse { layer: LayerId, shapes: usize },
    #[error("layer index {index} is out of range for {len} layer(s)")]
    IndexOutOfRange { index: usize, len: usize },
    #[error("no more {0} IDs are available in this document")]
    IdsExhausted(&'static str),
}

/// Invariants: every shape's `layer_id` names an existing layer, and IDs
/// are unique and never reused within one document.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    layers: Vec<Layer>, // cut order: first is processed first
    shapes: Vec<Shape>, // insertion order
    next_layer_id: u32,
    next_shape_id: u64,
}

impl Document {
    pub fn new() -> Document {
        Document::default()
    }

    /// Layers in cut order.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    pub fn layer_index(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    /// Appends a new layer with default settings at the end of the cut order.
    pub fn add_layer(
        &mut self,
        name: impl Into<String>,
        color: [u8; 3],
    ) -> Result<LayerId, DocumentError> {
        let id = LayerId(self.next_layer_id);
        self.next_layer_id = self
            .next_layer_id
            .checked_add(1)
            .ok_or(DocumentError::IdsExhausted("layer"))?;
        self.layers.push(Layer::new(id, name, color));
        Ok(id)
    }

    /// Edits a layer's settings in place. The layer's `id` is restored
    /// afterwards, so shape assignments cannot be broken by an edit.
    pub fn update_layer(
        &mut self,
        id: LayerId,
        edit: impl FnOnce(&mut Layer),
    ) -> Result<(), DocumentError> {
        let layer = self
            .layers
            .iter_mut()
            .find(|l| l.id == id)
            .ok_or(DocumentError::UnknownLayer(id))?;
        edit(layer);
        layer.id = id;
        Ok(())
    }

    pub fn remove_layer(&mut self, id: LayerId) -> Result<Layer, DocumentError> {
        let index = self
            .layer_index(id)
            .ok_or(DocumentError::UnknownLayer(id))?;
        let shapes = self.shapes_in_layer(id).count();
        if shapes > 0 {
            return Err(DocumentError::LayerInUse { layer: id, shapes });
        }
        Ok(self.layers.remove(index))
    }

    pub fn move_layer(&mut self, id: LayerId, new_index: usize) -> Result<(), DocumentError> {
        let index = self
            .layer_index(id)
            .ok_or(DocumentError::UnknownLayer(id))?;
        let len = self.layers.len();
        if new_index >= len {
            return Err(DocumentError::IndexOutOfRange {
                index: new_index,
                len,
            });
        }
        let layer = self.layers.remove(index);
        self.layers.insert(new_index, layer);
        Ok(())
    }

    pub fn shapes(&self) -> &[Shape] {
        &self.shapes
    }

    pub fn shape(&self, id: ShapeId) -> Option<&Shape> {
        self.shapes.iter().find(|s| s.id == id)
    }

    pub fn shapes_in_layer(&self, layer_id: LayerId) -> impl Iterator<Item = &Shape> + '_ {
        self.shapes.iter().filter(move |s| s.layer_id == layer_id)
    }

    pub fn add_shape(
        &mut self,
        kind: ShapeKind,
        transform: Transform,
        layer_id: LayerId,
    ) -> Result<ShapeId, DocumentError> {
        self.require_layer(layer_id)?;
        let id = ShapeId(self.next_shape_id);
        self.next_shape_id = self
            .next_shape_id
            .checked_add(1)
            .ok_or(DocumentError::IdsExhausted("shape"))?;
        self.shapes.push(Shape {
            id,
            kind,
            transform,
            layer_id,
        });
        Ok(id)
    }

    pub fn remove_shape(&mut self, id: ShapeId) -> Result<Shape, DocumentError> {
        let index = self
            .shapes
            .iter()
            .position(|s| s.id == id)
            .ok_or(DocumentError::UnknownShape(id))?;
        Ok(self.shapes.remove(index))
    }

    pub fn assign_shape(&mut self, id: ShapeId, layer_id: LayerId) -> Result<(), DocumentError> {
        self.require_layer(layer_id)?;
        self.shape_mut(id)?.layer_id = layer_id;
        Ok(())
    }

    pub fn set_transform(
        &mut self,
        id: ShapeId,
        transform: Transform,
    ) -> Result<(), DocumentError> {
        self.shape_mut(id)?.transform = transform;
        Ok(())
    }

    pub fn bounds(&self) -> Option<Rect> {
        self.shapes
            .iter()
            .filter_map(Shape::bounds)
            .reduce(|a, b| a.union(&b))
    }

    /// The ID the next `add_layer` will issue.
    pub(crate) fn next_layer_id(&self) -> u32 {
        self.next_layer_id
    }

    /// The ID the next `add_shape` will issue.
    pub(crate) fn next_shape_id(&self) -> u64 {
        self.next_shape_id
    }

    /// Restores a layer with its existing ID. The caller guarantees the ID
    /// is not in use and `index <= layers.len()`. The ID counter only moves
    /// forward, so restored IDs are never issued again.
    pub(crate) fn insert_layer(&mut self, index: usize, layer: Layer) {
        self.next_layer_id = self.next_layer_id.max(layer.id.0.saturating_add(1));
        self.layers.insert(index, layer);
    }

    /// Restores a shape with its existing ID. The caller guarantees the ID
    /// is not in use, its layer exists and `index <= shapes.len()`.
    pub(crate) fn insert_shape(&mut self, index: usize, shape: Shape) {
        self.next_shape_id = self.next_shape_id.max(shape.id.0.saturating_add(1));
        self.shapes.insert(index, shape);
    }

    /// The caller guarantees `index < layers.len()` and the layer is unused.
    pub(crate) fn remove_layer_at(&mut self, index: usize) -> Layer {
        self.layers.remove(index)
    }

    /// Direct access for history edits. The caller must keep `id` unchanged
    /// and only assign layers that exist.
    pub(crate) fn shape_at_mut(&mut self, index: usize) -> Option<&mut Shape> {
        self.shapes.get_mut(index)
    }

    /// The caller guarantees `index < shapes.len()`.
    pub(crate) fn remove_shape_at(&mut self, index: usize) -> Shape {
        self.shapes.remove(index)
    }

    fn require_layer(&self, id: LayerId) -> Result<(), DocumentError> {
        match self.layer(id) {
            Some(_) => Ok(()),
            None => Err(DocumentError::UnknownLayer(id)),
        }
    }

    fn shape_mut(&mut self, id: ShapeId) -> Result<&mut Shape, DocumentError> {
        self.shapes
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(DocumentError::UnknownShape(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{EPS, Point, approx_eq};
    use crate::path::Path;
    use crate::shape::{EllipseShape, RectShape};

    fn rect(w: f64, h: f64) -> ShapeKind {
        ShapeKind::Rect(RectShape::new(w, h, 0.0).unwrap())
    }

    fn ids(doc: &Document) -> Vec<LayerId> {
        doc.layers().iter().map(|l| l.id).collect()
    }

    #[test]
    fn new_document_is_empty() {
        let doc = Document::new();
        assert!(doc.layers().is_empty());
        assert!(doc.shapes().is_empty());
        assert_eq!(doc.bounds(), None);
    }

    #[test]
    fn add_layers_in_order_with_unique_ids() {
        let mut doc = Document::new();
        let a = doc.add_layer("Cut", [255, 0, 0]).unwrap();
        let b = doc.add_layer("Engrave", [0, 0, 255]).unwrap();
        assert_ne!(a, b);
        assert_eq!(ids(&doc), vec![a, b]);
        assert_eq!(doc.layer(b).unwrap().name, "Engrave");
        assert_eq!(doc.layer_index(b), Some(1));
    }

    #[test]
    fn add_and_remove_shape_round_trip() {
        let mut doc = Document::new();
        let layer = doc.add_layer("Cut", [0, 0, 0]).unwrap();
        let id = doc
            .add_shape(rect(10.0, 5.0), Transform::identity(), layer)
            .unwrap();
        assert_eq!(doc.shape(id).unwrap().layer_id, layer);
        let removed = doc.remove_shape(id).unwrap();
        assert_eq!(removed.id, id);
        assert!(doc.shapes().is_empty());
        assert_eq!(doc.shape(id), None);
        assert_eq!(doc.remove_shape(id), Err(DocumentError::UnknownShape(id)));
    }

    #[test]
    fn ids_are_not_reused_after_removal() {
        let mut doc = Document::new();
        let layer = doc.add_layer("Cut", [0, 0, 0]).unwrap();
        let first = doc
            .add_shape(rect(1.0, 1.0), Transform::identity(), layer)
            .unwrap();
        doc.remove_shape(first).unwrap();
        let second = doc
            .add_shape(rect(1.0, 1.0), Transform::identity(), layer)
            .unwrap();
        assert_ne!(first, second);

        let spare = doc.add_layer("Spare", [0, 0, 0]).unwrap();
        doc.remove_layer(spare).unwrap();
        let next = doc.add_layer("Next", [0, 0, 0]).unwrap();
        assert_ne!(spare, next);
    }

    #[test]
    fn add_shape_to_unknown_layer_fails() {
        let mut doc = Document::new();
        let missing = LayerId(42);
        assert_eq!(
            doc.add_shape(rect(1.0, 1.0), Transform::identity(), missing),
            Err(DocumentError::UnknownLayer(missing))
        );
        assert!(doc.shapes().is_empty());
    }

    #[test]
    fn assign_shape_moves_between_layers() {
        let mut doc = Document::new();
        let a = doc.add_layer("A", [0, 0, 0]).unwrap();
        let b = doc.add_layer("B", [0, 0, 0]).unwrap();
        let s1 = doc
            .add_shape(rect(1.0, 1.0), Transform::identity(), a)
            .unwrap();
        let s2 = doc
            .add_shape(rect(2.0, 2.0), Transform::identity(), a)
            .unwrap();
        doc.assign_shape(s2, b).unwrap();
        let in_a: Vec<ShapeId> = doc.shapes_in_layer(a).map(|s| s.id).collect();
        let in_b: Vec<ShapeId> = doc.shapes_in_layer(b).map(|s| s.id).collect();
        assert_eq!(in_a, vec![s1]);
        assert_eq!(in_b, vec![s2]);
    }

    #[test]
    fn assign_to_unknown_layer_or_shape_fails() {
        let mut doc = Document::new();
        let a = doc.add_layer("A", [0, 0, 0]).unwrap();
        let s = doc
            .add_shape(rect(1.0, 1.0), Transform::identity(), a)
            .unwrap();
        assert_eq!(
            doc.assign_shape(s, LayerId(99)),
            Err(DocumentError::UnknownLayer(LayerId(99)))
        );
        assert_eq!(doc.shape(s).unwrap().layer_id, a);
        assert_eq!(
            doc.assign_shape(ShapeId(99), a),
            Err(DocumentError::UnknownShape(ShapeId(99)))
        );
    }

    #[test]
    fn remove_layer_in_use_fails() {
        let mut doc = Document::new();
        let a = doc.add_layer("A", [0, 0, 0]).unwrap();
        doc.add_shape(rect(1.0, 1.0), Transform::identity(), a)
            .unwrap();
        doc.add_shape(rect(1.0, 1.0), Transform::identity(), a)
            .unwrap();
        assert_eq!(
            doc.remove_layer(a),
            Err(DocumentError::LayerInUse {
                layer: a,
                shapes: 2
            })
        );
        assert_eq!(doc.layers().len(), 1);
    }

    #[test]
    fn remove_empty_and_unknown_layer() {
        let mut doc = Document::new();
        let a = doc.add_layer("A", [0, 0, 0]).unwrap();
        let removed = doc.remove_layer(a).unwrap();
        assert_eq!(removed.id, a);
        assert!(doc.layers().is_empty());
        assert_eq!(doc.remove_layer(a), Err(DocumentError::UnknownLayer(a)));
    }

    #[test]
    fn move_layer_reorders_and_keeps_assignments() {
        let mut doc = Document::new();
        let a = doc.add_layer("A", [0, 0, 0]).unwrap();
        let b = doc.add_layer("B", [0, 0, 0]).unwrap();
        let c = doc.add_layer("C", [0, 0, 0]).unwrap();
        let s = doc
            .add_shape(rect(1.0, 1.0), Transform::identity(), a)
            .unwrap();

        doc.move_layer(a, 2).unwrap();
        assert_eq!(ids(&doc), vec![b, c, a]);
        doc.move_layer(c, 0).unwrap();
        assert_eq!(ids(&doc), vec![c, b, a]);
        assert_eq!(doc.shape(s).unwrap().layer_id, a);

        assert_eq!(
            doc.move_layer(a, 3),
            Err(DocumentError::IndexOutOfRange { index: 3, len: 3 })
        );
        assert_eq!(
            doc.move_layer(LayerId(99), 0),
            Err(DocumentError::UnknownLayer(LayerId(99)))
        );
    }

    #[test]
    fn update_layer_edits_settings_but_not_id() {
        let mut doc = Document::new();
        let a = doc.add_layer("A", [0, 0, 0]).unwrap();
        doc.update_layer(a, |layer| {
            layer.speed_mm_min = 3000.0;
            layer.passes = 3;
            layer.id = LayerId(1234);
        })
        .unwrap();
        let layer = doc.layer(a).unwrap();
        assert!(approx_eq(layer.speed_mm_min, 3000.0, EPS));
        assert_eq!(layer.passes, 3);
        assert_eq!(layer.id, a);
        assert_eq!(
            doc.update_layer(LayerId(99), |_| {}),
            Err(DocumentError::UnknownLayer(LayerId(99)))
        );
    }

    #[test]
    fn set_transform_updates_bounds() {
        let mut doc = Document::new();
        let a = doc.add_layer("A", [0, 0, 0]).unwrap();
        let s = doc
            .add_shape(rect(10.0, 5.0), Transform::identity(), a)
            .unwrap();
        doc.set_transform(s, Transform::translate(20.0, 30.0))
            .unwrap();
        let expected = Rect::new(Point::new(20.0, 30.0), Point::new(30.0, 35.0));
        assert!(doc.bounds().unwrap().approx_eq(&expected, EPS));
        assert_eq!(
            doc.set_transform(ShapeId(99), Transform::identity()),
            Err(DocumentError::UnknownShape(ShapeId(99)))
        );
    }

    #[test]
    fn document_bounds_cover_transformed_shapes() {
        let mut doc = Document::new();
        let a = doc.add_layer("A", [0, 0, 0]).unwrap();
        let b = doc.add_layer("B", [0, 0, 0]).unwrap();
        doc.add_shape(rect(10.0, 10.0), Transform::translate(5.0, 5.0), a)
            .unwrap();
        let ellipse = ShapeKind::Ellipse(EllipseShape::new(4.0, 2.0).unwrap());
        doc.add_shape(
            ellipse,
            Transform::rotate(std::f64::consts::FRAC_PI_2).then(Transform::translate(50.0, 40.0)),
            b,
        )
        .unwrap();
        // An empty path contributes no bounds.
        doc.add_shape(ShapeKind::Path(Path::default()), Transform::identity(), a)
            .unwrap();
        // Rect: (5,5)-(15,15). Rotated ellipse: (48,36)-(52,44).
        let expected = Rect::new(Point::new(5.0, 5.0), Point::new(52.0, 44.0));
        assert!(doc.bounds().unwrap().approx_eq(&expected, 1e-9));
    }

    #[test]
    fn layer_ids_exhausted() {
        let mut doc = Document {
            next_layer_id: u32::MAX,
            ..Document::default()
        };
        assert_eq!(
            doc.add_layer("Last", [0, 0, 0]),
            Err(DocumentError::IdsExhausted("layer"))
        );
    }
}
