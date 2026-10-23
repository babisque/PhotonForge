use egui::DragValue;
use pf_core::{MachineProfile, Origin};

use crate::actions;
use crate::state::{AppState, ProfileDraft};

const ORIGINS: [Origin; 4] = [
    Origin::BottomLeft,
    Origin::TopLeft,
    Origin::TopRight,
    Origin::BottomRight,
];

/// Edits a draft of the machine profile. Nothing changes until "Apply",
/// and `MachineProfile::new` refuses anything outside the safety rules.
pub fn show_machine(ui: &mut egui::Ui, state: &mut AppState) {
    ui.heading("Machine");
    let draft = &mut state.profile_draft;
    egui::Grid::new("machine_profile")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Work area width");
            ui.add(mm(&mut draft.width_mm));
            ui.end_row();
            ui.label("Work area height");
            ui.add(mm(&mut draft.height_mm));
            ui.end_row();

            ui.label("Origin");
            egui::ComboBox::from_id_salt("machine_origin")
                .selected_text(origin_name(draft.origin))
                .show_ui(ui, |ui| {
                    for origin in ORIGINS {
                        let supported = origin == Origin::BottomLeft;
                        ui.add_enabled_ui(supported, |ui| {
                            ui.selectable_value(&mut draft.origin, origin, origin_name(origin))
                                .on_disabled_hover_text("Not supported by the G-code output yet");
                        });
                    }
                });
            ui.end_row();

            ui.label("Max S value ($30)");
            ui.add(
                DragValue::new(&mut draft.max_s_value)
                    .range(1..=u32::MAX)
                    .speed(1.0),
            )
            .on_hover_text("The S value GRBL treats as 100 % power");
            ui.end_row();

            ui.label("Max feed");
            ui.add(
                DragValue::new(&mut draft.max_feed_mm_min)
                    .range(1.0..=f64::INFINITY)
                    .clamp_existing_to_range(false)
                    .suffix(" mm/min")
                    .speed(10.0),
            );
            ui.end_row();

            ui.label("Max power");
            ui.add(percent(&mut draft.max_power_percent, 100.0))
                .on_hover_text("No layer can be output above this power");
            ui.end_row();

            let frame_max = draft.max_power_percent;
            ui.label("Frame power");
            ui.add(percent(&mut draft.frame_power_percent, frame_max))
                .on_hover_text("Power used when framing the job; 0 keeps the laser off");
            ui.end_row();
        });

    if let Some(error) = &state.profile_error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    let changed = state.profile_draft != ProfileDraft::from_profile(&state.profile);
    if changed {
        ui.weak("Unapplied changes.");
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(changed, egui::Button::new("Apply"))
            .clicked()
        {
            actions::apply_profile_draft(state);
        }
        if ui
            .add_enabled(changed, egui::Button::new("Revert"))
            .clicked()
        {
            state.profile_draft = ProfileDraft::from_profile(&state.profile);
            state.profile_error = None;
        }
        if ui.button("Reset to defaults").clicked() {
            state.profile_draft = ProfileDraft::from_profile(&MachineProfile::default());
            state.profile_error = None;
        }
    });
    ui.weak("The profile lasts for this session only.");
}

fn mm(value: &mut f64) -> DragValue<'_> {
    DragValue::new(value)
        .range(1.0..=f64::INFINITY)
        .clamp_existing_to_range(false)
        .suffix(" mm")
        .speed(1.0)
}

fn percent(value: &mut f64, max: f64) -> DragValue<'_> {
    DragValue::new(value)
        .range(0.0..=max)
        .clamp_existing_to_range(false)
        .suffix(" %")
        .speed(0.1)
}

fn origin_name(origin: Origin) -> &'static str {
    match origin {
        Origin::BottomLeft => "Bottom left",
        Origin::TopLeft => "Top left",
        Origin::TopRight => "Top right",
        Origin::BottomRight => "Bottom right",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_renders_headless_without_touching_the_profile() {
        let mut state = AppState::new();
        state.profile_error = Some("invalid max power 150%".into());
        let before = state.profile.clone();
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show_machine(ui, &mut state);
            });
            // No renderer here to upload font textures to.
            output.textures_delta.clear();
        }
        assert_eq!(state.profile, before);
        assert_eq!(state.profile_draft, ProfileDraft::from_profile(&before));
    }

    #[test]
    fn origins_have_names() {
        assert_eq!(origin_name(Origin::BottomLeft), "Bottom left");
        assert_eq!(origin_name(Origin::TopRight), "Top right");
    }
}
