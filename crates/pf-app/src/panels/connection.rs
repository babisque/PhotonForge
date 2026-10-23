use pf_device::{
    COMMON_BAUD_RATES, DeviceSnapshot, LinkState, MachineState, POLL_HZ_RANGE, ReadOnlyRequest,
    compare_with_profile, setting_info,
};

use crate::actions;
use crate::panels::console::describe_alarm;
use crate::state::AppState;

/// Requested while drawing, run afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceAction {
    RefreshPorts,
    Connect,
    Disconnect,
    Request(ReadOnlyRequest),
}

/// The Device tab: connect to a GRBL controller and read its state. This
/// phase never sends anything that moves the machine or fires the laser.
pub fn show_connection(ui: &mut egui::Ui, state: &mut AppState) {
    ui.heading("Device");
    let mut wanted = Vec::new();
    connection_section(ui, state, &mut wanted);

    let snapshot = &state.device.snapshot;
    if snapshot.is_connected() {
        ui.separator();
        status_section(ui, snapshot);
        ui.separator();
        controller_section(ui, state, &mut wanted);
    }

    for action in wanted {
        match action {
            DeviceAction::RefreshPorts => actions::refresh_ports(state),
            DeviceAction::Connect => actions::connect_device(state),
            DeviceAction::Disconnect => actions::disconnect_device(state),
            DeviceAction::Request(request) => actions::device_request(state, request),
        }
    }
}

fn connection_section(ui: &mut egui::Ui, state: &mut AppState, wanted: &mut Vec<DeviceAction>) {
    let device = &mut state.device;
    let connected = device.connection.is_some();
    ui.add_enabled_ui(!connected, |ui| {
        egui::Grid::new("device_connection")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Port");
                ui.horizontal(|ui| {
                    let shown = if device.port.is_empty() {
                        "Select a port"
                    } else {
                        device.port.as_str()
                    };
                    egui::ComboBox::from_id_salt("device_port")
                        .selected_text(shown.to_owned())
                        .show_ui(ui, |ui| {
                            for port in &device.ports {
                                let label = match &port.description {
                                    Some(description) => format!("{} – {description}", port.name),
                                    None => port.name.clone(),
                                };
                                ui.selectable_value(&mut device.port, port.name.clone(), label);
                            }
                            if device.ports.is_empty() {
                                ui.weak("No ports found");
                            }
                        });
                    if ui
                        .button("⟳")
                        .on_hover_text("Refresh the port list")
                        .clicked()
                    {
                        wanted.push(DeviceAction::RefreshPorts);
                    }
                });
                ui.end_row();

                ui.label("Baud rate");
                egui::ComboBox::from_id_salt("device_baud")
                    .selected_text(device.baud.to_string())
                    .show_ui(ui, |ui| {
                        for baud in COMMON_BAUD_RATES {
                            ui.selectable_value(&mut device.baud, baud, baud.to_string());
                        }
                    });
                ui.end_row();

                ui.label("Status rate");
                ui.add(egui::Slider::new(&mut device.poll_hz, POLL_HZ_RANGE).suffix(" Hz"));
                ui.end_row();

                ui.label("DTR");
                ui.checkbox(&mut device.dtr, "Assert DTR on connect")
                    .on_hover_text(
                        "On Arduino-based boards DTR resets GRBL. Leave off unless the \
                         board sends nothing without it. See docs/hardware/dtr.md.",
                    );
                ui.end_row();
            });
    });

    ui.horizontal(|ui| {
        if connected {
            if ui.button("Disconnect").clicked() {
                wanted.push(DeviceAction::Disconnect);
            }
        } else {
            let can_connect = !device.port.is_empty();
            if ui
                .add_enabled(can_connect, egui::Button::new("Connect"))
                .on_hover_text("Read-only: status, settings and build info")
                .clicked()
            {
                wanted.push(DeviceAction::Connect);
            }
        }
        link_label(ui, &device.snapshot);
    });
    if let (LinkState::Disconnected, Some(reason)) =
        (&device.snapshot.link, &device.snapshot.disconnect_reason)
    {
        ui.weak(reason);
    }
}

fn link_label(ui: &mut egui::Ui, snapshot: &DeviceSnapshot) {
    match &snapshot.link {
        LinkState::Disconnected => {
            ui.label("Disconnected");
        }
        LinkState::Opening => {
            ui.spinner();
            ui.label("Opening port…");
        }
        LinkState::Identifying => {
            ui.spinner();
            ui.label("Waiting for GRBL…");
        }
        LinkState::Ready { version } => {
            ui.label(format!("Connected (Grbl {version})"));
        }
    }
}

