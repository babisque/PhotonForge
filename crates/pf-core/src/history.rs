//! Undo and redo for document edits.
//!
//! The app describes an edit as a `Command`. Applying it records a `Change`
//! that holds both the before and the after state, so undo and redo put back
//! exactly the same layers and shapes, IDs included. Later entries that refer
//! to those IDs therefore stay valid.

use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::document::{Document, DocumentError};
use crate::layer::{Layer, LayerId, LayerMode, PowerMode};
use crate::shape::{Shape, ShapeId};
use crate::transform::Transform;

/// Older entries are dropped beyond this many undo steps.
pub const MAX_UNDO: usize = 500;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum HistoryError {
    #[error(transparent)]
    Document(#[from] DocumentError),
    /// A recorded change does not match the document any more. This means a
    /// bug: the document was edited without going through `History`.
    #[error("the undo history no longer matches the document")]
    OutOfSync,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Appends the imported shapes after the existing ones. Each imported
    /// layer joins the first existing layer with the same color and mode,
    /// or is appended as a new layer.
    ImportDocument(Document),
    /// New transforms for existing shapes.
    SetTransforms(Vec<(ShapeId, Transform)>),
    DeleteShapes(Vec<ShapeId>),
    AssignShapes {
        ids: Vec<ShapeId>,
        layer: LayerId,
    },
    SetLayerName {
        id: LayerId,
        name: String,
    },
    SetLayerMode {
        id: LayerId,
        mode: LayerMode,
    },
    SetLayerSpeed {
        id: LayerId,
        speed: f64,
    }, // mm/min
    SetLayerPower {
        id: LayerId,
        power: f64,
    }, // %
    SetLayerPasses {
        id: LayerId,
        passes: u32,
    },
    SetLayerOutput {
        id: LayerId,
        enabled: bool,
    },
    SetLayerPowerMode {
        id: LayerId,
        mode: PowerMode,
    },
    MoveLayer {
        id: LayerId,
        to: usize,
    },
}

impl Command {
    /// Short description for menus and status messages.
    pub fn label(&self) -> &'static str {
        match self {
            Command::ImportDocument(_) => "Import",
            Command::SetTransforms(_) => "Transform",
            Command::DeleteShapes(_) => "Delete",
            Command::AssignShapes { .. } => "Assign to layer",
            Command::SetLayerName { .. } => "Rename layer",
            Command::SetLayerMode { .. } => "Change layer mode",
            Command::SetLayerSpeed { .. } => "Change speed",
            Command::SetLayerPower { .. } => "Change power",
            Command::SetLayerPasses { .. } => "Change passes",
            Command::SetLayerOutput { .. } => "Toggle output",
            Command::SetLayerPowerMode { .. } => "Change power mode",
            Command::MoveLayer { .. } => "Reorder layers",
        }
    }

    /// Commands with equal keys may be merged into one undo step.
    fn merge_key(&self) -> Option<MergeKey> {
        match self {
            Command::SetTransforms(items) => {
                Some(MergeKey::Shapes(items.iter().map(|(id, _)| *id).collect()))
            }
            Command::SetLayerName { id, .. }
            | Command::SetLayerMode { id, .. }
            | Command::SetLayerSpeed { id, .. }
            | Command::SetLayerPower { id, .. }
            | Command::SetLayerPasses { id, .. }
            | Command::SetLayerOutput { id, .. }
            | Command::SetLayerPowerMode { id, .. } => Some(MergeKey::Layer(*id, self.label())),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum MergeKey {
    Shapes(Vec<ShapeId>),
    Layer(LayerId, &'static str), // the label tells the edited field apart
}

/// An invertible edit. Index lists are in ascending order.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    Insert {
        layers: Vec<(usize, Layer)>,
        shapes: Vec<(usize, Shape)>,
    },
    Remove {
        layers: Vec<(usize, Layer)>,
        shapes: Vec<(usize, Shape)>,
    },
    Transforms(Vec<(ShapeId, Transform, Transform)>), // shape, from, to
    Assign(Vec<(ShapeId, LayerId, LayerId)>),         // shape, from, to
    Layer {
        before: Layer,
        after: Layer,
    },
    MoveLayer {
        id: LayerId,
        from: usize,
        to: usize,
    },
}

impl Change {
    fn inverse(&self) -> Change {
        match self {
            Change::Insert { layers, shapes } => Change::Remove {
                layers: layers.clone(),
                shapes: shapes.clone(),
            },
            Change::Remove { layers, shapes } => Change::Insert {
                layers: layers.clone(),
                shapes: shapes.clone(),
            },
            Change::Transforms(items) => Change::Transforms(
                items
                    .iter()
                    .rev()
                    .map(|&(id, from, to)| (id, to, from))
                    .collect(),
            ),
            Change::Assign(items) => Change::Assign(
                items
                    .iter()
                    .rev()
                    .map(|&(id, from, to)| (id, to, from))
                    .collect(),
            ),
            Change::Layer { before, after } => Change::Layer {
                before: after.clone(),
                after: before.clone(),
            },
            Change::MoveLayer { id, from, to } => Change::MoveLayer {
                id: *id,
                from: *to,
                to: *from,
            },
        }
    }

    /// Folds a later change with the same merge key into this one. Returns
    /// false when the two cannot be combined.
    fn absorb(&mut self, later: &Change) -> bool {
        match (self, later) {
            (Change::Layer { after, .. }, Change::Layer { after: newer, .. }) => {
                *after = newer.clone();
                true
            }
            (Change::Transforms(items), Change::Transforms(newer))
                if items.len() == newer.len() =>
            {
                for (item, newer) in items.iter_mut().zip(newer) {
                    item.2 = newer.2;
                }
                true
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    change: Change,
    label: &'static str,
    merge_key: Option<MergeKey>,
}

#[derive(Debug, Clone, Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    revision: u64,
}

impl History {
    pub fn new() -> History {
        History::default()
    }

    /// Applies `command` as a new undo step. On error neither the document
    /// nor the history changes.
    pub fn apply(&mut self, doc: &mut Document, command: Command) -> Result<(), HistoryError> {
        self.record(doc, command, false)
    }

    /// Like `apply`, but merges into the previous step when that step came
    /// from the same kind of command on the same target and has not been
    /// sealed. Used for continuous edits such as dragging a value.
    pub fn apply_merged(
        &mut self,
        doc: &mut Document,
        command: Command,
    ) -> Result<(), HistoryError> {
        self.record(doc, command, true)
    }

    /// Ends a merged gesture: later commands start a new undo step.
    pub fn seal(&mut self) {
        if let Some(top) = self.undo.last_mut() {
            top.merge_key = None;
        }
    }

    /// Reverts the latest step and returns its label, or `None` when there
    /// is nothing to undo.
    pub fn undo(&mut self, doc: &mut Document) -> Result<Option<&'static str>, HistoryError> {
        let Some(entry) = self.undo.pop() else {
            return Ok(None);
        };
        if let Err(err) = perform(doc, &entry.change.inverse()) {
            self.undo.push(entry);
            return Err(err);
        }
        let label = entry.label;
        self.redo.push(Entry {
            merge_key: None,
            ..entry
        });
        self.bump();
        Ok(Some(label))
    }

    /// Re-applies the latest undone step and returns its label, or `None`
    /// when there is nothing to redo.
    pub fn redo(&mut self, doc: &mut Document) -> Result<Option<&'static str>, HistoryError> {
        let Some(entry) = self.redo.pop() else {
            return Ok(None);
        };
        if let Err(err) = perform(doc, &entry.change) {
            self.redo.push(entry);
            return Err(err);
        }
        let label = entry.label;
        self.undo.push(entry);
        self.bump();
        Ok(Some(label))
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<&'static str> {
        self.undo.last().map(|e| e.label)
    }

    pub fn redo_label(&self) -> Option<&'static str> {
        self.redo.last().map(|e| e.label)
    }

    /// Changes whenever the document changes through this history.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn record(
        &mut self,
        doc: &mut Document,
        command: Command,
        merge: bool,
    ) -> Result<(), HistoryError> {
        let label = command.label();
        // Only merged steps can absorb later ones.
        let merge_key = if merge { command.merge_key() } else { None };
        let change = plan(doc, command)?;
        perform(doc, &change)?;

        let merged = merge_key.is_some()
            && self.redo.is_empty()
            && match self.undo.last_mut() {
                Some(top) if top.merge_key == merge_key => top.change.absorb(&change),
                _ => false,
            };
        if !merged {
            self.undo.push(Entry {
                change,
                label,
                merge_key,
            });
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.bump();
        Ok(())
    }

    fn bump(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

/// Turns a command into a change against the current document, validating
/// every ID. Does not modify the document.
fn plan(doc: &Document, command: Command) -> Result<Change, HistoryError> {
    let change = match command {
        Command::ImportDocument(imported) => plan_import(doc, &imported)?,
        Command::SetTransforms(items) => {
            let index = shape_index(doc);
            let mut planned = Vec::with_capacity(items.len());
            for (id, to) in items {
                let from = lookup(doc, &index, id)?.transform;
                planned.push((id, from, to));
            }
            Change::Transforms(planned)
        }
        Command::DeleteShapes(ids) => {
            let index = shape_index(doc);
            let mut shapes = Vec::with_capacity(ids.len());
            for id in ids {
                let i = *index.get(&id).ok_or(DocumentError::UnknownShape(id))?;
                shapes.push((i, doc.shapes()[i].clone()));
            }
            shapes.sort_by_key(|(i, _)| *i);
            shapes.dedup_by_key(|(i, _)| *i);
            Change::Remove {
                layers: Vec::new(),
                shapes,
            }
        }
        Command::AssignShapes { ids, layer } => {
            if doc.layer(layer).is_none() {
                return Err(DocumentError::UnknownLayer(layer).into());
            }
            let index = shape_index(doc);
            let mut planned = Vec::with_capacity(ids.len());
            for id in ids {
                let from = lookup(doc, &index, id)?.layer_id;
                planned.push((id, from, layer));
            }
            Change::Assign(planned)
        }
        Command::SetLayerName { id, name } => edit_layer(doc, id, |l| l.name = name)?,
        Command::SetLayerMode { id, mode } => edit_layer(doc, id, |l| l.mode = mode)?,
        Command::SetLayerSpeed { id, speed } => edit_layer(doc, id, |l| l.speed_mm_min = speed)?,
        Command::SetLayerPower { id, power } => edit_layer(doc, id, |l| l.power_percent = power)?,
        Command::SetLayerPasses { id, passes } => edit_layer(doc, id, |l| l.passes = passes)?,
        Command::SetLayerOutput { id, enabled } => {
            edit_layer(doc, id, |l| l.output_enabled = enabled)?
        }
        Command::SetLayerPowerMode { id, mode } => edit_layer(doc, id, |l| l.power_mode = mode)?,
        Command::MoveLayer { id, to } => {
            let from = doc.layer_index(id).ok_or(DocumentError::UnknownLayer(id))?;
            let len = doc.layers().len();
            if to >= len {
                return Err(DocumentError::IndexOutOfRange { index: to, len }.into());
            }
            Change::MoveLayer { id, from, to }
        }
    };
    Ok(change)
}

fn plan_import(doc: &Document, imported: &Document) -> Result<Change, HistoryError> {
    let mut next_layer = doc.next_layer_id();
    let mut next_shape = doc.next_shape_id();
    let mut new_layers: Vec<(usize, Layer)> = Vec::new();
    let mut target: HashMap<LayerId, LayerId> = HashMap::new();

    for layer in imported.layers() {
        let same = |l: &Layer| l.color == layer.color && l.mode == layer.mode;
        let existing = doc
            .layers()
            .iter()
            .chain(new_layers.iter().map(|(_, l)| l))
            .find(|l| same(l))
            .map(|l| l.id);
        let id = match existing {
            Some(id) => id,
            None => {
                let id = LayerId(next_layer);
                next_layer = next_layer
                    .checked_add(1)
                    .ok_or(DocumentError::IdsExhausted("layer"))?;
                let index = doc.layers().len() + new_layers.len();
                new_layers.push((
                    index,
                    Layer {
                        id,
                        ..layer.clone()
                    },
                ));
                id
            }
        };
        target.insert(layer.id, id);
    }

    let mut new_shapes = Vec::with_capacity(imported.shapes().len());
    for (k, shape) in imported.shapes().iter().enumerate() {
        let layer_id = *target
            .get(&shape.layer_id)
            .ok_or(DocumentError::UnknownLayer(shape.layer_id))?;
        let id = ShapeId(next_shape);
        next_shape = next_shape
            .checked_add(1)
            .ok_or(DocumentError::IdsExhausted("shape"))?;
        new_shapes.push((
            doc.shapes().len() + k,
            Shape {
                id,
                layer_id,
                ..shape.clone()
            },
        ));
    }

    Ok(Change::Insert {
        layers: new_layers,
        shapes: new_shapes,
    })
}

fn edit_layer(
    doc: &Document,
    id: LayerId,
    edit: impl FnOnce(&mut Layer),
) -> Result<Change, HistoryError> {
    let before = doc
        .layer(id)
        .ok_or(DocumentError::UnknownLayer(id))?
        .clone();
    let mut after = before.clone();
    edit(&mut after);
    Ok(Change::Layer { before, after })
}

fn shape_index(doc: &Document) -> HashMap<ShapeId, usize> {
    doc.shapes()
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id, i))
        .collect()
}

fn lookup<'a>(
    doc: &'a Document,
    index: &HashMap<ShapeId, usize>,
    id: ShapeId,
) -> Result<&'a Shape, DocumentError> {
    index
        .get(&id)
        .map(|&i| &doc.shapes()[i])
        .ok_or(DocumentError::UnknownShape(id))
}

