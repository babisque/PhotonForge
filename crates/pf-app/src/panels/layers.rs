use std::collections::HashMap;
use std::ops::RangeInclusive;

use egui::emath::Numeric;
use pf_core::{Command, Layer, LayerId, LayerMode, MachineProfile, PowerMode};

use crate::actions;
use crate::state::AppState;

const MAX_PASSES: u32 = 100;
const SWATCH_PX: f32 = 14.0;
const MODES: [LayerMode; 3] = [LayerMode::Line, LayerMode::Fill, LayerMode::Image];

/// A change requested while drawing the panel. Edits are applied after the
/// frame's widgets so the layer list is not borrowed while it changes.
#[derive(Debug, Clone, PartialEq)]
enum Edit {
    Apply(Command),
    /// Part of a continuous gesture: dragging a value or typing a name.
    Merge(Command),
    /// The gesture ended; the next change starts a new undo step.
    Seal,
    Select(LayerId),
    AssignSelection(LayerId),
}

pub fn show_layers(ui: &mut egui::Ui, state: &mut AppState) {
    ui.heading("Layers");
    let Some(current) = current_layer(state) else {
        ui.weak("No layers yet. Import an SVG to get started.");
        return;
    };
    let mut edits = Vec::new();
    layer_table(ui, state, current, &mut edits);
    ui.separator();
    if let Some(layer) = state.document.layer(current) {
        let has_selection = !state.selection.is_empty();
        layer_editor(ui, layer, &state.profile, has_selection, &mut edits);
    }
    for edit in edits {
        run(state, edit);
    }
}

/// The layer shown in the editor: the chosen one if it still exists,
/// otherwise the first layer.
fn current_layer(state: &AppState) -> Option<LayerId> {
    let doc = &state.document;
    state
        .selected_layer
        .filter(|id| doc.layer(*id).is_some())
        .or_else(|| doc.layers().first().map(|l| l.id))
}

fn layer_table(ui: &mut egui::Ui, state: &AppState, current: LayerId, edits: &mut Vec<Edit>) {
    let mut counts: HashMap<LayerId, usize> = HashMap::new();
    for shape in state.document.shapes() {
        *counts.entry(shape.layer_id).or_default() += 1;
    }
    let layers = state.document.layers();
    let last = layers.len().saturating_sub(1);

    egui::Grid::new("layer_table")
        .num_columns(6)
        .striped(true)
        .show(ui, |ui| {
            for (index, layer) in layers.iter().enumerate() {
                swatch(ui, layer.color);
                let label = format!(
                    "{} ({})",
                    layer.name,
                    counts.get(&layer.id).copied().unwrap_or(0)
                );
                if ui.selectable_label(layer.id == current, label).clicked() {
                    edits.push(Edit::Select(layer.id));
                }
                ui.label(mode_name(layer.mode));
                ui.label(settings_summary(layer));

                let mut output = layer.output_enabled;
                if ui
                    .checkbox(&mut output, "")
                    .on_hover_text("Include this layer in the output")
                    .changed()
                {
                    edits.push(Edit::Apply(Command::SetLayerOutput {
                        id: layer.id,
                        enabled: output,
                    }));
                }

                ui.horizontal(|ui| {
                    let up = ui.add_enabled(index > 0, egui::Button::new("▲").small());
                    if up.on_hover_text("Cut earlier").clicked() {
                        edits.push(Edit::Apply(Command::MoveLayer {
                            id: layer.id,
                            to: index - 1,
                        }));
                    }
                    let down = ui.add_enabled(index < last, egui::Button::new("▼").small());
                    if down.on_hover_text("Cut later").clicked() {
                        edits.push(Edit::Apply(Command::MoveLayer {
                            id: layer.id,
                            to: index + 1,
                        }));
                    }
                });
                ui.end_row();
            }
        });
}

