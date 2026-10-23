//! PhotonForge desktop application: a thin eframe GUI over the library crates.

mod actions;
mod app;
mod display;
mod panels;
mod state;

fn main() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PhotonForge")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "PhotonForge",
        options,
        Box::new(|_cc| Ok(Box::new(app::PhotonForgeApp::default()))),
    )
    .map_err(|e| anyhow::anyhow!("failed to start GUI: {e}"))
}
