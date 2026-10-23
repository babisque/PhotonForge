use pf_core::{Document, LayerId, Origin};
use pf_gcode::GcodeError;

use crate::actions;
use crate::state::AppState;

pub fn show_preview(ui: &mut egui::Ui, state: &mut AppState) {
    ui.heading("Preview");
    ui.checkbox(&mut state.show_preview, "Show toolpaths on the canvas");
    ui.add_enabled(
        state.show_preview,
        egui::Checkbox::new(&mut state.show_travels, "Show travel moves"),
    );
    ui.separator();

    actions::refresh_preview(state);
    let ready = match state.preview.result() {
        Some(Ok(preview)) => {
            let estimate = &preview.estimate;
            egui::Grid::new("job_estimate")
                .num_columns(2)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Cut distance");
                    ui.label(format!("{:.1} mm", estimate.cut_mm));
                    ui.end_row();
                    ui.label("Travel distance");
                    ui.label(format!("{:.1} mm", estimate.travel_mm));
                    ui.end_row();
                    ui.label("Estimated time");
                    ui.strong(format_duration(estimate.seconds));
                    ui.end_row();
                });
            ui.weak("Acceleration is not modeled, so the real job takes longer.");
            true
        }
        Some(Err(err)) => {
            let message = describe_gcode_error(err, &state.document);
            ui.colored_label(ui.visuals().error_fg_color, message);
            false
        }
        None => false,
    };

    ui.add_space(8.0);
    let save = ui.add_enabled(
        ready && state.job.is_none(),
        egui::Button::new("Save G-code…"),
    );
    if save.clicked() {
        actions::save_gcode(state);
    }
}

/// A G-code error in the user's terms: layer names instead of IDs.
pub fn describe_gcode_error(err: &GcodeError, doc: &Document) -> String {
    let name = |id: &LayerId| match doc.layer(*id) {
        Some(layer) => format!("\"{}\"", layer.name),
        None => format!("{}", id.0),
    };
    match err {
        GcodeError::OutOfBounds { layer, .. } => {
            format!("Layer {} has shapes outside the work area.", name(layer))
        }
        GcodeError::PowerExceedsLimit { layer, power, max } => format!(
            "Layer {} power {power} % is above the machine maximum of {max} %.",
            name(layer)
        ),
        GcodeError::InvalidPower { layer, power } => {
            format!("Layer {} power {power} % is not valid.", name(layer))
        }
        GcodeError::InvalidSpeed { layer, speed, max } => format!(
            "Layer {} speed {speed} mm/min must be between 1 and {max} mm/min.",
            name(layer)
        ),
        GcodeError::UnsupportedOrigin(origin) => format!(
            "The {} machine origin is not supported yet.",
            origin_name(*origin)
        ),
        GcodeError::NothingToOutput => {
            String::from("Nothing to output: no enabled Line layer has any shapes.")
        }
        GcodeError::InvalidTemplateChar { .. } | GcodeError::Geometry(_) => err.to_string(),
    }
}

fn origin_name(origin: Origin) -> &'static str {
    match origin {
        Origin::BottomLeft => "bottom-left",
        Origin::TopLeft => "top-left",
        Origin::TopRight => "top-right",
        Origin::BottomRight => "bottom-right",
    }
}

/// Rounded to whole seconds, e.g. "≈ 2 min 05 s".
pub fn format_duration(seconds: f64) -> String {
    let total = if seconds.is_finite() && seconds > 0.0 {
        seconds.round() as u64
    } else {
        0
    };
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("≈ {h} h {m:02} min")
    } else if m > 0 {
        format!("≈ {m} min {s:02} s")
    } else {
        format!("≈ {s} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::{Point, Rect};

    #[test]
    fn durations_are_rounded_and_readable() {
        assert_eq!(format_duration(0.4), "≈ 0 s");
        assert_eq!(format_duration(44.6), "≈ 45 s");
        assert_eq!(format_duration(125.0), "≈ 2 min 05 s");
        assert_eq!(format_duration(3_725.0), "≈ 1 h 02 min");
        assert_eq!(format_duration(f64::NAN), "≈ 0 s");
        assert_eq!(format_duration(-3.0), "≈ 0 s");
    }

    #[test]
    fn errors_name_the_layer() {
        let mut doc = Document::new();
        let cut = doc.add_layer("Cut", [255, 0, 0]).unwrap();
        let out = GcodeError::OutOfBounds {
            layer: cut,
            bounds: Rect::new(Point::ORIGIN, Point::new(1.0, 1.0)),
        };
        assert_eq!(
            describe_gcode_error(&out, &doc),
            "Layer \"Cut\" has shapes outside the work area."
        );
        let power = GcodeError::PowerExceedsLimit {
            layer: cut,
            power: 120.0,
            max: 100.0,
        };
        assert_eq!(
            describe_gcode_error(&power, &doc),
            "Layer \"Cut\" power 120 % is above the machine maximum of 100 %."
        );
        let gone = GcodeError::InvalidPower {
            layer: LayerId(9),
            power: f64::NAN,
        };
        assert!(describe_gcode_error(&gone, &doc).starts_with("Layer 9 "));
        assert_eq!(
            describe_gcode_error(&GcodeError::UnsupportedOrigin(Origin::TopLeft), &doc),
            "The top-left machine origin is not supported yet."
        );
    }

    #[test]
    fn panel_renders_headless() {
        let mut state = AppState::new();
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show_preview(ui, &mut state);
            });
            // No renderer here to upload font textures to.
            output.textures_delta.clear();
        }
        assert_eq!(
            state.preview.result(),
            Some(&Err(GcodeError::NothingToOutput))
        );
    }
}
