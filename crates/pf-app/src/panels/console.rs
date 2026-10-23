use std::collections::VecDeque;
use std::time::Instant;

use pf_device::{ConnectionState, DeviceEvent, ReadOnlyRequest, Response, alarm_info, error_info};

use crate::state::AppState;

/// Older lines are dropped beyond this.
pub const MAX_CONSOLE_LINES: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From the controller.
    In,
    /// To the controller.
    Out,
    /// PhotonForge's own note, e.g. what an alarm code means.
    Note,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleEntry {
    pub seconds: f64, // since the console was created
    pub direction: Direction,
    pub text: String,
    /// A `?` or `<…>` report: frequent, hidden by default.
    pub is_status: bool,
}

/// The device log. There is no input line: in this phase only the fixed
/// read-only requests can be sent.
#[derive(Debug)]
pub struct Console {
    started: Instant,
    entries: VecDeque<ConsoleEntry>,
    pub hide_status: bool,
}

impl Default for Console {
    fn default() -> Self {
        Console {
            started: Instant::now(),
            entries: VecDeque::new(),
            hide_status: true,
        }
    }
}

impl Console {
    pub fn record(&mut self, event: &DeviceEvent) {
        match event {
            DeviceEvent::Received { line, response } => {
                let is_status = matches!(response, Response::Status(_));
                self.push(Direction::In, line.clone(), is_status);
            }
            DeviceEvent::Sent(request) => {
                let is_status = *request == ReadOnlyRequest::StatusQuery;
                self.push(Direction::Out, request.label().to_owned(), is_status);
            }
            DeviceEvent::ConnectionChanged(ConnectionState::Ready { version }) => {
                self.note(format!("Connected to GRBL {version}."));
            }
            DeviceEvent::Disconnected { reason } => self.note(format!("Disconnected: {reason}")),
            DeviceEvent::Alarm(code) => self.note(describe_alarm(*code)),
            DeviceEvent::CommandFailed { request, code } => self.note(format!(
                "{} failed: {}",
                request.label(),
                describe_error(*code)
            )),
            DeviceEvent::RequestTimedOut(request) => {
                self.note(format!(
                    "{} got no answer and was given up.",
                    request.label()
                ));
            }
            DeviceEvent::ControllerReset => self.note(String::from("GRBL was reset.")),
            DeviceEvent::Unresponsive => {
                self.note(String::from("No status report for 3 s."));
            }
            DeviceEvent::Warning(text) => self.note(text.clone()),
            _ => {}
        }
    }

    pub fn note(&mut self, text: String) {
        self.push(Direction::Note, text, false);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn visible(&self) -> impl Iterator<Item = &ConsoleEntry> {
        let hide_status = self.hide_status;
        self.entries
            .iter()
            .filter(move |e| !(hide_status && e.is_status))
    }

    /// The visible lines as text, for the clipboard.
    pub fn to_text(&self) -> String {
        self.visible()
            .map(format_entry)
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn push(&mut self, direction: Direction, text: String, is_status: bool) {
        if self.entries.len() >= MAX_CONSOLE_LINES {
            self.entries.pop_front();
        }
        self.entries.push_back(ConsoleEntry {
            seconds: self.started.elapsed().as_secs_f64(),
            direction,
            text,
            is_status,
        });
    }
}

pub fn describe_alarm(code: u8) -> String {
    match alarm_info(code) {
        Some(info) => format!("ALARM:{code} {}: {}", info.name, info.description),
        None => format!("ALARM:{code} (unknown alarm code)"),
    }
}

pub fn describe_error(code: u8) -> String {
    match error_info(code) {
        Some(info) => format!("error:{code} {}: {}", info.name, info.description),
        None => format!("error:{code} (unknown error code)"),
    }
}

fn format_entry(entry: &ConsoleEntry) -> String {
    let arrow = match entry.direction {
        Direction::In => "<",
        Direction::Out => ">",
        Direction::Note => "*",
    };
    format!("{:9.3} {arrow} {}", entry.seconds, entry.text)
}

pub fn show_console(ui: &mut egui::Ui, state: &mut AppState) {
    let console = &mut state.device.console;
    ui.horizontal(|ui| {
        ui.strong("Console");
        ui.checkbox(&mut console.hide_status, "Hide status reports");
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(console.to_text());
        }
        if ui.button("Clear").clicked() {
            console.clear();
        }
    });
    ui.separator();
    let note = ui.visuals().warn_fg_color;
    let out = ui.visuals().hyperlink_color;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for entry in console.visible() {
                let text = egui::RichText::new(format_entry(entry)).monospace();
                let text = match entry.direction {
                    Direction::In => text,
                    Direction::Out => text.color(out),
                    Direction::Note => text.color(note),
                };
                ui.label(text);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_device::parse_line;

    fn received(line: &str) -> DeviceEvent {
        DeviceEvent::Received {
            line: line.to_owned(),
            response: parse_line(line),
        }
    }

    fn texts(console: &Console) -> Vec<String> {
        console.visible().map(|e| e.text.clone()).collect()
    }

    #[test]
    fn status_traffic_is_hidden_by_default() {
        let mut console = Console::default();
        console.record(&DeviceEvent::Sent(ReadOnlyRequest::StatusQuery));
        console.record(&received("<Idle|MPos:0.000,0.000,0.000|FS:0,0>"));
        console.record(&DeviceEvent::Sent(ReadOnlyRequest::ViewSettings));
        console.record(&received("$32=1"));
        assert_eq!(texts(&console), vec!["$$", "$32=1"]);
        console.hide_status = false;
        assert_eq!(console.visible().count(), 4);
    }

    #[test]
    fn codes_are_explained_in_notes() {
        let mut console = Console::default();
        console.record(&DeviceEvent::Alarm(2));
        console.record(&DeviceEvent::CommandFailed {
            request: ReadOnlyRequest::ViewSettings,
            code: 8,
        });
        console.record(&DeviceEvent::Alarm(99));
        let notes = texts(&console);
        assert!(notes[0].starts_with("ALARM:2 Soft limit: "));
        assert!(notes[1].starts_with("$$ failed: error:8 Not idle: "));
        assert_eq!(notes[2], "ALARM:99 (unknown alarm code)");
        assert!(console.visible().all(|e| e.direction == Direction::Note));
    }

    #[test]
    fn the_log_is_capped() {
        let mut console = Console::default();
        for i in 0..MAX_CONSOLE_LINES + 5 {
            console.note(format!("line {i}"));
        }
        assert_eq!(console.visible().count(), MAX_CONSOLE_LINES);
        assert_eq!(console.visible().next().unwrap().text, "line 5");
        console.clear();
        assert_eq!(console.visible().count(), 0);
    }

    #[test]
    fn copied_text_marks_direction() {
        let mut console = Console::default();
        console.record(&DeviceEvent::Sent(ReadOnlyRequest::BuildInfo));
        console.record(&received("[VER:1.1h.20190825:]"));
        console.record(&DeviceEvent::Disconnected {
            reason: "Connection lost".into(),
        });
        let lines: Vec<String> = console
            .to_text()
            .lines()
            .map(|l| l.trim_start().split_once(' ').unwrap().1.to_owned())
            .collect();
        assert_eq!(
            lines,
            vec![
                "> $I",
                "< [VER:1.1h.20190825:]",
                "* Disconnected: Connection lost"
            ]
        );
    }
}