fn status_section(ui: &mut egui::Ui, snapshot: &DeviceSnapshot) {
    let Some(status) = &snapshot.status else {
        ui.weak("No status report yet.");
        return;
    };
    let (name, color) = state_label(status.state, ui.visuals());
    ui.horizontal(|ui| {
        ui.label("State");
        ui.label(egui::RichText::new(name).strong().size(18.0).color(color));
    });
    if snapshot.unresponsive {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Not responding (GRBL sends no status while homing).",
        );
    }
    if let Some(code) = snapshot.last_alarm {
        ui.colored_label(ui.visuals().error_fg_color, describe_alarm(code));
    }
    if status.state == MachineState::Alarm {
        ui.weak(
            "Unlocking and homing arrive in a later phase; PhotonForge does not send them yet.",
        );
    }

    let units = match snapshot.settings.as_ref().and_then(|s| s.report_inches()) {
        Some(true) => "in",
        _ => "mm",
    };
    let positions = snapshot.positions;
    egui::Grid::new("device_status")
        .num_columns(4)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            for (label, value) in [
                ("Machine", positions.and_then(|p| p.machine)),
                ("Work", positions.and_then(|p| p.work)),
            ] {
                ui.label(label);
                match value {
                    Some([x, y, z]) => {
                        for v in [x, y, z] {
                            ui.monospace(format!("{v:9.3}"));
                        }
                    }
                    None => {
                        for _ in 0..3 {
                            ui.monospace("        —");
                        }
                    }
                }
                ui.end_row();
            }
        });
    ui.weak(format!("X, Y, Z in {units}"));

    egui::Grid::new("device_rates")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("Feed");
            ui.label(optional(status.feed, &format!(" {units}/min")));
            ui.end_row();
            ui.label("Power (S)");
            ui.label(optional(status.spindle, ""));
            ui.end_row();
            if let Some(ov) = status.overrides {
                ui.label("Overrides");
                ui.label(format!(
                    "feed {} % · rapid {} % · power {} %",
                    ov.feed, ov.rapid, ov.spindle
                ));
                ui.end_row();
            }
            if let Some(pins) = &status.pins {
                ui.label("Pins");
                ui.label(pins);
                ui.end_row();
            }
            if let Some(buffer) = status.buffer {
                ui.label("Buffer free");
                ui.label(format!(
                    "{} blocks · {} bytes",
                    buffer.planner_blocks_free, buffer.rx_bytes_free
                ));
                ui.end_row();
            }
        });
}

fn controller_section(ui: &mut egui::Ui, state: &AppState, wanted: &mut Vec<DeviceAction>) {
    let snapshot = &state.device.snapshot;
    let ready = matches!(snapshot.link, LinkState::Ready { .. });
    let machine_state = snapshot.status.as_ref().map(|s| s.state);
    ui.horizontal_wrapped(|ui| {
        for (request, label) in [
            (ReadOnlyRequest::ViewSettings, "Read settings ($$)"),
            (ReadOnlyRequest::BuildInfo, "Build info ($I)"),
            (ReadOnlyRequest::ParserState, "Parser state ($G)"),
        ] {
            let allowed = ready && request.accepted_in(machine_state);
            let button = ui
                .add_enabled(allowed, egui::Button::new(label))
                .on_disabled_hover_text("GRBL does not answer this in its current state");
            if button.clicked() {
                wanted.push(DeviceAction::Request(request));
            }
        }
    });

    if let Some(info) = &snapshot.build_info {
        let date = info.version.build_date.as_deref().unwrap_or("no date");
        let mut text = format!("Grbl {} ({date})", info.version.version);
        if let Some(options) = &info.options {
            text.push_str(&format!(
                " · options {} · RX buffer {} bytes",
                if options.codes.is_empty() {
                    "none"
                } else {
                    &options.codes
                },
                options.rx_buffer_bytes
            ));
        }
        ui.label(text);
    }
    if let Some(parser) = &snapshot.parser_state {
        ui.label(format!("Parser: {}", parser.words.join(" ")));
    }

    let Some(settings) = &snapshot.settings else {
        return;
    };
    for hint in compare_with_profile(settings, &state.profile) {
        ui.colored_label(ui.visuals().warn_fg_color, hint.to_string());
    }
    for err in &snapshot.setting_errors {
        ui.colored_label(ui.visuals().error_fg_color, err.to_string());
    }
    egui::CollapsingHeader::new(format!("Settings ({})", settings.len()))
        .id_salt("device_settings")
        .show(ui, |ui| {
            egui::Grid::new("device_settings_table")
                .num_columns(3)
                .striped(true)
                .show(ui, |ui| {
                    for (number, setting) in settings.iter() {
                        ui.monospace(format!("${number}"));
                        ui.monospace(&setting.raw);
                        match setting_info(number) {
                            Some(info) => {
                                ui.label(format!("{} ({})", info.name, info.units))
                                    .on_hover_text(info.description);
                            }
                            None => {
                                ui.weak("Not a GRBL 1.1 setting");
                            }
                        }
                        ui.end_row();
                    }
                });
        });
}

