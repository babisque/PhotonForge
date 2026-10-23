//! PhotonForge desktop application: a thin eframe GUI over the library crates.

mod app;

fn main() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("PhotonForge"),
        ..Default::default()
    };
    eframe::run_native(
        "PhotonForge",
        options,
        Box::new(|_cc| Ok(Box::new(app::PhotonForgeApp::default()))),
    )
    .map_err(|e| anyhow::anyhow!("failed to start GUI: {e}"))
}
