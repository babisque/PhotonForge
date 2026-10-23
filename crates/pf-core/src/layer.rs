/// Identifies a layer within a `Document`. Issued by the document and never
/// reused; it carries no meaning beyond identity (it is not a color or an
/// index into the layer order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayerId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayerMode {
    #[default]
    Line,
    Fill,
    Image,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PowerMode {
    /// `M4`: power scales with actual speed and is off while stationary.
    /// The safer default for GRBL laser mode.
    #[default]
    M4Dynamic,
    /// `M3`: constant power regardless of speed.
    M3Constant,
}

pub const DEFAULT_SPEED_MM_MIN: f64 = 1000.0;
/// Default power for new layers (% of the machine maximum). Deliberately low.
pub const DEFAULT_POWER_PERCENT: f64 = 10.0;
pub const DEFAULT_PASSES: u32 = 1;

pub const PALETTE: [[u8; 3]; 12] = [
    [0, 0, 0],
    [0, 0, 255],
    [255, 0, 0],
    [0, 170, 0],
    [255, 140, 0],
    [170, 0, 170],
    [0, 170, 170],
    [200, 170, 0],
    [128, 128, 128],
    [120, 60, 20],
    [255, 80, 160],
    [60, 60, 140],
];

pub fn palette_color(index: usize) -> [u8; 3] {
    PALETTE[index % PALETTE.len()]
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub color: [u8; 3], // RGB, display only
    pub mode: LayerMode,
    pub power_mode: PowerMode,
    pub speed_mm_min: f64,  // mm/min
    pub power_percent: f64, // % of the machine maximum; clamped by MachineProfile
    pub passes: u32,
    pub output_enabled: bool,
}

impl Layer {
    pub fn new(id: LayerId, name: impl Into<String>, color: [u8; 3]) -> Layer {
        Layer {
            id,
            name: name.into(),
            color,
            mode: LayerMode::default(),
            power_mode: PowerMode::default(),
            speed_mm_min: DEFAULT_SPEED_MM_MIN,
            power_percent: DEFAULT_POWER_PERCENT,
            passes: DEFAULT_PASSES,
            output_enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{EPS, approx_eq};

    #[test]
    fn new_layer_has_conservative_defaults() {
        let layer = Layer::new(LayerId(7), "Cut", [255, 0, 0]);
        assert_eq!(layer.id, LayerId(7));
        assert_eq!(layer.name, "Cut");
        assert_eq!(layer.color, [255, 0, 0]);
        assert_eq!(layer.mode, LayerMode::Line);
        assert_eq!(layer.power_mode, PowerMode::M4Dynamic);
        assert!(approx_eq(layer.speed_mm_min, 1000.0, EPS));
        assert!(approx_eq(layer.power_percent, 10.0, EPS));
        assert_eq!(layer.passes, 1);
        assert!(layer.output_enabled);
    }

    #[test]
    fn enum_defaults() {
        assert_eq!(LayerMode::default(), LayerMode::Line);
        assert_eq!(PowerMode::default(), PowerMode::M4Dynamic);
    }

    #[test]
    fn palette_cycles() {
        assert_eq!(palette_color(0), PALETTE[0]);
        assert_eq!(palette_color(1), PALETTE[1]);
        assert_eq!(palette_color(PALETTE.len()), PALETTE[0]);
        assert_eq!(palette_color(PALETTE.len() + 2), PALETTE[2]);
    }

    #[test]
    fn palette_colors_are_distinct() {
        for (i, a) in PALETTE.iter().enumerate() {
            for b in &PALETTE[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
