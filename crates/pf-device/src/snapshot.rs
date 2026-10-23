//! The latest known state of a connection, built from its events, for
//! display. Console lines (`Received`, `Sent`) are left to the caller.

use pf_core::units::MM_PER_INCH;

use crate::event::{ConnectionState, DeviceEvent};
use crate::protocol::ParseError;
use crate::protocol::build_info::BuildInfo;
use crate::protocol::parser_state::ParserState;
use crate::protocol::settings::GrblSettings;
use crate::protocol::status::{MachineState, Positions, StatusReport};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LinkState {
    #[default]
    Disconnected,
    Opening,
    Identifying,
    Ready {
        version: String,
    },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeviceSnapshot {
    pub link: LinkState,
    /// Why the last connection ended, until the next one starts.
    pub disconnect_reason: Option<String>,
    pub status: Option<StatusReport>,
    pub positions: Option<Positions>,
    pub settings: Option<GrblSettings>,
    pub setting_errors: Vec<ParseError>,
    pub build_info: Option<BuildInfo>,
    pub parser_state: Option<ParserState>,
    /// The most recent `ALARM:N`, cleared once GRBL leaves the alarm state.
    pub last_alarm: Option<u8>,
    pub unresponsive: bool,
}

impl DeviceSnapshot {
    pub fn apply(&mut self, event: &DeviceEvent) {
        match event {
            DeviceEvent::ConnectionChanged(state) => {
                if *state == ConnectionState::Opening {
                    // A new connection: forget everything from the last one.
                    *self = DeviceSnapshot::default();
                }
                self.link = match state {
                    ConnectionState::Opening => LinkState::Opening,
                    ConnectionState::Identifying => LinkState::Identifying,
                    ConnectionState::Ready { version } => LinkState::Ready {
                        version: version.clone(),
                    },
                };
            }
            DeviceEvent::Disconnected { reason } => {
                self.link = LinkState::Disconnected;
                self.disconnect_reason = Some(reason.clone());
                // Live values are stale now; what was read stays visible.
                self.status = None;
                self.positions = None;
                self.unresponsive = false;
            }
            DeviceEvent::Status { report, positions } => {
                if report.state != MachineState::Alarm {
                    self.last_alarm = None;
                }
                self.status = Some(report.clone());
                self.positions = Some(*positions);
            }
            DeviceEvent::SettingsLoaded { settings, errors } => {
                self.settings = Some(settings.clone());
                self.setting_errors = errors.clone();
            }
            DeviceEvent::BuildInfoLoaded(info) => self.build_info = Some(info.clone()),
            DeviceEvent::ParserStateLoaded(state) => self.parser_state = Some(state.clone()),
            DeviceEvent::Alarm(code) => self.last_alarm = Some(*code),
            DeviceEvent::ControllerReset => {
                self.status = None;
                self.positions = None;
                self.parser_state = None;
            }
            DeviceEvent::Unresponsive => self.unresponsive = true,
            DeviceEvent::Responsive => self.unresponsive = false,
            DeviceEvent::Received { .. }
            | DeviceEvent::Sent(_)
            | DeviceEvent::CommandFailed { .. }
            | DeviceEvent::RequestTimedOut(_)
            | DeviceEvent::Message(_)
            | DeviceEvent::Warning(_) => {}
        }
    }

    pub fn is_connected(&self) -> bool {
        self.link != LinkState::Disconnected
    }

    /// Where the laser head is, in machine millimeters. GRBL reports inches
    /// when `$13=1`; until the settings are read, millimeters are assumed.
    pub fn machine_position_mm(&self) -> Option<[f64; 3]> {
        let position = self.positions?.machine?;
        let inches = self
            .settings
            .as_ref()
            .and_then(GrblSettings::report_inches)
            .unwrap_or(false);
        Some(if inches {
            position.map(|v| v * MM_PER_INCH)
        } else {
            position
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::status::parse_status;

    fn status(line: &str) -> DeviceEvent {
        let report = parse_status(line).unwrap();
        let positions = report.positions(None);
        DeviceEvent::Status { report, positions }
    }

    fn snapshot_after(events: &[DeviceEvent]) -> DeviceSnapshot {
        let mut snapshot = DeviceSnapshot::default();
        for event in events {
            snapshot.apply(event);
        }
        snapshot
    }

    #[test]
    fn follows_a_connection_from_open_to_ready() {
        let s = snapshot_after(&[
            DeviceEvent::ConnectionChanged(ConnectionState::Opening),
            DeviceEvent::ConnectionChanged(ConnectionState::Identifying),
            DeviceEvent::ConnectionChanged(ConnectionState::Ready {
                version: "1.1h".into(),
            }),
            status("<Idle|MPos:1.000,2.000,0.000|FS:0,0>"),
        ]);
        assert_eq!(
            s.link,
            LinkState::Ready {
                version: "1.1h".into()
            }
        );
        assert!(s.is_connected());
        assert_eq!(s.status.as_ref().map(|r| r.state), Some(MachineState::Idle));
        assert_eq!(s.positions.and_then(|p| p.machine), Some([1.0, 2.0, 0.0]));
    }

    #[test]
    fn alarms_clear_when_grbl_leaves_the_alarm_state() {
        let mut s = snapshot_after(&[
            DeviceEvent::Alarm(2),
            status("<Alarm|MPos:0.000,0.000,0.000|FS:0,0>"),
        ]);
        assert_eq!(s.last_alarm, Some(2));
        s.apply(&status("<Idle|MPos:0.000,0.000,0.000|FS:0,0>"));
        assert_eq!(s.last_alarm, None);
    }

    #[test]
    fn disconnecting_keeps_what_was_read_but_drops_live_values() {
        let mut s = snapshot_after(&[
            DeviceEvent::SettingsLoaded {
                settings: GrblSettings::default(),
                errors: vec![],
            },
            status("<Idle|MPos:0.000,0.000,0.000|FS:0,0>"),
            DeviceEvent::Unresponsive,
            DeviceEvent::Disconnected {
                reason: "Connection lost: broken pipe".into(),
            },
        ]);
        assert_eq!(s.link, LinkState::Disconnected);
        assert!(!s.is_connected());
        assert_eq!(
            s.disconnect_reason.as_deref(),
            Some("Connection lost: broken pipe")
        );
        assert!(s.status.is_none() && s.positions.is_none());
        assert!(!s.unresponsive);
        assert!(s.settings.is_some());
        // The next connection starts clean.
        s.apply(&DeviceEvent::ConnectionChanged(ConnectionState::Opening));
        assert_eq!(s.disconnect_reason, None);
        assert!(s.settings.is_none());
    }

    #[test]
    fn responsiveness_and_resets_are_tracked() {
        let mut s = snapshot_after(&[DeviceEvent::Unresponsive]);
        assert!(s.unresponsive);
        s.apply(&DeviceEvent::Responsive);
        assert!(!s.unresponsive);
        s.apply(&status("<Idle|MPos:0.000,0.000,0.000|FS:0,0>"));
        s.apply(&DeviceEvent::ControllerReset);
        assert!(s.status.is_none());
    }

    #[test]
    fn head_position_is_converted_from_inches_when_grbl_reports_them() {
        let mut s = snapshot_after(&[status("<Idle|MPos:1.000,2.000,0.000|FS:0,0>")]);
        assert_eq!(s.machine_position_mm(), Some([1.0, 2.0, 0.0]));
        let mut settings = GrblSettings::default();
        settings.insert(13, "1").unwrap();
        s.apply(&DeviceEvent::SettingsLoaded {
            settings,
            errors: vec![],
        });
        assert_eq!(s.machine_position_mm(), Some([25.4, 50.8, 0.0]));
        // A work position alone does not tell where the machine is.
        let w = snapshot_after(&[status("<Idle|WPos:1.000,2.000,0.000|FS:0,0>")]);
        assert_eq!(w.machine_position_mm(), None);
    }
}
