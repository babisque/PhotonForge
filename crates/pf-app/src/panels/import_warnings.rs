use pf_io::ImportWarning;

use crate::state::AppState;

/// Lists the last import's warnings until the user dismisses them.
pub fn show_import_warnings(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(report) = &state.import_report else {
        return;
    };
    let mut dismissed = false;
    let modal = egui::Modal::new(egui::Id::unique("import_warnings")).show(ui.ctx(), |ui| {
        ui.set_max_width(520.0);
        ui.heading("Import warnings");
        ui.label(format!(
            "{} was imported with {} warning(s):",
            report.file,
            report.warnings.len()
        ));
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                for warning in &report.warnings {
                    ui.label(format!("• {}", describe(warning)));
                }
            });
        ui.add_space(8.0);
        if ui.button("OK").clicked() {
            dismissed = true;
        }
    });
    if dismissed || modal.should_close() {
        state.import_report = None;
    }
}

pub fn describe(warning: &ImportWarning) -> String {
    match warning {
        ImportWarning::SkippedText { id } => format!(
            "{} was skipped: text is not supported yet. Convert it to paths first.",
            element(id)
        ),
        ImportWarning::SkippedImage { id } => format!(
            "{} was skipped: raster images are not supported yet.",
            element(id)
        ),
        ImportWarning::FillOnlyShape { id } => format!(
            "{} has a fill but no stroke, so it went on a Fill layer, which is not output yet.",
            element(id)
        ),
        ImportWarning::UnsupportedPaint { id } => format!(
            "{} uses a gradient or pattern, so it went on a black layer.",
            element(id)
        ),
        ImportWarning::IgnoredClipPath { id } => format!(
            "The clip path on {} was ignored; its contents were imported unclipped.",
            element_in_sentence(id)
        ),
        ImportWarning::IgnoredMask { id } => format!(
            "The mask on {} was ignored; its contents were imported unmasked.",
            element_in_sentence(id)
        ),
        ImportWarning::HiddenShape { id } => format!(
            "{} is hidden or has neither stroke nor fill, so it was skipped.",
            element(id)
        ),
        ImportWarning::NoPhysicalSize { assumed_dpi } => format!(
            "The file has no physical size, so {assumed_dpi} DPI was assumed. Check the imported size."
        ),
        ImportWarning::InvalidGeometry { id, reason } => format!(
            "{} has geometry that cannot be used ({reason}), so it was skipped.",
            element(id)
        ),
    }
}

fn element(id: &Option<String>) -> String {
    match id {
        Some(id) => format!("Element \"{id}\""),
        None => String::from("An element"),
    }
}

fn element_in_sentence(id: &Option<String>) -> String {
    match id {
        Some(id) => format!("element \"{id}\""),
        None => String::from("an element"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_name_the_element_when_it_has_an_id() {
        let named = ImportWarning::SkippedText {
            id: Some("title".into()),
        };
        assert_eq!(
            describe(&named),
            "Element \"title\" was skipped: text is not supported yet. Convert it to paths first."
        );
        let clipped = ImportWarning::IgnoredClipPath {
            id: Some("Logo".into()),
        };
        assert_eq!(
            describe(&clipped),
            "The clip path on element \"Logo\" was ignored; its contents were imported unclipped."
        );
        let anonymous = ImportWarning::IgnoredMask { id: None };
        assert_eq!(
            describe(&anonymous),
            "The mask on an element was ignored; its contents were imported unmasked."
        );
    }

    #[test]
    fn dpi_and_reason_are_included() {
        let dpi = ImportWarning::NoPhysicalSize { assumed_dpi: 96.0 };
        assert!(describe(&dpi).contains("96 DPI"));
        let invalid = ImportWarning::InvalidGeometry {
            id: None,
            reason: "NaN coordinate".into(),
        };
        assert!(describe(&invalid).contains("(NaN coordinate)"));
    }
}
