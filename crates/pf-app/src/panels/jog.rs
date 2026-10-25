use egui::DragValue;
use pf_core::MachineProfile;
use pf_device::{ControlRequest, JogMove};

use crate::actions;
use crate::state::{AppState, JogUi};

pub const STEPS_MM: [f64; 4] = [0.1, 1.0, 10.0, 50.0];

/// Rows of the arrow pad; `None` is an empty cell.
const PAD: [[Option<[i8; 2]>; 3]; 3] = [
    [None, Some([0, 1]), None],
    [Some([-1, 0]), None, Some([1, 0])],
    [None, Some([0, -1]), None],
];

/// Arrow pad for jogging. Step mode sends one clamped move per click;
/// continuous mode moves toward the edge while a button is held and cancels
/// on release or when the window loses focus.
pub fn show_jog(ui: &mut egui::Ui, state: &mut AppState) {
    let allowed = state.device.snapshot.can_jog();
    let profile = &state.profile;
    let jog = &mut state.device.jog;
    let mut wanted = Vec::new();
    egui::CollapsingHeader::new("Jog")
        .id_salt("device_jog")
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Step");
                for step in STEPS_MM {
                    ui.selectable_value(&mut jog.step_mm, step, format!("{step} mm"));
                }
            });
            ui.horizontal(|ui| {
                ui.label("Feed");
                ui.add(
                    DragValue::new(&mut jog.feed_mm_min)
                        .range(1.0..=profile.max_feed_mm_min())
                        .suffix(" mm/min")
                        .speed(10.0),
                );
            });
            ui.checkbox(&mut jog.continuous, "Continuous (moves while held)");
            if let Err(reason) = &allowed {
                ui.weak(reason);
            }
            egui::Grid::new("jog_pad").show(ui, |ui| {
                for row in PAD {
                    for cell in row {
                        match cell {
                            Some(direction) => {
                                jog_button(
                                    ui,
                                    direction,
                                    allowed.is_ok(),
                                    jog,
                                    profile,
                                    &mut wanted,
                                );
                            }
                            None => {
                                ui.label("");
                            }
                        }
                    }
                    ui.end_row();
                }
            });
        });
    let (pointer_down, focused) = ui.input(|i| (i.pointer.primary_down(), i.focused));
    if release_ends_jog(jog, pointer_down, focused) {
        wanted.push(ControlRequest::JogCancel);
    }
    for control in wanted {
        actions::device_control(state, control);
    }
}

fn jog_button(
    ui: &mut egui::Ui,
    direction: [i8; 2],
    enabled: bool,
    jog: &mut JogUi,
    profile: &MachineProfile,
    wanted: &mut Vec<ControlRequest>,
) {
    let button = egui::Button::new(arrow(direction))
        .min_size(egui::vec2(36.0, 36.0))
        .sense(egui::Sense::click_and_drag());
    let response = ui.add_enabled(enabled, button);
    let step = if jog.continuous {
        if !response.is_pointer_button_down_on() || jog.held.is_some() {
            return;
        }
        jog.held = Some(direction);
        JogMove::to_edge(direction, jog.feed_mm_min, profile)
    } else {
        if !response.clicked() {
            return;
        }
        JogMove::step(direction, jog.step_mm, jog.feed_mm_min)
    };
    wanted.push(ControlRequest::Jog {
        step,
        profile: profile.clone(),
    });
}

/// A held continuous jog ends when the button is released or the window
/// loses focus (it would never see the release). True when it just ended.
fn release_ends_jog(jog: &mut JogUi, pointer_down: bool, focused: bool) -> bool {
    if jog.held.is_some() && !(pointer_down && focused) {
        jog.held = None;
        return true;
    }
    false
}

fn arrow(direction: [i8; 2]) -> &'static str {
    match direction {
        [0, 1] => "⬆ Y+",
        [0, -1] => "⬇ Y−",
        [-1, 0] => "⬅ X−",
        _ => "➡ X+",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_jog_is_cancelled_on_release_or_focus_loss() {
        let mut jog = JogUi {
            held: Some([1, 0]),
            ..JogUi::default()
        };
        assert!(!release_ends_jog(&mut jog, true, true));
        assert!(release_ends_jog(&mut jog, true, false));
        assert_eq!(jog.held, None);
        // Nothing to cancel twice.
        assert!(!release_ends_jog(&mut jog, false, true));
        jog.held = Some([0, -1]);
        assert!(release_ends_jog(&mut jog, false, true));
    }

    #[test]
    fn pad_renders_headless_and_sends_nothing_by_itself() {
        let mut state = AppState::new();
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show_jog(ui, &mut state);
            });
            output.textures_delta.clear();
        }
        assert_eq!(state.device.jog.held, None);
        assert_eq!(arrow([-1, 0]), "⬅ X−");
    }
}