fn layer_editor(
    ui: &mut egui::Ui,
    layer: &Layer,
    profile: &MachineProfile,
    has_selection: bool,
    edits: &mut Vec<Edit>,
) {
    let id = layer.id;
    egui::Grid::new("layer_editor")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Name");
            let mut name = layer.name.clone();
            let response = ui.text_edit_singleline(&mut name);
            if response.changed() {
                edits.push(Edit::Merge(Command::SetLayerName { id, name }));
            }
            if response.lost_focus() {
                edits.push(Edit::Seal);
            }
            ui.end_row();

            ui.label("Mode");
            let mut mode = layer.mode;
            egui::ComboBox::from_id_salt("layer_mode")
                .selected_text(mode_name(mode))
                .show_ui(ui, |ui| {
                    for option in MODES {
                        ui.selectable_value(&mut mode, option, mode_name(option));
                    }
                });
            if mode != layer.mode {
                edits.push(Edit::Apply(Command::SetLayerMode { id, mode }));
            }
            ui.end_row();

            ui.label("Speed");
            edit_number(
                ui,
                edits,
                layer.speed_mm_min,
                1.0..=profile.max_feed_mm_min(),
                " mm/min",
                10.0,
                |speed| Command::SetLayerSpeed { id, speed },
            );
            ui.end_row();

            ui.label("Power");
            edit_number(
                ui,
                edits,
                layer.power_percent,
                0.0..=profile.max_power_percent(),
                " %",
                0.1,
                |power| Command::SetLayerPower { id, power },
            );
            ui.end_row();

            ui.label("Passes");
            edit_number(
                ui,
                edits,
                layer.passes,
                1..=MAX_PASSES,
                "",
                0.05,
                |passes| Command::SetLayerPasses { id, passes },
            );
            ui.end_row();

            ui.label("Output");
            let mut output = layer.output_enabled;
            if ui.checkbox(&mut output, "Include in output").changed() {
                edits.push(Edit::Apply(Command::SetLayerOutput {
                    id,
                    enabled: output,
                }));
            }
            ui.end_row();

            ui.label("Laser mode");
            let mut power_mode = layer.power_mode;
            ui.vertical(|ui| {
                ui.radio_value(&mut power_mode, PowerMode::M4Dynamic, "M4 dynamic")
                    .on_hover_text(
                        "Power follows the actual speed and is off while stopped. \
                         The safer choice for GRBL laser mode.",
                    );
                ui.radio_value(&mut power_mode, PowerMode::M3Constant, "M3 constant")
                    .on_hover_text(
                        "Constant power whatever the speed, so corners and \
                         slow-downs burn deeper.",
                    );
            });
            if power_mode != layer.power_mode {
                edits.push(Edit::Apply(Command::SetLayerPowerMode {
                    id,
                    mode: power_mode,
                }));
            }
            ui.end_row();
        });

    for warning in limit_warnings(layer, profile) {
        ui.colored_label(ui.visuals().error_fg_color, warning);
    }
    if layer.mode != LayerMode::Line {
        ui.weak("Fill and Image layers are not output yet.");
    }
    ui.add_space(6.0);
    let assign = ui.add_enabled(
        has_selection,
        egui::Button::new("Move selected shapes to this layer"),
    );
    if assign.clicked() {
        edits.push(Edit::AssignSelection(id));
    }
}

/// A `DragValue` that records one undo step per drag. Values outside
/// `range` are shown as they are, never clamped silently.
fn edit_number<N: Numeric>(
    ui: &mut egui::Ui,
    edits: &mut Vec<Edit>,
    value: N,
    range: RangeInclusive<N>,
    suffix: &str,
    speed: f64,
    command: impl Fn(N) -> Command,
) {
    let mut edited = value;
    let response = ui.add(
        egui::DragValue::new(&mut edited)
            .range(range)
            .clamp_existing_to_range(false)
            .suffix(suffix)
            .speed(speed),
    );
    if response.changed() && edited != value {
        edits.push(if response.dragged() {
            Edit::Merge(command(edited))
        } else {
            Edit::Apply(command(edited))
        });
    }
    if response.drag_stopped() {
        edits.push(Edit::Seal);
    }
}

fn run(state: &mut AppState, edit: Edit) {
    match edit {
        Edit::Apply(command) => actions::apply(state, command),
        Edit::Merge(command) => actions::apply_merged(state, command),
        Edit::Seal => state.history.seal(),
        Edit::Select(id) => state.selected_layer = Some(id),
        Edit::AssignSelection(id) => actions::assign_selection(state, id),
    }
}

fn swatch(ui: &mut egui::Ui, [r, g, b]: [u8; 3]) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(SWATCH_PX), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 2.0, egui::Color32::from_rgb(r, g, b));
}

fn mode_name(mode: LayerMode) -> &'static str {
    match mode {
        LayerMode::Line => "Line",
        LayerMode::Fill => "Fill",
        LayerMode::Image => "Image",
    }
}

fn settings_summary(layer: &Layer) -> String {
    let passes = if layer.passes == 1 {
        String::new()
    } else {
        format!(" · ×{}", layer.passes)
    };
    format!(
        "{} mm/min · {} %{passes}",
        number(layer.speed_mm_min),
        number(layer.power_percent)
    )
}

/// Whole numbers without decimals, others with one.
fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

