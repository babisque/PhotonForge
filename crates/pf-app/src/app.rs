//! Top-level application state and the root UI layout.

use std::time::Duration;

use egui::{Key, KeyboardShortcut, Modifiers};

use crate::actions;
use crate::panels::{canvas, connection, console, import_warnings, layers, machine, preview};
use crate::state::{AppState, SideTab, StatusLevel};

const IMPORT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::I);
const SAVE_GCODE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::E);
const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
const REDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
const REDO_SHIFT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);

#[derive(Debug)]
pub struct PhotonForgeApp {
    state: AppState,
}

impl Default for PhotonForgeApp {
    fn default() -> Self {
        PhotonForgeApp {
            state: AppState::new(),
        }
    }
}

impl eframe::App for PhotonForgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        actions::poll_jobs(&mut self.state);
        actions::poll_device(&mut self.state);
        if self.state.job.is_some() || self.state.device.connection.is_some() {
            ui.ctx().request_repaint_after(JOB_POLL);
        }
        handle_shortcuts(ui, &mut self.state);
        egui::Panel::top("menu_bar").show(ui, |ui| show_menu_bar(ui, &mut self.state));
        egui::Panel::bottom("status_bar").show(ui, |ui| show_status_bar(ui, &self.state));
        if self.state.device.show_console {
            egui::Panel::bottom("console")
                .resizable(true)
                .default_size(180.0)
                .show(ui, |ui| console::show_console(ui, &mut self.state));
        }
        egui::Panel::right("side_panel")
            .resizable(true)
            .default_size(300.0)
            .show(ui, |ui| show_side_panel(ui, &mut self.state));
        egui::CentralPanel::default().show(ui, |ui| canvas::show_canvas(ui, &mut self.state));
        import_warnings::show_import_warnings(ui, &mut self.state);
    }
}

fn show_menu_bar(ui: &mut egui::Ui, state: &mut AppState) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            let import = ui.add_enabled(
                state.job.is_none(),
                egui::Button::new("Import SVG…").shortcut_text(ui.ctx().format_shortcut(&IMPORT)),
            );
            if import.clicked() {
                actions::import_svg(state);
            }
            let save = ui.add_enabled(
                state.job.is_none(),
                egui::Button::new("Save G-code…")
                    .shortcut_text(ui.ctx().format_shortcut(&SAVE_GCODE)),
            );
            if save.clicked() {
                actions::save_gcode(state);
            }
            ui.separator();
            if ui.button("Quit").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        ui.menu_button("Edit", |ui| {
            let undo = ui.add_enabled(
                state.history.can_undo(),
                egui::Button::new(edit_label("Undo", state.history.undo_label()))
                    .shortcut_text(ui.ctx().format_shortcut(&UNDO)),
            );
            if undo.clicked() {
                actions::undo(state);
            }
            let redo = ui.add_enabled(
                state.history.can_redo(),
                egui::Button::new(edit_label("Redo", state.history.redo_label()))
                    .shortcut_text(ui.ctx().format_shortcut(&REDO)),
            );
            if redo.clicked() {
                actions::redo(state);
            }
        });
        ui.menu_button("View", |ui| {
            if ui.button("Fit work area (F)").clicked() {
                state.needs_fit = true;
            }
            ui.checkbox(&mut state.show_frame_time, "Show frame time");
            ui.checkbox(&mut state.device.show_console, "Console");
        });
    });
}

/// "GRBL: Idle" while connected, for the status bar.
fn device_summary(snapshot: &pf_device::DeviceSnapshot) -> Option<String> {
    use pf_device::LinkState;
    match &snapshot.link {
        LinkState::Disconnected => None,
        LinkState::Opening | LinkState::Identifying => Some(String::from("GRBL: connecting…")),
        LinkState::Ready { .. } => Some(match &snapshot.status {
            Some(status) => format!("GRBL: {}", connection::state_name(status.state)),
            None => String::from("GRBL: connected"),
        }),
    }
}

fn edit_label(verb: &str, step: Option<&str>) -> String {
    match step {
        Some(step) => format!("{verb} {step}"),
        None => verb.to_owned(),
    }
}

