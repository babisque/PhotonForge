use pf_core::{Document, DocumentError, LayerId, LayerMode};

/// Creates one layer per distinct (color, mode) pair, in the order the pairs
/// are first seen, which also fixes the cut order.
#[derive(Debug, Default)]
pub(crate) struct ColorLayers {
    layers: Vec<([u8; 3], LayerMode, LayerId)>,
}

impl ColorLayers {
    pub(crate) fn layer_for(
        &mut self,
        doc: &mut Document,
        color: [u8; 3],
        mode: LayerMode,
    ) -> Result<LayerId, DocumentError> {
        let existing = self
            .layers
            .iter()
            .find(|(c, m, _)| *c == color && *m == mode);
        if let Some(&(_, _, id)) = existing {
            return Ok(id);
        }
        let id = doc.add_layer(layer_name(color, mode), color)?;
        doc.update_layer(id, |layer| layer.mode = mode)?;
        self.layers.push((color, mode, id));
        Ok(id)
    }
}

fn layer_name(color: [u8; 3], mode: LayerMode) -> String {
    let [r, g, b] = color;
    let hex = format!("#{r:02X}{g:02X}{b:02X}");
    match mode {
        LayerMode::Line => hex,
        LayerMode::Fill => format!("{hex} fill"),
        LayerMode::Image => format!("{hex} image"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_color_and_mode_reuses_the_layer() {
        let mut doc = Document::new();
        let mut layers = ColorLayers::default();
        let a = layers
            .layer_for(&mut doc, [255, 0, 0], LayerMode::Line)
            .unwrap();
        let b = layers
            .layer_for(&mut doc, [255, 0, 0], LayerMode::Line)
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(doc.layers().len(), 1);
    }

    #[test]
    fn layers_follow_first_seen_order() {
        let mut doc = Document::new();
        let mut layers = ColorLayers::default();
        let blue = layers
            .layer_for(&mut doc, [0, 0, 255], LayerMode::Line)
            .unwrap();
        let red = layers
            .layer_for(&mut doc, [255, 0, 0], LayerMode::Line)
            .unwrap();
        let blue_fill = layers
            .layer_for(&mut doc, [0, 0, 255], LayerMode::Fill)
            .unwrap();
        let ids: Vec<LayerId> = doc.layers().iter().map(|l| l.id).collect();
        assert_eq!(ids, vec![blue, red, blue_fill]);

        let names: Vec<&str> = doc.layers().iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["#0000FF", "#FF0000", "#0000FF fill"]);
        assert_eq!(doc.layer(blue_fill).unwrap().mode, LayerMode::Fill);
        assert_eq!(doc.layer(red).unwrap().color, [255, 0, 0]);
    }

    #[test]
    fn near_identical_colors_stay_separate() {
        let mut doc = Document::new();
        let mut layers = ColorLayers::default();
        let a = layers
            .layer_for(&mut doc, [255, 0, 0], LayerMode::Line)
            .unwrap();
        let b = layers
            .layer_for(&mut doc, [254, 0, 0], LayerMode::Line)
            .unwrap();
        assert_ne!(a, b);
    }
}