/// Settings above the machine profile's limits. Export refuses these
/// layers (`pf_gcode::validate`); the panel only points them out.
fn limit_warnings(layer: &Layer, profile: &MachineProfile) -> Vec<String> {
    let mut warnings = Vec::new();
    if layer.speed_mm_min > profile.max_feed_mm_min() {
        warnings.push(format!(
            "Speed is above the machine maximum of {} mm/min.",
            number(profile.max_feed_mm_min())
        ));
    }
    if layer.power_percent > profile.max_power_percent() {
        warnings.push(format!(
            "Power is above the machine maximum of {} %.",
            number(profile.max_power_percent())
        ));
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::{Point, Rect};

    fn layer() -> Layer {
        Layer::new(LayerId(3), "Cut", [255, 0, 0])
    }

    fn limited_profile() -> MachineProfile {
        MachineProfile::new(
            Rect::new(Point::ORIGIN, Point::new(300.0, 200.0)),
            pf_core::Origin::BottomLeft,
            1000,
            3000.0,
            50.0,
            0.0,
        )
        .unwrap()
    }

    fn state_with_layers() -> (AppState, LayerId, LayerId) {
        let mut state = AppState::new();
        let a = state.document.add_layer("Red", [255, 0, 0]).unwrap();
        let b = state.document.add_layer("Blue", [0, 0, 255]).unwrap();
        (state, a, b)
    }

    #[test]
    fn summary_shows_speed_power_and_extra_passes() {
        let mut l = layer();
        assert_eq!(settings_summary(&l), "1000 mm/min · 10 %");
        l.power_percent = 12.5;
        l.passes = 3;
        assert_eq!(settings_summary(&l), "1000 mm/min · 12.5 % · ×3");
    }

    #[test]
    fn limits_are_pointed_out_not_clamped() {
        let mut l = layer();
        assert!(limit_warnings(&l, &limited_profile()).is_empty());
        l.speed_mm_min = 4000.0;
        l.power_percent = 80.0;
        let warnings = limit_warnings(&l, &limited_profile());
        assert_eq!(
            warnings,
            vec![
                "Speed is above the machine maximum of 3000 mm/min.",
                "Power is above the machine maximum of 50 %.",
            ]
        );
        assert_eq!(l.power_percent, 80.0);
    }

    #[test]
    fn current_layer_falls_back_to_the_first() {
        let (mut state, a, b) = state_with_layers();
        assert_eq!(current_layer(&state), Some(a));
        state.selected_layer = Some(b);
        assert_eq!(current_layer(&state), Some(b));
        state.selected_layer = Some(LayerId(99));
        assert_eq!(current_layer(&state), Some(a));
        assert_eq!(current_layer(&AppState::new()), None);
    }

    #[test]
    fn merged_edits_and_seal_make_one_undo_step_per_gesture() {
        let (mut state, a, _) = state_with_layers();
        for speed in [1100.0, 1200.0, 1300.0] {
            run(
                &mut state,
                Edit::Merge(Command::SetLayerSpeed { id: a, speed }),
            );
        }
        run(&mut state, Edit::Seal);
        run(
            &mut state,
            Edit::Merge(Command::SetLayerSpeed {
                id: a,
                speed: 1400.0,
            }),
        );
        actions::undo(&mut state);
        assert_eq!(state.document.layer(a).unwrap().speed_mm_min, 1300.0);
        actions::undo(&mut state);
        assert_eq!(state.document.layer(a).unwrap().speed_mm_min, 1000.0);
    }

    #[test]
    fn select_and_assign_route_to_state_and_actions() {
        let (mut state, a, b) = state_with_layers();
        let shape = state
            .document
            .add_shape(
                pf_core::ShapeKind::Rect(pf_core::RectShape::new(1.0, 1.0, 0.0).unwrap()),
                pf_core::Transform::identity(),
                a,
            )
            .unwrap();
        run(&mut state, Edit::Select(b));
        assert_eq!(state.selected_layer, Some(b));
        state.selection = vec![shape];
        run(&mut state, Edit::AssignSelection(b));
        assert_eq!(state.document.shape(shape).unwrap().layer_id, b);
        assert_eq!(state.history.undo_label(), Some("Assign to layer"));
    }

    #[test]
    fn panel_renders_headless() {
        let (mut state, _, _) = state_with_layers();
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show_layers(ui, &mut state);
            });
            // No renderer here to upload font textures to.
            output.textures_delta.clear();
        }
        // Drawing alone never edits the document.
        assert!(!state.history.can_undo());
    }
}