fn state_label(state: MachineState, visuals: &egui::Visuals) -> (String, egui::Color32) {
    let green = egui::Color32::from_rgb(40, 160, 70);
    let blue = egui::Color32::from_rgb(40, 110, 220);
    let color = match state {
        MachineState::Idle => green,
        MachineState::Run | MachineState::Jog | MachineState::Home => blue,
        MachineState::Hold(_) | MachineState::Door(_) => visuals.warn_fg_color,
        MachineState::Alarm => visuals.error_fg_color,
        MachineState::Check => visuals.text_color(),
        MachineState::Sleep => visuals.weak_text_color(),
    };
    (state_name(state), color)
}

/// E.g. "Idle", "Hold:1", "Homing".
pub fn state_name(state: MachineState) -> String {
    let with_sub = |name: &str, sub: Option<u8>| match sub {
        Some(n) => format!("{name}:{n}"),
        None => name.to_owned(),
    };
    match state {
        MachineState::Idle => "Idle".into(),
        MachineState::Run => "Run".into(),
        MachineState::Jog => "Jog".into(),
        MachineState::Home => "Homing".into(),
        MachineState::Hold(sub) => with_sub("Hold", sub),
        MachineState::Door(sub) => with_sub("Door", sub),
        MachineState::Alarm => "Alarm".into(),
        MachineState::Check => "Check mode".into(),
        MachineState::Sleep => "Sleep".into(),
    }
}

fn optional(value: Option<f64>, suffix: &str) -> String {
    match value {
        Some(v) => format!("{v}{suffix}"),
        None => String::from("—"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_device::{ConnectionState, DeviceEvent, GrblSettings, parse_status};

    fn connected_state() -> AppState {
        let mut state = AppState::new();
        let report = parse_status(
            "<Alarm|MPos:1.000,2.000,0.000|FS:0,0|WCO:0.000,0.000,0.000|Ov:100,100,100|Pn:X|Bf:15,128>",
        )
        .unwrap();
        let positions = report.positions(None);
        let mut settings = GrblSettings::default();
        settings.insert(30, "255").unwrap();
        settings.insert(32, "0").unwrap();
        settings.insert(341, "fork setting").unwrap();
        for event in [
            DeviceEvent::ConnectionChanged(ConnectionState::Opening),
            DeviceEvent::ConnectionChanged(ConnectionState::Ready {
                version: "1.1h".into(),
            }),
            DeviceEvent::Alarm(2),
            DeviceEvent::Status { report, positions },
            DeviceEvent::SettingsLoaded {
                settings,
                errors: vec![],
            },
            DeviceEvent::Unresponsive,
        ] {
            state.device.snapshot.apply(&event);
        }
        state
    }

    fn render(state: &mut AppState) {
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show_connection(ui, state);
            });
            // No renderer here to upload font textures to.
            output.textures_delta.clear();
        }
    }

    #[test]
    fn renders_headless_disconnected_and_connected() {
        let mut state = AppState::new();
        render(&mut state);
        assert!(state.device.connection.is_none());

        let mut state = connected_state();
        render(&mut state);
        // Drawing never connects or requests anything.
        assert!(state.device.connection.is_none());
        assert!(state.device.console.visible().next().is_none());
    }

    #[test]
    fn states_have_names_and_colors() {
        let visuals = egui::Visuals::dark();
        assert_eq!(
            state_label(MachineState::Hold(Some(1)), &visuals).0,
            "Hold:1"
        );
        assert_eq!(state_label(MachineState::Door(None), &visuals).0, "Door");
        assert_eq!(
            state_label(MachineState::Alarm, &visuals).1,
            visuals.error_fg_color
        );
        assert_eq!(optional(Some(1500.0), " mm/min"), "1500 mm/min");
        assert_eq!(optional(None, " mm/min"), "—");
    }
}