/// Applies a change. Everything is checked before the first mutation, so a
/// mismatch leaves the document untouched.
fn perform(doc: &mut Document, change: &Change) -> Result<(), HistoryError> {
    match change {
        Change::Insert { layers, shapes } => {
            let inserted: HashSet<LayerId> = layers.iter().map(|(_, l)| l.id).collect();
            let layers_ok = layers
                .iter()
                .enumerate()
                .all(|(k, (i, l))| doc.layer(l.id).is_none() && *i <= doc.layers().len() + k);
            let index = shape_index(doc);
            let shapes_ok = shapes.iter().enumerate().all(|(k, (i, s))| {
                !index.contains_key(&s.id)
                    && *i <= doc.shapes().len() + k
                    && (inserted.contains(&s.layer_id) || doc.layer(s.layer_id).is_some())
            });
            if !(layers_ok && shapes_ok) {
                return Err(HistoryError::OutOfSync);
            }
            for (i, layer) in layers {
                doc.insert_layer(*i, layer.clone());
            }
            for (i, shape) in shapes {
                doc.insert_shape(*i, shape.clone());
            }
        }
        Change::Remove { layers, shapes } => {
            let shapes_ok = shapes
                .iter()
                .all(|(i, s)| doc.shapes().get(*i).map(|x| x.id) == Some(s.id));
            let removed: HashSet<ShapeId> = shapes.iter().map(|(_, s)| s.id).collect();
            let layers_ok = layers.iter().all(|(i, l)| {
                doc.layers().get(*i).map(|x| x.id) == Some(l.id)
                    && doc.shapes_in_layer(l.id).all(|s| removed.contains(&s.id))
            });
            if !(shapes_ok && layers_ok) {
                return Err(HistoryError::OutOfSync);
            }
            for (i, _) in shapes.iter().rev() {
                doc.remove_shape_at(*i);
            }
            for (i, _) in layers.iter().rev() {
                doc.remove_layer_at(*i);
            }
        }
        Change::Transforms(items) => {
            let index = shape_index(doc);
            let positions = matching(doc, &index, items, |s, from| s.transform == *from)?;
            for (pos, (_, _, to)) in positions.into_iter().zip(items) {
                if let Some(shape) = doc.shape_at_mut(pos) {
                    shape.transform = *to;
                }
            }
        }
        Change::Assign(items) => {
            if items.iter().any(|(_, _, to)| doc.layer(*to).is_none()) {
                return Err(HistoryError::OutOfSync);
            }
            let index = shape_index(doc);
            let positions = matching(doc, &index, items, |s, from| s.layer_id == *from)?;
            for (pos, (_, _, to)) in positions.into_iter().zip(items) {
                if let Some(shape) = doc.shape_at_mut(pos) {
                    shape.layer_id = *to;
                }
            }
        }
        Change::Layer { before, after } => {
            if doc.layer(before.id) != Some(before) || before.id != after.id {
                return Err(HistoryError::OutOfSync);
            }
            doc.update_layer(before.id, |layer| *layer = after.clone())?;
        }
        Change::MoveLayer { id, from, to } => {
            let in_place = doc.layers().get(*from).map(|l| l.id) == Some(*id);
            if !in_place || *to >= doc.layers().len() {
                return Err(HistoryError::OutOfSync);
            }
            doc.move_layer(*id, *to)?;
        }
    }
    Ok(())
}

