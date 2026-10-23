//! Top-level application state and the root UI.

/// Root application state. Empty until the GUI MVP (Phase 4).
#[derive(Debug, Default)]
pub struct PhotonForgeApp {}

impl eframe::App for PhotonForgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |_ui| {});
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_constructs_with_default_state() {
        let _app = PhotonForgeApp::default();
    }
}