/// How often to check on a background job while it runs.
const JOB_POLL: Duration = Duration::from_millis(50);

/// Text fields keep their own undo, so document shortcuts only fire when
/// no widget has keyboard focus.
fn handle_shortcuts(ui: &mut egui::Ui, state: &mut AppState) {
    if ui.memory(|m| m.focused().is_some()) {
        return;
    }
    // Most specific first: a plain Ctrl+Z also matches Ctrl+Shift+Z.
    let (save, import, redo_shift, redo, undo) = ui.input_mut(|i| {
        (
            i.consume_shortcut(&SAVE_GCODE),
            i.consume_shortcut(&IMPORT),
            i.consume_shortcut(&REDO_SHIFT),
            i.consume_shortcut(&REDO),
            i.consume_shortcut(&UNDO),
        )
    });
    if save {
        actions::save_gcode(state);
    } else if import {
        actions::import_svg(state);
    } else if redo_shift || redo {
        actions::redo(state);
    } else if undo {
        actions::undo(state);
    }
}

fn show_status_bar(ui: &mut egui::Ui, state: &AppState) {
    let area = state.profile.work_area();
    ui.horizontal(|ui| {
        if state.job.is_some() {
            ui.spinner();
        }
        if let Some(status) = &state.status {
            let color = match status.level {
                StatusLevel::Info => ui.visuals().text_color(),
                StatusLevel::Warning => ui.visuals().warn_fg_color,
                StatusLevel::Error => ui.visuals().error_fg_color,
            };
            ui.colored_label(color, &status.text);
            ui.separator();
        }
        ui.label(format!(
            "{} layer(s) · {} shape(s) · {} selected · work area {} × {} mm",
            state.document.layers().len(),
            state.document.shapes().len(),
            state.selection.len(),
            area.width(),
            area.height()
        ));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(format!("{:.0}%", state.viewport.zoom() * 100.0));
            if let Some(device) = device_summary(&state.device.snapshot) {
                ui.separator();
                ui.label(device);
            }
            if let Some(p) = state.cursor_mm {
                ui.monospace(format!("X {:8.2}  Y {:8.2} mm", p.x, p.y));
            }
        });
    });
}

fn show_side_panel(ui: &mut egui::Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.side_tab, SideTab::Layers, "Layers");
        ui.selectable_value(&mut state.side_tab, SideTab::Machine, "Machine");
        ui.selectable_value(&mut state.side_tab, SideTab::Preview, "Preview");
        ui.selectable_value(&mut state.side_tab, SideTab::Device, "Device");
    });
    ui.separator();
    match state.side_tab {
        SideTab::Layers => layers::show_layers(ui, state),
        SideTab::Machine => machine::show_machine(ui, state),
        SideTab::Preview => preview::show_preview(ui, state),
        SideTab::Device => connection::show_connection(ui, state),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_constructs_with_default_state() {
        let _app = PhotonForgeApp::default();
    }

    #[test]
    fn device_summary_names_the_state() {
        use pf_device::{ConnectionState, DeviceEvent, DeviceSnapshot, parse_status};
        let mut snapshot = DeviceSnapshot::default();
        assert_eq!(device_summary(&snapshot), None);
        snapshot.apply(&DeviceEvent::ConnectionChanged(ConnectionState::Opening));
        assert_eq!(
            device_summary(&snapshot).as_deref(),
            Some("GRBL: connecting…")
        );
        let report = parse_status("<Hold:1|MPos:0.000,0.000,0.000|FS:0,0>").unwrap();
        let positions = report.positions(None);
        snapshot.apply(&DeviceEvent::ConnectionChanged(ConnectionState::Ready {
            version: "1.1h".into(),
        }));
        snapshot.apply(&DeviceEvent::Status { report, positions });
        assert_eq!(device_summary(&snapshot).as_deref(), Some("GRBL: Hold:1"));
    }

    #[test]
    fn edit_labels_name_the_step() {
        assert_eq!(
            edit_label("Undo", Some("Change speed")),
            "Undo Change speed"
        );
        assert_eq!(edit_label("Redo", None), "Redo");
    }
}