/// Shape positions for `(id, from, to)` items whose current state matches
/// `from`. Later items see the `to` of earlier items on the same shape.
fn matching<T: Copy>(
    doc: &Document,
    index: &HashMap<ShapeId, usize>,
    items: &[(ShapeId, T, T)],
    is_current: impl Fn(&Shape, &T) -> bool,
) -> Result<Vec<usize>, HistoryError> {
    let mut pending: HashMap<ShapeId, T> = HashMap::new();
    let mut positions = Vec::with_capacity(items.len());
    for (id, from, to) in items {
        let pos = *index.get(id).ok_or(HistoryError::OutOfSync)?;
        let ok = match pending.get(id) {
            Some(_) => true,
            None => is_current(&doc.shapes()[pos], from),
        };
        if !ok {
            return Err(HistoryError::OutOfSync);
        }
        pending.insert(*id, *to);
        positions.push(pos);
    }
    Ok(positions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::{RectShape, ShapeKind};

    fn rect() -> ShapeKind {
        ShapeKind::Rect(RectShape::new(10.0, 5.0, 0.0).unwrap())
    }

    /// Two layers (red, blue) with two shapes on red and one on blue.
    fn sample() -> (Document, LayerId, LayerId, Vec<ShapeId>) {
        let mut doc = Document::new();
        let red = doc.add_layer("Red", [255, 0, 0]).unwrap();
        let blue = doc.add_layer("Blue", [0, 0, 255]).unwrap();
        let ids = vec![
            doc.add_shape(rect(), Transform::identity(), red).unwrap(),
            doc.add_shape(rect(), Transform::translate(20.0, 0.0), red)
                .unwrap(),
            doc.add_shape(rect(), Transform::translate(0.0, 20.0), blue)
                .unwrap(),
        ];
        (doc, red, blue, ids)
    }

    fn imported(layers: &[([u8; 3], LayerMode)], shapes_per_layer: usize) -> Document {
        let mut doc = Document::new();
        for (i, (color, mode)) in layers.iter().enumerate() {
            let id = doc.add_layer(format!("L{i}"), *color).unwrap();
            doc.update_layer(id, |l| l.mode = *mode).unwrap();
            for _ in 0..shapes_per_layer {
                doc.add_shape(rect(), Transform::identity(), id).unwrap();
            }
        }
        doc
    }

    /// Applies, undoes and redoes `command`, checking the document matches
    /// the original after undo and the edited state after redo.
    fn round_trip(doc: &mut Document, command: Command) {
        let original = doc.clone();
        let mut history = History::new();
        history.apply(doc, command).unwrap();
        let edited = doc.clone();
        assert_ne!(edited, original, "command had no effect");
        assert!(history.undo(doc).unwrap().is_some());
        // ID counters stay advanced after undo, so compare content only.
        assert_eq!(doc.layers(), original.layers());
        assert_eq!(doc.shapes(), original.shapes());
        assert!(history.redo(doc).unwrap().is_some());
        assert_eq!(*doc, edited);
    }

    #[test]
    fn every_command_round_trips() {
        let (doc, red, blue, ids) = sample();
        let commands = vec![
            Command::ImportDocument(imported(&[([0, 170, 0], LayerMode::Line)], 2)),
            Command::SetTransforms(vec![
                (ids[0], Transform::translate(5.0, 5.0)),
                (ids[2], Transform::rotate(1.0)),
            ]),
            Command::DeleteShapes(vec![ids[2], ids[0]]),
            Command::AssignShapes {
                ids: vec![ids[0], ids[2]],
                layer: blue,
            },
            Command::SetLayerName {
                id: red,
                name: "Cut".into(),
            },
            Command::SetLayerMode {
                id: red,
                mode: LayerMode::Fill,
            },
            Command::SetLayerSpeed {
                id: red,
                speed: 2500.0,
            },
            Command::SetLayerPower {
                id: red,
                power: 42.0,
            },
            Command::SetLayerPasses { id: red, passes: 3 },
            Command::SetLayerOutput {
                id: red,
                enabled: false,
            },
            Command::SetLayerPowerMode {
                id: red,
                mode: PowerMode::M3Constant,
            },
            Command::MoveLayer { id: red, to: 1 },
        ];
        for command in commands {
            let mut doc = doc.clone();
            round_trip(&mut doc, command);
        }
    }

    #[test]
    fn import_reuses_layers_with_the_same_color_and_mode() {
        let (mut doc, red, _, ids) = sample();
        let mut history = History::new();
        let incoming = imported(
            &[
                ([255, 0, 0], LayerMode::Line), // joins Red
                ([255, 0, 0], LayerMode::Fill), // same color, other mode: new
                ([0, 170, 0], LayerMode::Line), // new
            ],
            1,
        );
        history
            .apply(&mut doc, Command::ImportDocument(incoming))
            .unwrap();

        assert_eq!(doc.layers().len(), 4);
        assert_eq!(doc.shapes().len(), 6);
        let added = &doc.shapes()[3..];
        assert_eq!(added[0].layer_id, red);
        assert_ne!(added[1].layer_id, red);
        assert_eq!(doc.layer(added[1].layer_id).unwrap().mode, LayerMode::Fill);
        assert_eq!(doc.layer(added[2].layer_id).unwrap().color, [0, 170, 0]);
        // Fresh IDs that do not collide with existing ones.
        for shape in added {
            assert!(!ids.contains(&shape.id));
        }
    }

    #[test]
    fn redo_restores_the_same_ids_so_later_steps_stay_valid() {
        let mut doc = Document::new();
        let mut history = History::new();
        let incoming = imported(&[([0, 0, 0], LayerMode::Line)], 1);
        history
            .apply(&mut doc, Command::ImportDocument(incoming))
            .unwrap();
        let shape = doc.shapes()[0].id;
        let moved = Transform::translate(7.0, 0.0);
        history
            .apply(&mut doc, Command::SetTransforms(vec![(shape, moved)]))
            .unwrap();
        let final_state = doc.clone();

        history.undo(&mut doc).unwrap();
        history.undo(&mut doc).unwrap();
        assert!(doc.shapes().is_empty());
        history.redo(&mut doc).unwrap();
        assert_eq!(doc.shapes()[0].id, shape);
        history.redo(&mut doc).unwrap();
        assert_eq!(doc, final_state);
    }

    #[test]
    fn ids_are_not_reissued_after_undoing_an_import() {
        let mut doc = Document::new();
        let mut history = History::new();
        let incoming = imported(&[([0, 0, 0], LayerMode::Line)], 1);
        history
            .apply(&mut doc, Command::ImportDocument(incoming))
            .unwrap();
        let first = doc.shapes()[0].id;
        history.redo(&mut doc).unwrap();
        history.undo(&mut doc).unwrap();
        let layer = doc.add_layer("New", [1, 2, 3]).unwrap();
        let next = doc.add_shape(rect(), Transform::identity(), layer).unwrap();
        // The import's IDs were issued once; the counter never went back.
        assert_ne!(next, first);
    }

    #[test]
    fn new_apply_clears_redo() {
        let (mut doc, red, _, _) = sample();
        let mut history = History::new();
        history
            .apply(&mut doc, Command::SetLayerPasses { id: red, passes: 2 })
            .unwrap();
        history.undo(&mut doc).unwrap();
        assert!(history.can_redo());
        history
            .apply(&mut doc, Command::SetLayerPasses { id: red, passes: 5 })
            .unwrap();
        assert!(!history.can_redo());
        assert_eq!(history.redo(&mut doc).unwrap(), None);
    }

    #[test]
    fn failing_command_changes_nothing() {
        let (mut doc, red, _, ids) = sample();
        let mut history = History::new();
        let before = doc.clone();
        let missing = ShapeId(999);
        let failures = vec![
            Command::SetTransforms(vec![
                (ids[0], Transform::translate(1.0, 1.0)),
                (missing, Transform::identity()),
            ]),
            Command::DeleteShapes(vec![ids[0], missing]),
            Command::AssignShapes {
                ids: vec![ids[0]],
                layer: LayerId(77),
            },
            Command::SetLayerSpeed {
                id: LayerId(77),
                speed: 1.0,
            },
            Command::MoveLayer { id: red, to: 2 },
        ];
        for command in failures {
            assert!(history.apply(&mut doc, command).is_err());
        }
        assert_eq!(doc, before);
        assert!(!history.can_undo());
        assert_eq!(history.revision(), 0);
    }

    #[test]
    fn out_of_sync_undo_is_refused_and_kept() {
        let (mut doc, red, _, _) = sample();
        let mut history = History::new();
        history
            .apply(
                &mut doc,
                Command::SetLayerSpeed {
                    id: red,
                    speed: 500.0,
                },
            )
            .unwrap();
        // An edit behind the history's back.
        doc.update_layer(red, |l| l.speed_mm_min = 1.0).unwrap();
        let tampered = doc.clone();
        assert_eq!(history.undo(&mut doc), Err(HistoryError::OutOfSync));
        assert_eq!(doc, tampered);
        assert!(history.can_undo());
    }

    #[test]
    fn merged_edits_form_one_step_until_sealed() {
        let (mut doc, red, _, _) = sample();
        let original = doc.clone();
        let mut history = History::new();
        for speed in [1100.0, 1200.0, 1300.0] {
            history
                .apply_merged(&mut doc, Command::SetLayerSpeed { id: red, speed })
                .unwrap();
        }
        // A different field on the same layer is a separate step.
        history
            .apply_merged(
                &mut doc,
                Command::SetLayerPower {
                    id: red,
                    power: 5.0,
                },
            )
            .unwrap();
        history.undo(&mut doc).unwrap();
        assert_eq!(doc.layer(red).unwrap().speed_mm_min, 1300.0);
        history.undo(&mut doc).unwrap();
        assert_eq!(doc, original);
        assert!(!history.can_undo());

        history
            .apply_merged(
                &mut doc,
                Command::SetLayerSpeed {
                    id: red,
                    speed: 900.0,
                },
            )
            .unwrap();
        history.seal();
        history
            .apply_merged(
                &mut doc,
                Command::SetLayerSpeed {
                    id: red,
                    speed: 800.0,
                },
            )
            .unwrap();
        history.undo(&mut doc).unwrap();
        assert_eq!(doc.layer(red).unwrap().speed_mm_min, 900.0);
    }

    #[test]
    fn merged_transforms_keep_the_first_before_state() {
        let (mut doc, _, _, ids) = sample();
        let original = doc.clone();
        let mut history = History::new();
        for dx in [1.0, 2.0, 3.0] {
            history
                .apply_merged(
                    &mut doc,
                    Command::SetTransforms(vec![(ids[0], Transform::translate(dx, 0.0))]),
                )
                .unwrap();
        }
        assert_eq!(
            doc.shape(ids[0]).unwrap().transform,
            Transform::translate(3.0, 0.0)
        );
        history.undo(&mut doc).unwrap();
        assert_eq!(doc, original);
    }

    #[test]
    fn plain_apply_never_merges() {
        let (mut doc, red, _, _) = sample();
        let mut history = History::new();
        history
            .apply(&mut doc, Command::SetLayerPasses { id: red, passes: 2 })
            .unwrap();
        history
            .apply_merged(&mut doc, Command::SetLayerPasses { id: red, passes: 3 })
            .unwrap();
        history.undo(&mut doc).unwrap();
        assert_eq!(doc.layer(red).unwrap().passes, 2);
    }

    #[test]
    fn duplicate_ids_in_one_transform_command_undo_cleanly() {
        let (mut doc, _, _, ids) = sample();
        let original = doc.clone();
        let mut history = History::new();
        history
            .apply(
                &mut doc,
                Command::SetTransforms(vec![
                    (ids[0], Transform::translate(1.0, 0.0)),
                    (ids[0], Transform::translate(2.0, 0.0)),
                ]),
            )
            .unwrap();
        assert_eq!(
            doc.shape(ids[0]).unwrap().transform,
            Transform::translate(2.0, 0.0)
        );
        history.undo(&mut doc).unwrap();
        assert_eq!(doc, original);
    }

    #[test]
    fn history_is_capped() {
        let (mut doc, red, _, _) = sample();
        let mut history = History::new();
        for passes in 0..(MAX_UNDO as u32 + 10) {
            history
                .apply(&mut doc, Command::SetLayerPasses { id: red, passes })
                .unwrap();
        }
        let mut undone = 0;
        while history.undo(&mut doc).unwrap().is_some() {
            undone += 1;
        }
        assert_eq!(undone, MAX_UNDO);
        assert_eq!(doc.layer(red).unwrap().passes, 9);
    }

    #[test]
    fn revision_and_labels_track_each_step() {
        let (mut doc, red, _, _) = sample();
        let mut history = History::new();
        assert_eq!(history.undo_label(), None);
        history
            .apply(&mut doc, Command::MoveLayer { id: red, to: 1 })
            .unwrap();
        assert_eq!(history.revision(), 1);
        assert_eq!(history.undo_label(), Some("Reorder layers"));
        assert_eq!(history.undo(&mut doc).unwrap(), Some("Reorder layers"));
        assert_eq!(history.revision(), 2);
        assert_eq!(history.redo_label(), Some("Reorder layers"));
        history.redo(&mut doc).unwrap();
        assert_eq!(history.revision(), 3);
        assert_eq!(
            history.undo(&mut Document::new()),
            Err(HistoryError::OutOfSync)
        );
        assert_eq!(history.revision(), 3);
    }
}
