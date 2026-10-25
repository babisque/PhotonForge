//! The latest known state of a connection, built from its events, for
//! display. Console lines (`Received`, `Sent`) are left to the caller.

use pf_core::units::MM_PER_INCH;

use crate::event::{ConnectionState, DeviceEvent};
use crate::job::{JobEvent, JobProgress, JobState, StopStage, transition};
use crate::protocol::ParseError;
use crate::protocol::build_info::BuildInfo;
use crate::protocol::parser_state::{ParserState, SpindleState};
use crate::protocol::settings::GrblSettings;
use crate::protocol::status::{MachineState, Overrides, Positions, StatusReport};

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
    /// GRBL sends `Ov:` only every few reports: the last one is kept.
    pub overrides: Option<Overrides>,
    pub settings: Option<GrblSettings>,
    pub setting_errors: Vec<ParseError>,
    pub build_info: Option<BuildInfo>,
    pub parser_state: Option<ParserState>,
    /// The most recent `ALARM:N`, cleared once GRBL leaves the alarm state.
    pub last_alarm: Option<u8>,
    pub unresponsive: bool,
    pub job: JobState,
    pub job_progress: Option<JobProgress>,
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
                self.overrides = None;
                self.unresponsive = false;
                // The connection thread is gone with its job; a job that was
                // still active failed.
                if let Ok(job) = transition(&self.job, JobEvent::Disconnected(reason.clone())) {
                    self.job = job;
                }
            }
            DeviceEvent::Status { report, positions } => {
                if report.state != MachineState::Alarm {
                    self.last_alarm = None;
                }
                if report.overrides.is_some() {
                    self.overrides = report.overrides;
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
                self.overrides = None;
                self.parser_state = None;
            }
            DeviceEvent::JobStateChanged(job) => {
                if *job == JobState::Running && !self.job.is_active() {
                    self.job_progress = None;
                }
                self.job = job.clone();
                // A job changes the laser's modal state; `$G` is read again
                // once it ends.
                self.parser_state = None;
            }
            DeviceEvent::JobProgress(progress) => self.job_progress = Some(*progress),
            DeviceEvent::Unresponsive => self.unresponsive = true,
            DeviceEvent::Responsive => self.unresponsive = false,
            DeviceEvent::Received { .. }
            | DeviceEvent::Sent(_)
            | DeviceEvent::SentRealtime(_)
            | DeviceEvent::SentLine(_)
            | DeviceEvent::ControlRejected { .. }
            | DeviceEvent::ControlCompleted { .. }
            | DeviceEvent::ControlFailed { .. }
            | DeviceEvent::CommandFailed { .. }
            | DeviceEvent::RequestTimedOut(_)
            | DeviceEvent::Message(_)
            | DeviceEvent::Warning(_) => {}
        }
    }

    pub fn is_connected(&self) -> bool {
        self.link != LinkState::Disconnected
    }

    /// Whether `$X` may be sent now, or why not. The user must still
    /// confirm it.
    pub fn can_unlock(&self) -> Result<(), String> {
        self.free_for_commands()?;
        match self.status.as_ref().map(|s| s.state) {
            Some(MachineState::Alarm) => Ok(()),
            _ => Err(String::from("there is no alarm to clear")),
        }
    }

    /// Whether `$H` may be sent now, or why not.
    pub fn can_home(&self) -> Result<(), String> {
        self.free_for_commands()?;
        let Some(settings) = &self.settings else {
            return Err(String::from("the controller settings are not read yet"));
        };
        if settings.homing() != Some(true) {
            return Err(String::from("homing is disabled on the controller ($22=0)"));
        }
        match self.status.as_ref().map(|s| s.state) {
            Some(MachineState::Idle | MachineState::Alarm) => Ok(()),
            Some(other) => Err(format!(
                "the machine is {other:?}; homing needs Idle or Alarm"
            )),
            None => Err(String::from("no status report yet")),
        }
    }

    /// Whether a jog may be sent now, or why not. A jog runs with GRBL's
    /// current spindle state and S (`gcode.c`, jog branch), so the last
    /// `$G` must show the laser off (`M5`).
    pub fn can_jog(&self) -> Result<(), String> {
        self.free_for_commands()?;
        match self.status.as_ref().map(|s| s.state) {
            Some(MachineState::Idle | MachineState::Jog) => {}
            Some(MachineState::Alarm) => {
                return Err(String::from("unlock or home the machine first"));
            }
            Some(other) => return Err(format!("the machine is {other:?}; jogging needs Idle")),
            None => return Err(String::from("no status report yet")),
        }
        if self.settings.is_none() || self.machine_position_mm().is_none() {
            return Err(String::from("the head position is not known yet"));
        }
        match self.parser_state.as_ref().and_then(|p| p.spindle) {
            Some(SpindleState::Off) => Ok(()),
            Some(_) => Err(String::from(
                "the laser is on (M3/M4) in GRBL's parser state; a jog would fire it",
            )),
            None => Err(String::from("checking the laser state ($G)")),
        }
    }

    /// Connected, and no job or stop sequence owns the connection. A stop
    /// waiting for an unlock lets unlock and homing through.
    fn free_for_commands(&self) -> Result<(), String> {
        if !matches!(self.link, LinkState::Ready { .. }) {
            return Err(String::from("not connected"));
        }
        match &self.job {
            JobState::Running | JobState::Held => Err(String::from("a job is running")),
            JobState::Stopping { stage, .. } if *stage != StopStage::AwaitingUnlock => {
                Err(String::from("the machine is stopping"))
            }
            _ => Ok(()),
        }
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
    fn overrides_survive_reports_without_them_until_a_reset() {
        let mut s = snapshot_after(&[status("<Idle|MPos:0,0,0|FS:0,0|Ov:100,50,120>")]);
        s.apply(&status("<Idle|MPos:0,0,0|FS:0,0>"));
        assert_eq!(s.overrides.map(|o| o.spindle), Some(120));
        s.apply(&DeviceEvent::ControllerReset);
        assert_eq!(s.overrides, None);
        s.apply(&status("<Idle|MPos:0,0,0|FS:0,0|Ov:100,100,100>"));
        s.apply(&DeviceEvent::Disconnected {
            reason: "unplugged".into(),
        });
        assert_eq!(s.overrides, None);
    }

    #[test]
    fn job_state_and_progress_follow_the_events() {
        let progress = JobProgress {
            lines_sent: 10,
            lines_acked: 4,
            total: 20,
        };
        let mut s = snapshot_after(&[
            DeviceEvent::JobStateChanged(JobState::Running),
            DeviceEvent::JobProgress(progress),
        ]);
        assert_eq!(s.job, JobState::Running);
        assert_eq!(s.job_progress, Some(progress));
        // Unplugging mid-job is a fault, never a pause to resume.
        s.apply(&DeviceEvent::Disconnected {
            reason: "unplugged".into(),
        });
        assert_eq!(
            s.job,
            JobState::Faulted(crate::job::FaultReason::Disconnected("unplugged".into()))
        );
        // An idle connection that drops stays idle.
        let mut s = DeviceSnapshot::default();
        s.apply(&DeviceEvent::Disconnected {
            reason: "closed".into(),
        });
        assert_eq!(s.job, JobState::Idle);
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
