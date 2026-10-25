//! The protocol logic of one connection, without threads or I/O: bytes and
//! the current time go in, actions come out. The connection thread performs
//! the actions; tests drive the session directly with chosen times.
//!
//! An action can only send an `Outgoing` command, a closed set. Without a
//! control request only read-only requests are ever sent.

use std::collections::VecDeque;
use std::mem;
use std::time::{Duration, Instant};

use crate::control::{ControlRequest, JobRequest};
use crate::event::{ConnectionState, DeviceEvent};
use crate::job::{FaultReason, JobEvent, JobProgress, JobState, StopStage, transition};
use crate::jog::{JogMove, plan_jog};
use crate::preflight::{preflight, scan_job};
use crate::protocol::ParseError;
use crate::protocol::build_info::{
    BuildInfo, BuildOptions, Version, is_supported_version, parse_options, parse_version,
};
use crate::protocol::codes::{alarm_info, error_info};
use crate::protocol::outgoing::{Outgoing, Realtime};
use crate::protocol::parse::{Response, parse_line};
use crate::protocol::parser_state::{ParserState, parse_parser_state};
use crate::protocol::readonly::ReadOnlyRequest;
use crate::protocol::settings::GrblSettings;
use crate::protocol::status::{MachineState, StatusReport};
use crate::snapshot::DeviceSnapshot;
use crate::stream::{Streamer, rx_capacity};
use pf_core::MachineProfile;

/// How long to wait for the welcome message before asking with `$I`.
/// Without a reset on connect (DTR off) the welcome message never comes.
pub(crate) const BANNER_WAIT: Duration = Duration::from_secs(2);
/// How long `$I` may take before the device is declared not GRBL.
pub(crate) const VERSION_WAIT: Duration = Duration::from_secs(3);
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// Silence after which the controller counts as unresponsive.
pub(crate) const STATUS_SILENCE: Duration = Duration::from_secs(3);
/// GRBL lines are far shorter; anything longer is cut into pieces.
const MAX_LINE_BYTES: usize = 256;
/// Stop: how long to wait for the feed hold to stop motion before the
/// reset. Holding first keeps the position (GRBL `interface.md`, `0x18`).
const STOP_HOLD_WAIT: Duration = Duration::from_secs(1);
/// Stop: how long to wait for the banner, then for a status, after `0x18`.
const STOP_RESET_WAIT: Duration = Duration::from_secs(2);
/// Pause: how long GRBL has to report the hold complete with the laser off.
/// `A:` comes with `Ov:`, at most every 20 reports while held.
const HOLD_CONFIRM_WAIT: Duration = Duration::from_secs(5);
/// Status polling while a pause or stop is being confirmed.
const FAST_POLL: Duration = Duration::from_millis(100);
/// Homing time limit when the settings do not give a better one.
const HOMING_FALLBACK: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "a few actions per loop pass, consumed at once; boxing would only add allocations"
)]
pub(crate) enum Action {
    Send(Outgoing),
    Emit(DeviceEvent),
    /// End the connection with this reason.
    Disconnect(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Identifying { since: Instant },
    Ready { since: Instant },
}

/// A line command waiting for its `ok` or `error:N`. GRBL answers them in
/// order, so the oldest one gets the next answer.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Pending {
    Request {
        request: ReadOnlyRequest,
        sent_at: Instant,
    },
    JobLine,
    LaserOff,
    Unlock {
        sent_at: Instant,
    },
    /// `$H` is answered only once the cycle completes.
    Home {
        deadline: Instant,
    },
    Jog {
        sent_at: Instant,
    },
}

/// Replies collected for the line command in flight.
#[derive(Debug, Default)]
struct Collected {
    settings: GrblSettings,
    setting_errors: Vec<ParseError>,
    version: Option<Version>,
    options: Option<BuildOptions>,
    parser_state: Option<ParserState>,
}

#[derive(Debug)]
pub(crate) struct Session {
    poll_interval: Duration,
    phase: Phase,
    /// Bytes of a line that has not ended yet.
    partial: Vec<u8>,
    in_flight: VecDeque<Pending>,
    queue: VecDeque<ReadOnlyRequest>,
    collected: Collected,
    asked_version: bool,
    last_poll: Option<Instant>,
    last_status: Option<Instant>,
    silent: bool,
    state: Option<MachineState>,
    /// GRBL sends WCO only now and then; positions need the last one.
    last_wco: Option<[f64; 3]>,
    /// Everything emitted so far, for the pre-flight checks.
    snapshot: DeviceSnapshot,
    job: JobState,
    streamer: Option<Streamer>,
    reported_progress: Option<JobProgress>,
    /// When a pause was asked for, until GRBL confirms it.
    hold_requested: Option<Instant>,
    resume_requested: bool,
    /// When the current stop stage began.
    stage_since: Instant,
    /// The banner arrived after the stop sequence's reset.
    reset_banner_seen: bool,
}

impl Session {
    pub(crate) fn new(poll_interval: Duration, now: Instant) -> Session {
        Session {
            poll_interval,
            phase: Phase::Identifying { since: now },
            partial: Vec::new(),
            in_flight: VecDeque::new(),
            queue: VecDeque::new(),
            collected: Collected::default(),
            asked_version: false,
            last_poll: None,
            last_status: None,
            silent: false,
            state: None,
            last_wco: None,
            snapshot: DeviceSnapshot::default(),
            job: JobState::Idle,
            streamer: None,
            reported_progress: None,
            hold_requested: None,
            resume_requested: false,
            stage_since: now,
            reset_banner_seen: false,
        }
    }

    /// Queues a request from the user. `?` is sent right away once ready;
    /// line commands wait for their turn and a state GRBL accepts them in.
    /// While a job streams they wait until it ends.
    pub(crate) fn request(&mut self, request: ReadOnlyRequest, now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        if request.is_realtime() {
            if self.is_ready() {
                self.last_poll = Some(now);
                out.push(Action::Send(Outgoing::ReadOnly(request)));
            }
        } else {
            self.enqueue(request);
            self.dispatch(now, &mut out);
        }
        self.track(out)
    }

    pub(crate) fn control(&mut self, control: ControlRequest, now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        let label = control.label();
        let result = if self.is_ready() {
            match control {
                ControlRequest::Start(job) => self.start(job, now, &mut out),
                ControlRequest::Hold => self.hold(now, &mut out),
                ControlRequest::Resume => self.resume(&mut out),
                ControlRequest::Stop => {
                    self.job_event(JobEvent::StopRequested, now, &mut out);
                    Ok(())
                }
                ControlRequest::AcknowledgeFault => {
                    if matches!(self.job, JobState::Faulted(_) | JobState::Done) {
                        self.job_event(JobEvent::Acknowledge, now, &mut out);
                        Ok(())
                    } else {
                        Err(vec![String::from("there is nothing to acknowledge")])
                    }
                }
                ControlRequest::Unlock => self.unlock(now, &mut out),
                ControlRequest::Home => self.home(now, &mut out),
                ControlRequest::Jog { step, profile } => self.jog(step, &profile, now, &mut out),
                ControlRequest::JogCancel => self.jog_cancel(&mut out),
            }
        } else {
            Err(vec![String::from("not connected to a GRBL controller")])
        };
        if let Err(reasons) = result {
            out.push(Action::Emit(DeviceEvent::ControlRejected {
                control: label,
                reasons,
            }));
        }
        self.dispatch(now, &mut out);
        self.fill_stream(&mut out);
        self.report_progress(&mut out);
        self.track(out)
    }

    pub(crate) fn on_bytes(&mut self, bytes: &[u8], now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                let line = mem::take(&mut self.partial);
                self.on_line(&line, now, &mut out);
            } else {
                if self.partial.len() >= MAX_LINE_BYTES {
                    let line = mem::take(&mut self.partial);
                    self.on_line(&line, now, &mut out);
                }
                self.partial.push(byte);
            }
            if out.iter().any(|a| matches!(a, Action::Disconnect(_))) {
                return self.track(out);
            }
        }
        self.dispatch(now, &mut out);
        self.fill_stream(&mut out);
        self.report_progress(&mut out);
        self.track(out)
    }

    /// Timers: identification, polling, timeouts and the watchdog. Call it
    /// regularly, at least as often as the poll interval.
    pub(crate) fn on_tick(&mut self, now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        match self.phase {
            Phase::Identifying { since } => {
                let waited = now.saturating_duration_since(since);
                if waited >= BANNER_WAIT + VERSION_WAIT {
                    out.push(Action::Disconnect(String::from(
                        "No GRBL 1.1 controller answered. Check the port and baud rate.",
                    )));
                    return self.track(out);
                }
                if waited >= BANNER_WAIT && !self.asked_version && self.in_flight.is_empty() {
                    self.asked_version = true;
                    self.send_line(ReadOnlyRequest::BuildInfo, now, &mut out);
                }
            }
            Phase::Ready { since } => {
                let interval = self.current_poll_interval();
                let due = self
                    .last_poll
                    .is_none_or(|t| now.saturating_duration_since(t) >= interval);
                if due {
                    self.poll_now(now, &mut out);
                }
                let heard = self.last_status.unwrap_or(since);
                // GRBL sends no status while homing.
                if !self.silent
                    && !self.is_homing()
                    && now.saturating_duration_since(heard) >= STATUS_SILENCE
                {
                    self.silent = true;
                    out.push(Action::Emit(DeviceEvent::Unresponsive));
                }
                self.job_timers(now, &mut out);
            }
        }
        self.command_timeouts(now, &mut out);
        self.dispatch(now, &mut out);
        self.fill_stream(&mut out);
        self.report_progress(&mut out);
        self.track(out)
    }

    /// Keeps the snapshot in step with everything emitted.
    fn track(&mut self, out: Vec<Action>) -> Vec<Action> {
        for action in &out {
            if let Action::Emit(event) = action {
                self.snapshot.apply(event);
            }
        }
        out
    }

    fn is_ready(&self) -> bool {
        matches!(self.phase, Phase::Ready { .. })
    }

    fn on_line(&mut self, raw: &[u8], now: Instant, out: &mut Vec<Action>) {
        let text = String::from_utf8_lossy(raw);
        let line = text.trim_end_matches('\r');
        if line.trim().is_empty() {
            return;
        }
        let response = parse_line(line);
        out.push(Action::Emit(DeviceEvent::Received {
            line: line.to_owned(),
            response: response.clone(),
        }));
        let collecting = self.collecting();

        match response {
            Response::Status(report) => {
                self.state = Some(report.state);
                if report.wco.is_some() {
                    self.last_wco = report.wco;
                }
                self.last_status = Some(now);
                if self.silent {
                    self.silent = false;
                    out.push(Action::Emit(DeviceEvent::Responsive));
                }
                let positions = report.positions(self.last_wco);
                out.push(Action::Emit(DeviceEvent::Status {
                    report: report.clone(),
                    positions,
                }));
                self.on_job_status(&report, now, out);
            }
            Response::Welcome { version } => self.on_welcome(version, now, out),
            Response::Ok => self.complete(None, now, out),
            Response::Error(code) => self.complete(Some(code), now, out),
            Response::Alarm(code) => {
                out.push(Action::Emit(DeviceEvent::Alarm(code)));
                self.job_event(JobEvent::Alarm(code), now, out);
                // A homing failure is an alarm, and `$H` is then never answered.
                if self.is_homing() {
                    self.in_flight
                        .retain(|p| !matches!(p, Pending::Home { .. }));
                    out.push(control_failed("Home", describe_alarm(code)));
                }
            }
            Response::Message(text) => out.push(Action::Emit(DeviceEvent::Message(text))),
            Response::Setting { number, value }
                if collecting == Some(ReadOnlyRequest::ViewSettings) =>
            {
                if let Err(err) = self.collected.settings.insert(number, &value) {
                    self.collected.setting_errors.push(err);
                }
            }
            Response::Version(data) if collecting == Some(ReadOnlyRequest::BuildInfo) => {
                match parse_version(&data) {
                    Ok(version) => {
                        if matches!(self.phase, Phase::Identifying { .. }) {
                            if !is_supported_version(&version.version) {
                                out.push(unsupported(&version.version));
                                return;
                            }
                            self.become_ready(version.version.clone(), now, out);
                            self.enqueue(ReadOnlyRequest::ViewSettings);
                            self.enqueue(ReadOnlyRequest::ParserState);
                        }
                        self.collected.version = Some(version);
                    }
                    Err(err) => out.push(warning(line, &err)),
                }
            }
            Response::Options(data) if collecting == Some(ReadOnlyRequest::BuildInfo) => {
                match parse_options(&data) {
                    Ok(options) => self.collected.options = Some(options),
                    Err(err) => out.push(warning(line, &err)),
                }
            }
            Response::ParserState(data) if collecting == Some(ReadOnlyRequest::ParserState) => {
                match parse_parser_state(&data) {
                    Ok(state) => self.collected.parser_state = Some(state),
                    Err(err) => out.push(warning(line, &err)),
                }
            }
            // Shown in the console through `Received`; nothing else to do.
            _ => {}
        }
    }

    /// GRBL printed its welcome message: it just started or was reset, and
    /// discarded anything it had not answered yet.
    fn on_welcome(&mut self, version: String, now: Instant, out: &mut Vec<Action>) {
        let was_ready = self.is_ready();
        for pending in &self.in_flight {
            let control = match pending {
                Pending::Unlock { .. } => "Unlock",
                Pending::Home { .. } => "Home",
                _ => continue,
            };
            out.push(control_failed(
                control,
                String::from("interrupted by a reset"),
            ));
        }
        self.in_flight.clear();
        self.queue.clear();
        self.collected = Collected::default();
        self.state = None;
        self.last_wco = None;
        if !is_supported_version(&version) {
            out.push(unsupported(&version));
            return;
        }
        if was_ready {
            out.push(Action::Emit(DeviceEvent::ControllerReset));
        }
        if matches!(
            self.job,
            JobState::Stopping {
                stage: StopStage::Resetting,
                ..
            }
        ) {
            self.reset_banner_seen = true;
            self.stage_since = now;
        }
        self.job_event(JobEvent::ControllerReset, now, out);
        self.become_ready(version, now, out);
        self.enqueue(ReadOnlyRequest::BuildInfo);
        self.enqueue(ReadOnlyRequest::ViewSettings);
        self.enqueue(ReadOnlyRequest::ParserState);
    }

    fn become_ready(&mut self, version: String, now: Instant, out: &mut Vec<Action>) {
        self.phase = Phase::Ready { since: now };
        self.last_poll = None;
        self.last_status = None;
        self.silent = false;
        out.push(Action::Emit(DeviceEvent::ConnectionChanged(
            ConnectionState::Ready { version },
        )));
    }

    fn collecting(&self) -> Option<ReadOnlyRequest> {
        match self.in_flight.front() {
            Some(Pending::Request { request, .. }) => Some(*request),
            _ => None,
        }
    }

    /// `ok` or `error:N` answers the oldest line command in flight. An
    /// answer with nothing in flight (e.g. after a timeout) is only shown in
    /// the console.
    fn complete(&mut self, error: Option<u8>, now: Instant, out: &mut Vec<Action>) {
        let Some(pending) = self.in_flight.pop_front() else {
            return;
        };
        match pending {
            Pending::Request { request, .. } => self.complete_request(request, error, out),
            Pending::JobLine => {
                // Lines of a job that is already stopping have no streamer.
                let Some(streamer) = self.streamer.as_mut() else {
                    return;
                };
                let answered = match error {
                    Some(_) => streamer.on_error(),
                    None => streamer.on_ack(),
                };
                match (answered, error) {
                    (Ok(line), Some(code)) => {
                        self.job_event(JobEvent::Error { line, code }, now, out)
                    }
                    (Ok(_), None) => {}
                    (Err(err), _) => out.push(Action::Emit(DeviceEvent::Warning(err.to_string()))),
                }
            }
            Pending::LaserOff => match error {
                None => self.job_event(JobEvent::StopFinished, now, out),
                Some(code) => self.job_event(
                    JobEvent::StopFailed(format!("M5 was refused with error:{code}")),
                    now,
                    out,
                ),
            },
            Pending::Unlock { .. } => self.complete_control("Unlock", error, now, out),
            Pending::Home { .. } => self.complete_control("Home", error, now, out),
            Pending::Jog { .. } => {
                if let Some(code) = error {
                    out.push(control_failed("Jog", describe_error(code)));
                }
            }
        }
    }

    /// `$X` or `$H` answered. Either one clears the alarm a stop sequence
    /// waits on, so the sequence goes on to `M5`.
    fn complete_control(
        &mut self,
        control: &'static str,
        error: Option<u8>,
        now: Instant,
        out: &mut Vec<Action>,
    ) {
        if let Some(code) = error {
            out.push(control_failed(control, describe_error(code)));
            return;
        }
        out.push(Action::Emit(DeviceEvent::ControlCompleted { control }));
        if matches!(
            self.job,
            JobState::Stopping {
                stage: StopStage::AwaitingUnlock,
                ..
            }
        ) {
            self.job_event(JobEvent::StopAdvanced(StopStage::LaserOff), now, out);
        }
    }

    fn command_timeouts(&mut self, now: Instant, out: &mut Vec<Action>) {
        match self.in_flight.front().copied() {
            Some(Pending::Request { request, sent_at })
                if now.saturating_duration_since(sent_at) >= REQUEST_TIMEOUT =>
            {
                self.in_flight.pop_front();
                self.collected = Collected::default();
                out.push(Action::Emit(DeviceEvent::RequestTimedOut(request)));
            }
            Some(Pending::Unlock { sent_at })
                if now.saturating_duration_since(sent_at) >= REQUEST_TIMEOUT =>
            {
                self.in_flight.pop_front();
                out.push(control_failed("Unlock", String::from("no answer to $X")));
            }
            Some(Pending::Jog { sent_at })
                if now.saturating_duration_since(sent_at) >= REQUEST_TIMEOUT =>
            {
                self.in_flight.pop_front();
                out.push(control_failed("Jog", String::from("no answer to the jog")));
            }
            Some(Pending::Home { deadline }) if now >= deadline => {
                self.in_flight.pop_front();
                out.push(control_failed(
                    "Home",
                    String::from("homing took too long; stopping the machine"),
                ));
                self.job_event(JobEvent::StopRequested, now, out);
            }
            _ => {}
        }
    }

    fn is_homing(&self) -> bool {
        self.in_flight
            .iter()
            .any(|p| matches!(p, Pending::Home { .. }))
    }

    fn command_pending(&self) -> Result<(), Vec<String>> {
        let busy = self.in_flight.iter().any(|p| {
            matches!(
                p,
                Pending::Unlock { .. } | Pending::Home { .. } | Pending::Jog { .. }
            )
        });
        if busy {
            Err(vec![String::from(
                "an unlock, homing or jog is still in progress",
            )])
        } else {
            Ok(())
        }
    }

    /// One jog in flight at most, so a held button cannot queue a runaway.
    fn jog(
        &mut self,
        step: JogMove,
        profile: &MachineProfile,
        now: Instant,
        out: &mut Vec<Action>,
    ) -> Result<(), Vec<String>> {
        self.snapshot.can_jog().map_err(|reason| vec![reason])?;
        self.command_pending()?;
        let fresh = self
            .last_status
            .is_some_and(|t| now.saturating_duration_since(t) <= 2 * self.poll_interval);
        let position = self.snapshot.machine_position_mm().filter(|_| fresh);
        let Some([x, y, _]) = position else {
            return Err(vec![String::from(
                "the head position is out of date; waiting for a status report",
            )]);
        };
        let planned = plan_jog(step, [x, y], profile).map_err(|err| vec![err.to_string()])?;
        self.in_flight.push_back(Pending::Jog { sent_at: now });
        out.push(Action::Send(Outgoing::Jog(planned.line)));
        Ok(())
    }

    fn jog_cancel(&mut self, out: &mut Vec<Action>) -> Result<(), Vec<String>> {
        let jogging = self.state == Some(MachineState::Jog)
            || self
                .in_flight
                .iter()
                .any(|p| matches!(p, Pending::Jog { .. }));
        if jogging {
            out.push(Action::Send(Outgoing::Realtime(Realtime::JogCancel)));
            Ok(())
        } else {
            Err(vec![String::from("the machine is not jogging")])
        }
    }

    fn unlock(&mut self, now: Instant, out: &mut Vec<Action>) -> Result<(), Vec<String>> {
        self.snapshot.can_unlock().map_err(|reason| vec![reason])?;
        self.command_pending()?;
        self.in_flight.push_back(Pending::Unlock { sent_at: now });
        out.push(Action::Send(Outgoing::Unlock));
        Ok(())
    }

    fn home(&mut self, now: Instant, out: &mut Vec<Action>) -> Result<(), Vec<String>> {
        self.snapshot.can_home().map_err(|reason| vec![reason])?;
        self.command_pending()?;
        let deadline = now + homing_timeout(self.snapshot.settings.as_ref());
        self.in_flight.push_back(Pending::Home { deadline });
        out.push(Action::Send(Outgoing::Home));
        Ok(())
    }

    fn complete_request(
        &mut self,
        request: ReadOnlyRequest,
        error: Option<u8>,
        out: &mut Vec<Action>,
    ) {
        let collected = mem::take(&mut self.collected);
        if let Some(code) = error {
            out.push(Action::Emit(DeviceEvent::CommandFailed { request, code }));
            return;
        }
        let event = match request {
            ReadOnlyRequest::ViewSettings => Some(DeviceEvent::SettingsLoaded {
                settings: collected.settings,
                errors: collected.setting_errors,
            }),
            ReadOnlyRequest::BuildInfo => collected.version.map(|version| {
                DeviceEvent::BuildInfoLoaded(BuildInfo {
                    version,
                    options: collected.options,
                })
            }),
            ReadOnlyRequest::ParserState => {
                collected.parser_state.map(DeviceEvent::ParserStateLoaded)
            }
            ReadOnlyRequest::StatusQuery => None,
        };
        out.extend(event.map(Action::Emit));
    }

    fn enqueue(&mut self, request: ReadOnlyRequest) {
        let in_flight = self.collecting() == Some(request);
        if !in_flight && !self.queue.contains(&request) {
            self.queue.push_back(request);
        }
    }

    /// Sends the next queued line command GRBL accepts in its current state,
    /// if nothing is in flight and no job owns the line.
    fn dispatch(&mut self, now: Instant, out: &mut Vec<Action>) {
        if !self.in_flight.is_empty() || !self.is_ready() || self.job_owns_line() {
            return;
        }
        let state = self.state;
        let Some(index) = self.queue.iter().position(|r| r.accepted_in(state)) else {
            return;
        };
        if let Some(request) = self.queue.remove(index) {
            self.send_line(request, now, out);
        }
    }

    fn send_line(&mut self, request: ReadOnlyRequest, now: Instant, out: &mut Vec<Action>) {
        self.in_flight.push_back(Pending::Request {
            request,
            sent_at: now,
        });
        out.push(Action::Send(Outgoing::ReadOnly(request)));
    }

    fn poll_now(&mut self, now: Instant, out: &mut Vec<Action>) {
        self.last_poll = Some(now);
        out.push(Action::Send(Outgoing::ReadOnly(
            ReadOnlyRequest::StatusQuery,
        )));
    }

    fn current_poll_interval(&self) -> Duration {
        let confirming =
            self.hold_requested.is_some() || matches!(self.job, JobState::Stopping { .. });
        if confirming {
            self.poll_interval.min(FAST_POLL)
        } else {
            self.poll_interval
        }
    }

    /// While a job streams or stops, its lines (and `M5`) are the only line
    /// commands, so every `ok` is matched to the right line.
    fn job_owns_line(&self) -> bool {
        match &self.job {
            JobState::Running | JobState::Held => true,
            JobState::Stopping { stage, .. } => *stage != StopStage::AwaitingUnlock,
            JobState::Idle | JobState::Faulted(_) | JobState::Done => false,
        }
    }

    fn start(
        &mut self,
        job: JobRequest,
        now: Instant,
        out: &mut Vec<Action>,
    ) -> Result<(), Vec<String>> {
        match self.job {
            JobState::Idle | JobState::Done => {}
            JobState::Faulted(_) => {
                return Err(vec![String::from("acknowledge the last fault first")]);
            }
            _ => return Err(vec![String::from("a job is already running")]),
        }
        self.command_pending()?;
        let summary = scan_job(&job.lines).map_err(|err| vec![err.to_string()])?;
        preflight(&self.snapshot, &summary, job.kind, &job.profile)
            .map_err(|errors| errors.iter().map(ToString::to_string).collect::<Vec<_>>())?;
        let options = self
            .snapshot
            .build_info
            .as_ref()
            .and_then(|info| info.options.as_ref());
        let streamer = Streamer::new(job.lines, job.mode, rx_capacity(options))
            .map_err(|err| vec![err.to_string()])?;
        self.streamer = Some(streamer);
        self.reported_progress = None;
        self.job_event(JobEvent::Start, now, out);
        Ok(())
    }

    fn hold(&mut self, now: Instant, out: &mut Vec<Action>) -> Result<(), Vec<String>> {
        if self.job != JobState::Running {
            return Err(vec![String::from("there is no running job to pause")]);
        }
        if self.hold_requested.is_none() {
            self.hold_requested = Some(now);
            out.push(Action::Send(Outgoing::Realtime(Realtime::FeedHold)));
            self.poll_now(now, out);
        }
        Ok(())
    }

    fn resume(&mut self, out: &mut Vec<Action>) -> Result<(), Vec<String>> {
        if self.job != JobState::Held {
            return Err(vec![String::from("only a paused job can be resumed")]);
        }
        if self.state != Some(MachineState::Hold(Some(0))) {
            return Err(vec![format!(
                "GRBL is {:?}; it can only resume from a completed hold",
                self.state
            )]);
        }
        self.resume_requested = true;
        out.push(Action::Send(Outgoing::Realtime(Realtime::CycleStart)));
        Ok(())
    }

    /// Feeds `event` to the job state machine and acts on the new state.
    fn job_event(&mut self, event: JobEvent, now: Instant, out: &mut Vec<Action>) {
        let next = match transition(&self.job, event) {
            Ok(next) => next,
            Err(err) => {
                out.push(Action::Emit(DeviceEvent::Warning(err.to_string())));
                return;
            }
        };
        if next == self.job {
            return;
        }
        self.report_progress(out);
        self.job = next.clone();
        out.push(Action::Emit(DeviceEvent::JobStateChanged(next)));
        self.enter_job_state(now, out);
    }

    fn enter_job_state(&mut self, now: Instant, out: &mut Vec<Action>) {
        match self.job {
            JobState::Running | JobState::Held => {}
            JobState::Stopping { stage, .. } => {
                self.stage_since = now;
                match stage {
                    StopStage::Holding => {
                        self.end_streaming();
                        out.push(Action::Send(Outgoing::Realtime(Realtime::FeedHold)));
                        self.poll_now(now, out);
                    }
                    StopStage::Resetting => {
                        self.reset_banner_seen = false;
                        out.push(Action::Send(Outgoing::Realtime(Realtime::SoftReset)));
                    }
                    // `$X` is only ever sent on the user's confirmation.
                    StopStage::AwaitingUnlock => {}
                    StopStage::LaserOff => {
                        self.in_flight.push_back(Pending::LaserOff);
                        out.push(Action::Send(Outgoing::LaserOff));
                    }
                }
            }
            JobState::Idle | JobState::Done | JobState::Faulted(_) => {
                self.end_streaming();
                // Jogging needs the laser's modal state as the job left it.
                self.enqueue(ReadOnlyRequest::ParserState);
                // After an alarm GRBL drops what it has not executed, so
                // those lines will never be answered.
                self.in_flight
                    .retain(|p| matches!(p, Pending::Request { .. }));
            }
        }
    }

    fn end_streaming(&mut self) {
        self.streamer = None;
        self.hold_requested = None;
        self.resume_requested = false;
    }

    fn on_job_status(&mut self, report: &StatusReport, now: Instant, out: &mut Vec<Action>) {
        let state = report.state;
        match self.job {
            JobState::Running => {
                if self.hold_requested.is_some() {
                    // `A:` is only sent along with `Ov:`; without it the
                    // laser output is not known yet.
                    if state == MachineState::Hold(Some(0)) && report.overrides.is_some() {
                        if laser_output_on(report) {
                            self.job_event(
                                JobEvent::Abort(FaultReason::LaserOnDuringHold),
                                now,
                                out,
                            );
                        } else {
                            self.hold_requested = None;
                            self.job_event(JobEvent::HoldConfirmed, now, out);
                        }
                    }
                } else if state == MachineState::Idle
                    && self.streamer.as_ref().is_some_and(Streamer::is_finished)
                {
                    self.job_event(JobEvent::Finished, now, out);
                }
            }
            JobState::Held => {
                if matches!(state, MachineState::Hold(_)) {
                    if report.overrides.is_some() && laser_output_on(report) {
                        self.job_event(JobEvent::Abort(FaultReason::LaserOnDuringHold), now, out);
                    }
                } else {
                    // Resumed by us or by GRBL's cycle start input.
                    self.resume_requested = false;
                    self.job_event(JobEvent::Resumed, now, out);
                    if state == MachineState::Idle
                        && self.streamer.as_ref().is_some_and(Streamer::is_finished)
                    {
                        self.job_event(JobEvent::Finished, now, out);
                    }
                }
            }
            JobState::Stopping {
                stage: StopStage::Holding,
                ..
            } if is_stopped(state) => {
                self.job_event(JobEvent::StopAdvanced(StopStage::Resetting), now, out);
            }
            JobState::Stopping {
                stage: StopStage::Resetting,
                ..
            } if self.reset_banner_seen => {
                // GRBL refuses G-code in alarm, so `M5` waits for an unlock.
                let next = if state == MachineState::Alarm {
                    StopStage::AwaitingUnlock
                } else {
                    StopStage::LaserOff
                };
                self.job_event(JobEvent::StopAdvanced(next), now, out);
            }
            _ => {}
        }
    }

    fn job_timers(&mut self, now: Instant, out: &mut Vec<Action>) {
        if let Some(asked) = self.hold_requested
            && self.job == JobState::Running
            && now.saturating_duration_since(asked) >= HOLD_CONFIRM_WAIT
        {
            self.job_event(JobEvent::Abort(FaultReason::HoldNotConfirmed), now, out);
            return;
        }
        let JobState::Stopping { stage, .. } = self.job else {
            return;
        };
        let waited = now.saturating_duration_since(self.stage_since);
        match stage {
            StopStage::Holding if waited >= STOP_HOLD_WAIT => {
                self.job_event(JobEvent::StopAdvanced(StopStage::Resetting), now, out);
            }
            StopStage::Resetting if waited >= STOP_RESET_WAIT => {
                let reason = if self.reset_banner_seen {
                    "no status report after the reset; the laser state is unknown, switch the machine off"
                } else {
                    "the controller did not confirm the reset; the laser may still be on, switch the machine off"
                };
                self.job_event(JobEvent::StopFailed(reason.to_owned()), now, out);
            }
            StopStage::LaserOff if waited >= REQUEST_TIMEOUT => {
                self.job_event(
                    JobEvent::StopFailed(String::from("M5 got no answer")),
                    now,
                    out,
                );
            }
            _ => {}
        }
    }

    /// Sends every job line that fits GRBL's receive buffer, once no other
    /// line command is waiting for its answer.
    fn fill_stream(&mut self, out: &mut Vec<Action>) {
        if !matches!(self.job, JobState::Running | JobState::Held) || !self.is_ready() {
            return;
        }
        if self.in_flight.iter().any(|p| *p != Pending::JobLine) {
            return;
        }
        let Some(streamer) = self.streamer.as_mut() else {
            return;
        };
        for line in streamer.take_ready() {
            self.in_flight.push_back(Pending::JobLine);
            out.push(Action::Send(Outgoing::JobLine(line)));
        }
    }

    /// At most one progress event per call, and only when it changed.
    fn report_progress(&mut self, out: &mut Vec<Action>) {
        let Some(progress) = self.streamer.as_ref().map(Streamer::progress) else {
            return;
        };
        if self.reported_progress != Some(progress) {
            self.reported_progress = Some(progress);
            out.push(Action::Emit(DeviceEvent::JobProgress(progress)));
        }
    }
}

/// A generous limit for one homing cycle. GRBL searches at most 1.5 x max
/// travel per axis at the seek rate `$25`, Z first and then X and Y together
/// (`HOMING_CYCLE_0`/`_1` in `config.h`), then locates slowly over a few mm.
/// Twice that search time plus 10 s, at most 10 minutes.
fn homing_timeout(settings: Option<&GrblSettings>) -> Duration {
    let Some(settings) = settings else {
        return HOMING_FALLBACK;
    };
    let (Some(travel), Some(seek)) = (settings.max_travel(), settings.homing_seek_rate()) else {
        return HOMING_FALLBACK;
    };
    if seek <= 0.0 {
        return HOMING_FALLBACK;
    }
    let search_mm = 1.5 * (travel[0].max(travel[1]) + travel[2]);
    let seconds = search_mm / seek * 60.0;
    Duration::from_secs_f64((2.0 * seconds + 10.0).min(600.0))
}

fn control_failed(control: &'static str, reason: String) -> Action {
    Action::Emit(DeviceEvent::ControlFailed { control, reason })
}

fn describe_error(code: u8) -> String {
    match error_info(code) {
        Some(info) => format!("error:{code} ({})", info.description),
        None => format!("error:{code}"),
    }
}

fn describe_alarm(code: u8) -> String {
    match alarm_info(code) {
        Some(info) => format!("ALARM:{code} ({})", info.description),
        None => format!("ALARM:{code}"),
    }
}

/// Spindle (laser) output on, from the `A:` field: `S` for M3, `C` for M4.
fn laser_output_on(report: &StatusReport) -> bool {
    report
        .accessories
        .as_deref()
        .is_some_and(|a| a.contains('S') || a.contains('C'))
}

/// Not moving: safe to reset without losing position. Door 0 and 1 are the
/// stopped door states (GRBL `interface.md`, "Machine State").
fn is_stopped(state: MachineState) -> bool {
    matches!(
        state,
        MachineState::Idle
            | MachineState::Alarm
            | MachineState::Sleep
            | MachineState::Check
            | MachineState::Hold(Some(0))
            | MachineState::Door(Some(0 | 1))
    )
}

fn unsupported(version: &str) -> Action {
    Action::Disconnect(format!(
        "GRBL {version} is not supported; PhotonForge needs GRBL 1.1."
    ))
}

fn warning(line: &str, err: &ParseError) -> Action {
    Action::Emit(DeviceEvent::Warning(format!(
        "Could not read {line:?}: {err}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::JobKind;
    use crate::protocol::status::ReportedPosition;
    use crate::stream::StreamMode;
    use ReadOnlyRequest::{BuildInfo as I, ParserState as G, StatusQuery as Q, ViewSettings as S};
    use pf_core::MachineProfile;

    const POLL: Duration = Duration::from_millis(200);
    const BANNER: &str = "Grbl 1.1h ['$' for help]\r\n";
    const VERSION_REPLY: &str = "[VER:1.1h.20190825:]\r\n[OPT:V,15,128]\r\nok\r\n";
    const SETTINGS_REPLY: &str = "$10=1\r\n$30=1000\r\n$32=1\r\n$130=300.000\r\nok\r\n";
    const GC_REPLY: &str = "[GC:G0 G54 G17 G21 G90 G94 M5 M9 T0 F0 S0]\r\nok\r\n";
    const IDLE: &str =
        "<Idle|MPos:0.000,0.000,0.000|FS:0,0|WCO:0.000,0.000,0.000|Ov:100,100,100>\r\n";

    fn ms(t0: Instant, millis: u64) -> Instant {
        t0 + Duration::from_millis(millis)
    }

    fn sends(actions: &[Action]) -> Vec<ReadOnlyRequest> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Send(Outgoing::ReadOnly(r)) => Some(*r),
                _ => None,
            })
            .collect()
    }

    /// Everything sent, as the bytes written.
    fn written(actions: &[Action]) -> Vec<u8> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Send(outgoing) => Some(outgoing.bytes()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    fn job_lines_sent(actions: &[Action]) -> Vec<String> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Send(Outgoing::JobLine(line)) => Some(line.as_str().to_owned()),
                _ => None,
            })
            .collect()
    }

    fn events(actions: &[Action]) -> Vec<DeviceEvent> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Emit(e) if !matches!(e, DeviceEvent::Received { .. }) => Some(e.clone()),
                _ => None,
            })
            .collect()
    }

    fn job_states(actions: &[Action]) -> Vec<JobState> {
        events(actions)
            .into_iter()
            .filter_map(|e| match e {
                DeviceEvent::JobStateChanged(state) => Some(state),
                _ => None,
            })
            .collect()
    }

    fn disconnect_reason(actions: &[Action]) -> Option<String> {
        actions.iter().find_map(|a| match a {
            Action::Disconnect(reason) => Some(reason.clone()),
            _ => None,
        })
    }

    fn ready(version: &str) -> DeviceEvent {
        DeviceEvent::ConnectionChanged(ConnectionState::Ready {
            version: version.into(),
        })
    }

    fn flight(s: &Session) -> Option<ReadOnlyRequest> {
        s.collecting()
    }

    /// A session that saw the banner and answered its startup reads.
    fn ready_session(t0: Instant) -> Session {
        let mut s = Session::new(POLL, t0);
        s.on_bytes(BANNER.as_bytes(), t0);
        s.on_bytes(VERSION_REPLY.as_bytes(), t0);
        s.on_bytes(SETTINGS_REPLY.as_bytes(), t0);
        s.on_bytes(GC_REPLY.as_bytes(), t0);
        assert!(s.in_flight.is_empty() && s.queue.is_empty());
        s
    }

    /// Ready and idle, with everything the pre-flight checks need.
    fn job_session(t0: Instant) -> Session {
        let mut s = ready_session(t0);
        s.on_bytes(IDLE.as_bytes(), t0);
        s
    }

    fn job(lines: &[String]) -> ControlRequest {
        ControlRequest::Start(JobRequest {
            kind: JobKind::Job,
            lines: lines.to_vec(),
            profile: MachineProfile::default(),
            mode: StreamMode::CharacterCounting,
        })
    }

    /// `n` cutting moves of about 20 bytes each, ending with `M5`.
    fn square_job(n: usize) -> Vec<String> {
        let mut lines = vec![String::from("G21 G90"), String::from("M4 S0")];
        for i in 0..n {
            lines.push(format!("G1 X{}.5 Y{}.25 F1200 S500", i % 100, i % 50));
        }
        lines.push(String::from("M5"));
        lines
    }

    fn stopping(stage: StopStage, fault: Option<FaultReason>) -> JobState {
        JobState::Stopping { stage, fault }
    }

    #[test]
    fn banner_identifies_and_starts_the_reads() {
        let t0 = Instant::now();
        let mut s = Session::new(POLL, t0);
        let out = s.on_bytes(BANNER.as_bytes(), t0);
        assert_eq!(events(&out), vec![ready("1.1h")]);
        // One line command at a time: `$I` first, the rest wait.
        assert_eq!(sends(&out), vec![I]);

        let out = s.on_bytes(VERSION_REPLY.as_bytes(), ms(t0, 10));
        assert!(matches!(
            events(&out)[..],
            [DeviceEvent::BuildInfoLoaded(ref info)]
                if info.version.version == "1.1h"
                    && info.options.as_ref().map(|o| o.rx_buffer_bytes) == Some(128)
        ));
        assert_eq!(sends(&out), vec![S]);

        let out = s.on_bytes(SETTINGS_REPLY.as_bytes(), ms(t0, 20));
        match &events(&out)[..] {
            [DeviceEvent::SettingsLoaded { settings, errors }] => {
                assert_eq!(settings.len(), 4);
                assert_eq!(settings.laser_mode(), Some(true));
                assert!(errors.is_empty());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(sends(&out), vec![G]);

        let out = s.on_bytes(GC_REPLY.as_bytes(), ms(t0, 30));
        assert!(matches!(
            events(&out)[..],
            [DeviceEvent::ParserStateLoaded(_)]
        ));
        assert!(sends(&out).is_empty());
        // Polling starts on the next tick.
        assert_eq!(sends(&s.on_tick(ms(t0, 40))), vec![Q]);
    }

    #[test]
    fn without_a_banner_the_version_is_asked_with_dollar_i() {
        let t0 = Instant::now();
        let mut s = Session::new(POLL, t0);
        assert!(s.on_tick(ms(t0, 1_999)).is_empty());
        assert_eq!(sends(&s.on_tick(ms(t0, 2_000))), vec![I]);
        // Not again while waiting.
        assert!(sends(&s.on_tick(ms(t0, 2_500))).is_empty());

        let out = s.on_bytes(VERSION_REPLY.as_bytes(), ms(t0, 2_600));
        let ev = events(&out);
        assert_eq!(ev[0], ready("1.1h"));
        assert!(matches!(ev[1], DeviceEvent::BuildInfoLoaded(_)));
        // `$I` is answered, so only `$$` and `$G` remain.
        assert_eq!(sends(&out), vec![S]);
        s.on_bytes(SETTINGS_REPLY.as_bytes(), ms(t0, 2_700));
        assert!(s.queue.is_empty());
        assert_eq!(flight(&s), Some(G));
    }

    #[test]
    fn a_silent_device_is_given_up() {
        let t0 = Instant::now();
        let mut s = Session::new(POLL, t0);
        s.on_tick(ms(t0, 2_000));
        // Something that is not GRBL answers `$I` with junk.
        s.on_bytes(b"Marlin\r\nok\r\n", ms(t0, 2_100));
        assert!(disconnect_reason(&s.on_tick(ms(t0, 4_999))).is_none());
        let reason = disconnect_reason(&s.on_tick(ms(t0, 5_000))).unwrap();
        assert!(reason.contains("No GRBL 1.1"));
    }

    #[test]
    fn other_grbl_versions_are_refused() {
        let t0 = Instant::now();
        let mut s = Session::new(POLL, t0);
        let out = s.on_bytes(b"Grbl 0.9j ['$' for help]\r\n", t0);
        assert_eq!(
            disconnect_reason(&out).as_deref(),
            Some("GRBL 0.9j is not supported; PhotonForge needs GRBL 1.1.")
        );
        assert!(sends(&out).is_empty());

        let mut s = Session::new(POLL, t0);
        s.on_tick(ms(t0, 2_000));
        let out = s.on_bytes(b"[VER:1.0c.20161014:]\r\n", ms(t0, 2_100));
        assert!(disconnect_reason(&out).unwrap().contains("1.0c"));
    }

    #[test]
    fn lines_are_reassembled_across_any_split() {
        let t0 = Instant::now();
        // Each reply arrives after its request went out, as with real GRBL.
        let chunks = [BANNER, VERSION_REPLY, SETTINGS_REPLY];
        let mut whole = Session::new(POLL, t0);
        let mut split = Session::new(POLL, t0);
        let mut all_at_once = Vec::new();
        let mut one_by_one = Vec::new();
        for chunk in chunks {
            all_at_once.extend(events(&whole.on_bytes(chunk.as_bytes(), t0)));
            for byte in chunk.as_bytes() {
                one_by_one.extend(events(&split.on_bytes(&[*byte], t0)));
            }
        }
        assert_eq!(one_by_one, all_at_once);
        assert!(matches!(
            all_at_once[..],
            [
                DeviceEvent::ConnectionChanged(_),
                DeviceEvent::BuildInfoLoaded(_),
                DeviceEvent::SettingsLoaded { .. }
            ]
        ));
    }

    #[test]
    fn overlong_lines_are_cut_not_buffered_forever() {
        let t0 = Instant::now();
        let mut s = Session::new(POLL, t0);
        let out = s.on_bytes(&[b'x'; 600], t0);
        let received = out
            .iter()
            .filter(|a| matches!(a, Action::Emit(DeviceEvent::Received { .. })))
            .count();
        assert_eq!(received, 2);
        assert_eq!(s.partial.len(), 600 - 2 * MAX_LINE_BYTES);
    }

    #[test]
    fn user_requests_queue_behind_the_one_in_flight() {
        let t0 = Instant::now();
        let mut s = ready_session(t0);
        assert_eq!(sends(&s.request(G, t0)), vec![G]);
        // Queued, and duplicates are dropped.
        assert!(sends(&s.request(S, t0)).is_empty());
        assert!(sends(&s.request(S, t0)).is_empty());
        assert!(sends(&s.request(G, t0)).is_empty());
        assert_eq!(s.queue, [S]);
        // `?` never waits.
        assert_eq!(sends(&s.request(Q, t0)), vec![Q]);
        assert_eq!(sends(&s.on_bytes(GC_REPLY.as_bytes(), t0)), vec![S]);
    }

    #[test]
    fn requests_wait_for_a_state_grbl_accepts_them_in() {
        let t0 = Instant::now();
        let mut s = ready_session(t0);
        s.on_bytes(b"<Run|MPos:1.000,2.000,0.000|FS:1000,500>\r\n", t0);
        // `$$` and `$I` are refused while running; `$G` is not.
        assert!(sends(&s.request(S, t0)).is_empty());
        assert!(sends(&s.request(I, t0)).is_empty());
        assert_eq!(sends(&s.request(G, t0)), vec![G]);
        s.on_bytes(GC_REPLY.as_bytes(), t0);
        assert!(s.in_flight.is_empty());
        // Once idle, they go out in order.
        let out = s.on_bytes(b"<Idle|MPos:1.000,2.000,0.000|FS:0,0>\r\n", t0);
        assert_eq!(sends(&out), vec![S]);
    }

    #[test]
    fn errors_and_timeouts_free_the_line() {
        let t0 = Instant::now();
        let mut s = ready_session(t0);
        s.request(S, t0);
        s.request(G, t0);
        let out = s.on_bytes(b"error:8\r\n", ms(t0, 10));
        assert_eq!(
            events(&out),
            vec![DeviceEvent::CommandFailed {
                request: S,
                code: 8
            }]
        );
        assert_eq!(sends(&out), vec![G]);

        let out = s.on_tick(ms(t0, 10 + 4_999));
        assert!(!events(&out).contains(&DeviceEvent::RequestTimedOut(G)));
        let out = s.on_tick(ms(t0, 10 + 5_000));
        assert!(events(&out).contains(&DeviceEvent::RequestTimedOut(G)));
        assert!(s.in_flight.is_empty());
        // A late answer is only shown, not misattributed.
        let out = s.on_bytes(GC_REPLY.as_bytes(), ms(t0, 6_000));
        assert!(events(&out).is_empty());
    }

    #[test]
    fn polling_follows_the_interval() {
        let t0 = Instant::now();
        let mut s = ready_session(t0);
        let mut polls = 0;
        for step in 0..=100 {
            let now = ms(t0, step * 10);
            polls += sends(&s.on_tick(now)).iter().filter(|r| **r == Q).count();
            s.on_bytes(b"<Idle|MPos:0.000,0.000,0.000|FS:0,0>\r\n", now);
        }
        // Ticks every 10 ms for 1 s at a 200 ms interval.
        assert_eq!(polls, 6);
    }

    #[test]
    fn silence_is_reported_once_until_status_returns() {
        let t0 = Instant::now();
        let mut s = ready_session(t0);
        assert!(!events(&s.on_tick(ms(t0, 2_999))).contains(&DeviceEvent::Unresponsive));
        assert!(events(&s.on_tick(ms(t0, 3_000))).contains(&DeviceEvent::Unresponsive));
        assert!(!events(&s.on_tick(ms(t0, 4_000))).contains(&DeviceEvent::Unresponsive));
        let out = s.on_bytes(b"<Home|MPos:0.000,0.000,0.000|FS:0,0>\r\n", ms(t0, 4_100));
        assert_eq!(events(&out)[0], DeviceEvent::Responsive);
    }

    #[test]
    fn work_position_uses_the_last_wco() {
        let t0 = Instant::now();
        let mut s = ready_session(t0);
        s.on_bytes(
            b"<Idle|MPos:10.000,10.000,0.000|FS:0,0|WCO:2.000,3.000,0.000>\r\n",
            t0,
        );
        let out = s.on_bytes(b"<Idle|MPos:12.000,13.000,0.000|FS:0,0>\r\n", t0);
        match &events(&out)[..] {
            [DeviceEvent::Status { report, positions }] => {
                assert_eq!(
                    report.position,
                    ReportedPosition::Machine([12.0, 13.0, 0.0])
                );
                assert_eq!(positions.work, Some([10.0, 10.0, 0.0]));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_reset_drops_pending_requests_and_rereads() {
        let t0 = Instant::now();
        let mut s = ready_session(t0);
        s.request(S, t0);
        s.request(G, t0);
        let out = s.on_bytes(BANNER.as_bytes(), ms(t0, 100));
        let ev = events(&out);
        assert_eq!(ev, vec![DeviceEvent::ControllerReset, ready("1.1h")]);
        assert_eq!(sends(&out), vec![I]);
        assert_eq!(s.queue, [S, G]);
    }

    #[test]
    fn alarms_messages_and_bad_settings_are_reported() {
        let t0 = Instant::now();
        let mut s = ready_session(t0);
        let out = s.on_bytes(b"ALARM:1\r\n[MSG:Reset to continue]\r\n", t0);
        assert_eq!(
            events(&out),
            vec![
                DeviceEvent::Alarm(1),
                DeviceEvent::Message("Reset to continue".into())
            ]
        );
        s.request(S, t0);
        let out = s.on_bytes(b"$32=7\r\n$30=1000\r\nok\r\n", t0);
        match &events(&out)[..] {
            [DeviceEvent::SettingsLoaded { settings, errors }] => {
                assert_eq!(settings.len(), 1);
                assert_eq!(errors.len(), 1);
            }
            other => panic!("{other:?}"),
        }
        // Settings lines nobody asked for are not collected.
        let out = s.on_bytes(b"$32=1\r\nok\r\n", t0);
        assert!(events(&out).is_empty());
    }

    #[test]
    fn a_job_streams_within_the_buffer_and_finishes_when_idle() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        let lines = square_job(40);
        let out = s.control(job(&lines), t0);
        assert_eq!(job_states(&out), vec![JobState::Running]);
        let mut sent = job_lines_sent(&out);
        assert!(sent.len() > 2 && sent.len() < lines.len());
        // Every `ok` frees room for more, never past 127 bytes in flight.
        while sent.len() < lines.len() {
            let out = s.on_bytes(b"ok\r\n", t0);
            sent.extend(job_lines_sent(&out));
            let in_flight = s.streamer.as_ref().unwrap().in_flight_bytes();
            assert!(in_flight <= 127, "{in_flight}");
        }
        assert_eq!(sent, lines);
        for _ in 0..(s.in_flight.len() - 1) {
            s.on_bytes(b"ok\r\n", t0);
        }
        // All answered, but GRBL is still moving: not done yet.
        let out = s.on_bytes(b"ok\r\n<Run|MPos:1,1,0|FS:1200,500>\r\n", t0);
        assert!(job_states(&out).is_empty());
        assert!(
            events(&out).contains(&DeviceEvent::JobProgress(JobProgress {
                lines_sent: lines.len(),
                lines_acked: lines.len(),
                total: lines.len(),
            }))
        );
        let out = s.on_bytes(IDLE.as_bytes(), t0);
        assert_eq!(job_states(&out), vec![JobState::Done]);
        // A finished job can be acknowledged or followed by another.
        let out = s.control(ControlRequest::AcknowledgeFault, t0);
        assert_eq!(job_states(&out), vec![JobState::Idle]);
    }

    #[test]
    fn start_is_refused_unless_every_check_passes() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        s.on_bytes(b"<Run|MPos:0,0,0|FS:0,0>\r\n", t0);
        let out = s.control(job(&square_job(3)), t0);
        assert!(written(&out).is_empty());
        match &events(&out)[..] {
            [DeviceEvent::ControlRejected { control, reasons }] => {
                assert_eq!(*control, "Start");
                assert!(reasons[0].contains("Run"), "{reasons:?}");
            }
            other => panic!("{other:?}"),
        }
        let mut s = job_session(t0);
        for bad in [
            vec![String::from("G0 X1 Y1"), String::from("$H")],
            vec![String::from("G1 X1 Y1 S5000")],
            vec![String::from("G0 X400 Y1")],
            vec![format!("G0 X1 Y1 ({})", "x".repeat(130))],
        ] {
            let out = s.control(job(&bad), t0);
            assert!(written(&out).is_empty(), "{bad:?}");
            assert!(matches!(
                events(&out)[..],
                [DeviceEvent::ControlRejected { .. }]
            ));
        }
        // Not before the controller is identified.
        let mut s = Session::new(POLL, t0);
        let out = s.control(job(&square_job(1)), t0);
        assert!(written(&out).is_empty());
    }

    #[test]
    fn read_only_requests_wait_until_the_job_ends() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        let lines = square_job(2);
        s.control(job(&lines), t0);
        let out = s.request(S, t0);
        assert!(sends(&out).is_empty());
        // `?` is real-time and still goes out.
        assert_eq!(sends(&s.request(Q, t0)), vec![Q]);
        for _ in 0..lines.len() {
            s.on_bytes(b"ok\r\n", t0);
        }
        let out = s.on_bytes(IDLE.as_bytes(), t0);
        assert_eq!(job_states(&out), vec![JobState::Done]);
        assert_eq!(sends(&out), vec![S]);
    }

    #[test]
    fn pause_waits_for_the_laser_to_be_reported_off() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        s.control(job(&square_job(40)), t0);
        let out = s.control(ControlRequest::Hold, t0);
        assert_eq!(written(&out), b"!?");
        // Hold complete, but `A:` is not known without `Ov:`.
        let out = s.on_bytes(b"<Hold:0|MPos:1,1,0|FS:0,0>\r\n", ms(t0, 100));
        assert!(job_states(&out).is_empty());
        // Polling speeds up while confirming.
        assert_eq!(sends(&s.on_tick(ms(t0, 200))), vec![Q]);
        let out = s.on_bytes(
            b"<Hold:0|MPos:1,1,0|FS:0,0|Ov:100,100,100>\r\n",
            ms(t0, 210),
        );
        assert_eq!(job_states(&out), vec![JobState::Held]);

        // Resume only from a completed hold.
        s.on_bytes(b"<Hold:1|MPos:1,1,0|FS:0,0>\r\n", ms(t0, 300));
        let out = s.control(ControlRequest::Resume, ms(t0, 300));
        assert!(written(&out).is_empty());
        s.on_bytes(b"<Hold:0|MPos:1,1,0|FS:0,0>\r\n", ms(t0, 400));
        let out = s.control(ControlRequest::Resume, ms(t0, 400));
        assert_eq!(written(&out), b"~");
        // The laser coming back on with the motion is expected.
        let out = s.on_bytes(
            b"<Run|MPos:1,1,0|FS:1200,500|Ov:100,100,100|A:S>\r\n",
            ms(t0, 500),
        );
        assert_eq!(job_states(&out), vec![JobState::Running]);
    }

    #[test]
    fn a_laser_left_on_during_a_pause_stops_the_job() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        s.control(job(&square_job(40)), t0);
        s.control(ControlRequest::Hold, t0);
        let out = s.on_bytes(
            b"<Hold:0|MPos:1,1,0|FS:0,0|Ov:100,100,100|A:S>\r\n",
            ms(t0, 100),
        );
        assert_eq!(
            job_states(&out),
            vec![stopping(
                StopStage::Holding,
                Some(FaultReason::LaserOnDuringHold)
            )]
        );
        // The stop sequence goes on to the reset.
        let out = s.on_bytes(b"<Hold:0|MPos:1,1,0|FS:0,0>\r\n", ms(t0, 150));
        assert_eq!(written(&out), [0x18]);
    }

    #[test]
    fn an_unconfirmed_pause_stops_the_job() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        s.control(job(&square_job(40)), t0);
        s.control(ControlRequest::Hold, t0);
        assert!(job_states(&s.on_tick(ms(t0, 4_999))).is_empty());
        assert_eq!(
            job_states(&s.on_tick(ms(t0, 5_000))),
            vec![stopping(
                StopStage::Holding,
                Some(FaultReason::HoldNotConfirmed)
            )]
        );
    }

    #[test]
    fn stop_holds_resets_and_waits_for_an_unlock_after_an_alarm() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        s.control(job(&square_job(40)), t0);
        let out = s.control(ControlRequest::Stop, t0);
        assert_eq!(job_states(&out), vec![stopping(StopStage::Holding, None)]);
        assert_eq!(written(&out), b"!?");
        // Lines already sent are answered; nothing more is streamed.
        assert!(job_lines_sent(&s.on_bytes(b"ok\r\nok\r\n", t0)).is_empty());
        // Still decelerating: wait.
        assert!(written(&s.on_bytes(b"<Hold:1|MPos:1,1,0|FS:0,0>\r\n", t0)).is_empty());
        let out = s.on_bytes(b"<Hold:0|MPos:1,1,0|FS:0,0>\r\n", ms(t0, 100));
        assert_eq!(written(&out), [0x18]);
        let out = s.on_bytes(
            b"ALARM:3\r\n\r\nGrbl 1.1h ['$' for help]\r\n[MSG:Reset to continue]\r\n",
            ms(t0, 150),
        );
        assert!(job_states(&out).is_empty());
        let out = s.on_bytes(b"<Alarm|MPos:1,1,0|FS:0,0>\r\n", ms(t0, 200));
        assert_eq!(
            job_states(&out),
            vec![stopping(StopStage::AwaitingUnlock, None)]
        );
        // The reads queued by the reset may go out, but neither `$X` nor
        // `M5` is sent on its own.
        assert_eq!(written(&out), b"$I\n");
        for t in [1_000, 5_000, 10_000] {
            let bytes = written(&s.on_tick(ms(t0, t)));
            assert!(!bytes.windows(2).any(|w| w == b"$X" || w == b"M5"));
        }
    }

    #[test]
    fn stop_ends_with_m5_when_grbl_comes_back_idle() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        s.control(job(&square_job(3)), t0);
        s.control(ControlRequest::Stop, t0);
        // Already stopped: reset at once.
        let out = s.on_bytes(IDLE.as_bytes(), t0);
        assert_eq!(written(&out), [0x18]);
        s.on_bytes(BANNER.as_bytes(), t0);
        let out = s.on_bytes(IDLE.as_bytes(), t0);
        assert_eq!(written(&out), b"M5\n");
        assert_eq!(job_states(&out), vec![stopping(StopStage::LaserOff, None)]);
        let out = s.on_bytes(b"ok\r\n", t0);
        assert_eq!(job_states(&out), vec![JobState::Idle]);
        // The reads queued by the reset go out afterwards.
        assert_eq!(sends(&out), vec![I]);
    }

    #[test]
    fn stop_resets_anyway_and_fails_loudly_without_a_banner() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        s.control(ControlRequest::Stop, t0);
        assert!(written(&s.on_tick(ms(t0, 999))) != [0x18]);
        let out = s.on_tick(ms(t0, 1_000));
        assert!(written(&out).contains(&0x18));
        let out = s.on_tick(ms(t0, 3_000));
        match &job_states(&out)[..] {
            [JobState::Faulted(FaultReason::StopFailed(reason))] => {
                assert!(reason.contains("switch the machine off"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_error_mid_job_stops_it_and_faults() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        s.control(job(&square_job(40)), t0);
        s.on_bytes(b"ok\r\nok\r\n", t0);
        let out = s.on_bytes(b"error:20\r\n", t0);
        let fault = FaultReason::CommandError { line: 2, code: 20 };
        assert_eq!(
            job_states(&out),
            vec![stopping(StopStage::Holding, Some(fault.clone()))]
        );
        assert_eq!(written(&out), b"!?");
        s.on_bytes(IDLE.as_bytes(), t0);
        s.on_bytes(BANNER.as_bytes(), t0);
        s.on_bytes(IDLE.as_bytes(), t0);
        let out = s.on_bytes(b"ok\r\n", t0);
        assert_eq!(job_states(&out), vec![JobState::Faulted(fault)]);
    }

    #[test]
    fn alarms_and_unexpected_resets_fault_a_running_job() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        s.control(job(&square_job(40)), t0);
        let out = s.on_bytes(b"ALARM:1\r\n", t0);
        assert_eq!(
            job_states(&out),
            vec![JobState::Faulted(FaultReason::Alarm(1))]
        );
        // The job's lines are dropped; only the `$G` re-read is in flight.
        assert!(
            s.in_flight
                .iter()
                .all(|p| matches!(p, Pending::Request { request: G, .. }))
        );
        // Nothing resumes, and a new start waits for the acknowledgement.
        assert!(written(&s.on_bytes(b"ok\r\n", t0)).is_empty());
        let out = s.control(job(&square_job(1)), t0);
        assert!(matches!(
            events(&out)[..],
            [DeviceEvent::ControlRejected { .. }]
        ));

        let mut s = job_session(t0);
        s.control(job(&square_job(40)), t0);
        let out = s.on_bytes(BANNER.as_bytes(), t0);
        assert!(job_states(&out).contains(&JobState::Faulted(FaultReason::ControllerReset)));
        assert!(job_lines_sent(&s.on_bytes(IDLE.as_bytes(), t0)).is_empty());
    }

    const HOMING_SETTINGS: &str =
        "$22=1\r\n$25=600\r\n$30=1000\r\n$32=1\r\n$130=300\r\n$131=200\r\n$132=50\r\nok\r\n";
    const ALARM: &str =
        "<Alarm|MPos:0.000,0.000,0.000|FS:0,0|WCO:0.000,0.000,0.000|Ov:100,100,100>\r\n";

    /// Like `job_session`, with homing enabled on the controller.
    fn homing_session(t0: Instant) -> Session {
        let mut s = job_session(t0);
        s.request(S, t0);
        s.on_bytes(HOMING_SETTINGS.as_bytes(), t0);
        assert_eq!(
            s.snapshot.settings.as_ref().and_then(GrblSettings::homing),
            Some(true)
        );
        s
    }

    fn rejected(actions: &[Action]) -> Vec<String> {
        events(actions)
            .into_iter()
            .find_map(|e| match e {
                DeviceEvent::ControlRejected { reasons, .. } => Some(reasons),
                _ => None,
            })
            .unwrap_or_default()
    }

    #[test]
    fn unlock_is_sent_only_in_an_alarm_and_never_during_a_job() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        let out = s.control(ControlRequest::Unlock, t0);
        assert!(written(&out).is_empty());
        assert_eq!(rejected(&out), vec!["there is no alarm to clear"]);

        s.on_bytes(ALARM.as_bytes(), t0);
        let out = s.control(ControlRequest::Unlock, t0);
        assert_eq!(written(&out), b"$X\n");
        // Not twice at once.
        assert!(written(&s.control(ControlRequest::Unlock, t0)).is_empty());
        let out = s.on_bytes(b"[MSG:Caution: Unlocked]\r\nok\r\n", t0);
        assert!(events(&out).contains(&DeviceEvent::ControlCompleted { control: "Unlock" }));

        let mut s = job_session(t0);
        s.control(job(&square_job(40)), t0);
        s.on_bytes(ALARM.as_bytes(), t0);
        let out = s.control(ControlRequest::Unlock, t0);
        assert!(!written(&out).windows(2).any(|w| w == b"$X"));
        assert_eq!(rejected(&out), vec!["a job is running"]);
    }

    #[test]
    fn unlock_or_homing_finishes_a_stop_that_waits_for_it() {
        for control in [ControlRequest::Unlock, ControlRequest::Home] {
            let t0 = Instant::now();
            let mut s = homing_session(t0);
            s.control(job(&square_job(40)), t0);
            s.control(ControlRequest::Stop, t0);
            s.on_bytes(b"<Hold:0|MPos:1,1,0|FS:0,0>\r\n", t0);
            s.on_bytes(b"ALARM:3\r\n\r\nGrbl 1.1h ['$' for help]\r\n", t0);
            s.on_bytes(ALARM.as_bytes(), t0);
            assert_eq!(s.job, stopping(StopStage::AwaitingUnlock, None));
            let expected: &[u8] = match control {
                ControlRequest::Unlock => b"$X\n",
                _ => b"$H\n",
            };
            let out = s.control(control, t0);
            assert!(written(&out).ends_with(expected), "{:?}", written(&out));
            // The reads queued by the reset are answered first.
            let mut out = Vec::new();
            for _ in 0..s.in_flight.len() - 1 {
                out = s.on_bytes(b"ok\r\n", t0);
            }
            out.extend(s.on_bytes(b"ok\r\n", t0));
            assert_eq!(job_states(&out), vec![stopping(StopStage::LaserOff, None)]);
            assert!(written(&out).ends_with(b"M5\n"));
            let out = s.on_bytes(b"ok\r\n", t0);
            assert_eq!(job_states(&out), vec![JobState::Idle]);
        }
    }

    #[test]
    fn homing_needs_it_enabled_and_idle_or_alarm() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        let out = s.control(ControlRequest::Home, t0);
        assert!(written(&out).is_empty());
        assert_eq!(
            rejected(&out),
            vec!["homing is disabled on the controller ($22=0)"]
        );

        let mut s = homing_session(t0);
        s.on_bytes(b"<Run|MPos:0,0,0|FS:0,0>\r\n", t0);
        assert!(rejected(&s.control(ControlRequest::Home, t0))[0].contains("Run"));
        s.on_bytes(ALARM.as_bytes(), t0);
        let out = s.control(ControlRequest::Home, t0);
        assert_eq!(written(&out), b"$H\n");
        // Nothing else starts while homing, and silence is expected.
        let out = s.control(job(&square_job(2)), t0);
        assert_eq!(
            rejected(&out),
            vec!["an unlock, homing or jog is still in progress"]
        );
        assert!(!events(&s.on_tick(ms(t0, 10_000))).contains(&DeviceEvent::Unresponsive));
        let out = s.on_bytes(b"ok\r\n", ms(t0, 20_000));
        assert!(events(&out).contains(&DeviceEvent::ControlCompleted { control: "Home" }));
    }

    #[test]
    fn homing_failures_are_reported() {
        let t0 = Instant::now();
        let mut s = homing_session(t0);
        s.control(ControlRequest::Home, t0);
        let out = s.on_bytes(b"ALARM:9\r\n", t0);
        match &events(&out)[..] {
            [
                DeviceEvent::Alarm(9),
                DeviceEvent::ControlFailed {
                    control: "Home",
                    reason,
                },
            ] => {
                assert!(reason.starts_with("ALARM:9"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        assert!(!s.is_homing());

        let mut s = homing_session(t0);
        s.control(ControlRequest::Home, t0);
        let out = s.on_bytes(b"error:5\r\n", t0);
        assert!(matches!(
            events(&out)[..],
            [DeviceEvent::ControlFailed {
                control: "Home",
                ..
            }]
        ));

        // 1.5 x (300 + 50) mm at 600 mm/min is 52.5 s; the limit is twice
        // that plus 10 s. Past it, the machine is stopped.
        let mut s = homing_session(t0);
        s.control(ControlRequest::Home, t0);
        assert!(job_states(&s.on_tick(ms(t0, 114_999))).is_empty());
        let out = s.on_tick(ms(t0, 115_000));
        assert!(matches!(
            events(&out)[0],
            DeviceEvent::ControlFailed {
                control: "Home",
                ..
            }
        ));
        assert_eq!(job_states(&out), vec![stopping(StopStage::Holding, None)]);
    }

    const AT_50: &str =
        "<Idle|MPos:50.000,50.000,0.000|FS:0,0|WCO:0.000,0.000,0.000|Ov:100,100,100>\r\n";

    fn jog_by(dx: f64, dy: f64) -> ControlRequest {
        ControlRequest::Jog {
            step: JogMove {
                dx,
                dy,
                feed_mm_min: 1000.0,
            },
            profile: MachineProfile::default(),
        }
    }

    #[test]
    fn jogs_are_clamped_and_sent_one_at_a_time() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        s.on_bytes(AT_50.as_bytes(), t0);
        let out = s.control(jog_by(10.0, 0.0), t0);
        assert_eq!(written(&out), b"$J=G91 G21 X10 F1000\n");
        let out = s.control(jog_by(10.0, 0.0), t0);
        assert!(written(&out).is_empty());
        assert_eq!(
            rejected(&out),
            vec!["an unlock, homing or jog is still in progress"]
        );
        assert!(events(&s.on_bytes(b"ok\r\n", t0)).is_empty());
        // From x = 50 a long jog left stops at the work area edge.
        let out = s.control(jog_by(-500.0, 0.0), t0);
        assert_eq!(written(&out), b"$J=G91 G21 X-50 F1000\n");
        let out = s.on_bytes(b"error:15\r\n", t0);
        assert!(matches!(
            events(&out)[..],
            [DeviceEvent::ControlFailed { control: "Jog", .. }]
        ));
    }

    #[test]
    fn jog_cancel_is_sent_only_while_jogging() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        s.on_bytes(AT_50.as_bytes(), t0);
        let out = s.control(ControlRequest::JogCancel, t0);
        assert!(written(&out).is_empty());
        s.control(jog_by(0.0, 100.0), t0);
        assert_eq!(written(&s.control(ControlRequest::JogCancel, t0)), [0x85]);
        s.on_bytes(b"ok\r\n<Jog|MPos:50,60,0|FS:1000,0>\r\n", t0);
        assert_eq!(written(&s.control(ControlRequest::JogCancel, t0)), [0x85]);
    }

    #[test]
    fn jogging_needs_a_known_position_and_the_laser_off() {
        let t0 = Instant::now();
        let mut s = job_session(t0);
        s.on_bytes(ALARM.as_bytes(), t0);
        assert_eq!(
            rejected(&s.control(jog_by(1.0, 0.0), t0)),
            vec!["unlock or home the machine first"]
        );

        // GRBL's parser left in M4 S500: a jog would cut.
        let mut s = job_session(t0);
        s.request(G, t0);
        s.on_bytes(
            b"[GC:G1 G54 G17 G21 G90 G94 M4 M9 T0 F1000 S500]\r\nok\r\n",
            t0,
        );
        s.on_bytes(AT_50.as_bytes(), t0);
        let out = s.control(jog_by(1.0, 0.0), t0);
        assert!(written(&out).is_empty());
        assert!(rejected(&out)[0].contains("a jog would fire it"));

        // An old position is not trusted.
        let mut s = job_session(t0);
        s.on_bytes(AT_50.as_bytes(), t0);
        assert!(rejected(&s.control(jog_by(1.0, 0.0), ms(t0, 401)))[0].contains("out of date"));

        // Not during a job; after it, only once `$G` was read again.
        let mut s = job_session(t0);
        let lines = square_job(2);
        s.control(job(&lines), t0);
        assert_eq!(
            rejected(&s.control(jog_by(1.0, 0.0), t0)),
            vec!["a job is running"]
        );
        for _ in 0..lines.len() {
            s.on_bytes(b"ok\r\n", t0);
        }
        let out = s.on_bytes(AT_50.as_bytes(), t0);
        assert_eq!(job_states(&out), vec![JobState::Done]);
        assert_eq!(sends(&out), vec![G]);
        assert_eq!(
            rejected(&s.control(jog_by(1.0, 0.0), t0)),
            vec!["checking the laser state ($G)"]
        );
        s.on_bytes(GC_REPLY.as_bytes(), t0);
        assert_eq!(
            written(&s.control(jog_by(1.0, 0.0), t0)),
            b"$J=G91 G21 X1 F1000\n"
        );
    }

    #[test]
    fn homing_timeout_follows_travel_and_seek_rate() {
        let mut settings = GrblSettings::default();
        assert_eq!(homing_timeout(None), HOMING_FALLBACK);
        assert_eq!(homing_timeout(Some(&settings)), HOMING_FALLBACK);
        for (n, v) in [(25, "600"), (130, "300"), (131, "200"), (132, "50")] {
            settings.insert(n, v).unwrap();
        }
        assert_eq!(homing_timeout(Some(&settings)), Duration::from_secs(115));
        settings.insert(25, "1").unwrap();
        assert_eq!(homing_timeout(Some(&settings)), Duration::from_secs(600));
    }

    /// After one session call: at most one read-only line command was sent,
    /// and it is now in flight.
    fn check_call(s: &Session, actions: &[Action], seed: u64) {
        let lines: Vec<ReadOnlyRequest> = sends(actions)
            .into_iter()
            .filter(|r| !r.is_realtime())
            .collect();
        assert!(lines.len() <= 1, "seed {seed}: {lines:?} sent in one call");
        if let Some(sent) = lines.first() {
            assert_eq!(
                flight(s),
                Some(*sent),
                "seed {seed}: sent {sent:?} without tracking it"
            );
        }
    }

    /// Deterministic pseudo-random numbers (xorshift64), so failures repeat.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    const CORPUS: [&[u8]; 18] = [
        BANNER.as_bytes(),
        b"Grbl 0.9j ['$' for help]\r\n",
        VERSION_REPLY.as_bytes(),
        SETTINGS_REPLY.as_bytes(),
        GC_REPLY.as_bytes(),
        b"ok\r\n",
        b"error:8\r\n",
        b"ALARM:2\r\n",
        b"[MSG:'$H'|'$X' to unlock]\r\n",
        IDLE.as_bytes(),
        b"<Run|MPos:5.000,1.000,0.000|FS:900,250>\r\n",
        b"<Hold:1|WPos:1.000,1.000,1.000>\r\n",
        b"<Hold:0|MPos:1,1,0|FS:0,0|Ov:100,100,100>\r\n",
        b"<Hold:0|MPos:1,1,0|FS:0,0|Ov:100,100,100|A:S>\r\n",
        b"<Alarm|MPos:0,0,0|Pn:X>\r\n",
        b"\xff\xfe\x00junk\r",
        b"[VER:garbage]\r\n",
        b"\r\n\r\n",
    ];

    /// Feeds random mixes of real GRBL output, garbage, user requests and
    /// time jumps. After every call the session has sent at most one line
    /// command and tracks it, nothing but read-only requests was ever
    /// sent, and it never panics.
    #[test]
    fn under_random_input_only_read_only_requests_are_sent() {
        for seed in 1..=40u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let t0 = Instant::now();
            let mut now = t0;
            let mut s = Session::new(POLL, t0);
            for _ in 0..400 {
                let actions = match rng.below(4) {
                    0 => {
                        let chunk = CORPUS[rng.below(CORPUS.len())];
                        let cut = rng.below(chunk.len() + 1);
                        let first = s.on_bytes(&chunk[..cut], now);
                        check_call(&s, &first, seed);
                        if disconnect_reason(&first).is_some() {
                            break;
                        }
                        s.on_bytes(&chunk[cut..], now)
                    }
                    1 => s.request(ReadOnlyRequest::ALL[rng.below(4)], now),
                    _ => {
                        now += Duration::from_millis(rng.below(1_500) as u64);
                        s.on_tick(now)
                    }
                };
                check_call(&s, &actions, seed);
                assert!(
                    actions.iter().all(
                        |a| !matches!(a, Action::Send(o) if !matches!(o, Outgoing::ReadOnly(_)))
                    ),
                    "seed {seed}: {actions:?}"
                );
                if disconnect_reason(&actions).is_some() {
                    break;
                }
            }
        }
    }

    /// Random controller output, controls and time jumps with jobs: `$X`
    /// and `$H` go out only in answer to their own request, jogging never,
    /// job lines only while a job runs, GRBL's buffer is never overfilled,
    /// and a job that was faulted or stopped never returns to `Running`
    /// without a new start.
    #[test]
    fn under_random_input_jobs_stay_safe() {
        let (mut unlocks, mut homes, mut jogs) = (0, 0, 0);
        for seed in 1..=60u64 {
            let mut rng = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D));
            let t0 = Instant::now();
            let mut now = t0;
            let mut s = homing_session(t0);
            let mut may_run = false;
            for _ in 0..500 {
                let mut started = false;
                let mut asked = None;
                let actions = match rng.below(6) {
                    0 | 1 => s.on_bytes(CORPUS[rng.below(CORPUS.len())], now),
                    2 => {
                        let control = match rng.below(10) {
                            0 => {
                                started = true;
                                job(&square_job(1 + rng.below(30)))
                            }
                            1 => ControlRequest::Hold,
                            2 => ControlRequest::Resume,
                            3 => ControlRequest::Stop,
                            4 => ControlRequest::AcknowledgeFault,
                            5 => ControlRequest::Unlock,
                            6 => ControlRequest::Home,
                            7 => jog_by(rng.below(21) as f64 - 10.0, rng.below(21) as f64 - 10.0),
                            8 => ControlRequest::JogCancel,
                            _ => {
                                started = true;
                                job(&square_job(2))
                            }
                        };
                        asked = Some(control.label());
                        s.control(control, now)
                    }
                    _ => {
                        now += Duration::from_millis(rng.below(1_200) as u64);
                        s.on_tick(now)
                    }
                };
                if disconnect_reason(&actions).is_some() {
                    break;
                }
                let bytes = written(&actions);
                for (command, only_for) in [
                    (&b"$X"[..], Some("Unlock")),
                    (b"$H", Some("Home")),
                    (b"$J", Some("Jog")),
                ] {
                    if bytes.windows(2).any(|w| w == command) {
                        assert!(
                            only_for.is_some() && asked == only_for,
                            "seed {seed}: sent {command:?} for {asked:?}"
                        );
                        match only_for {
                            Some("Unlock") => unlocks += 1,
                            Some("Home") => homes += 1,
                            _ => jogs += 1,
                        }
                    }
                }
                if !job_lines_sent(&actions).is_empty() {
                    assert!(
                        matches!(s.job, JobState::Running | JobState::Held),
                        "seed {seed}: job lines sent while {:?}",
                        s.job
                    );
                }
                if let Some(streamer) = &s.streamer {
                    assert!(streamer.in_flight_bytes() <= 127, "seed {seed}");
                }
                for state in job_states(&actions) {
                    match state {
                        JobState::Running => {
                            assert!(may_run || started, "seed {seed}: back to Running");
                            may_run = true;
                        }
                        JobState::Held => {}
                        _ => may_run = false,
                    }
                }
            }
        }
        // The run did reach the guarded commands, so the checks above held
        // for real sends, not just for their absence.
        assert!(
            unlocks > 0 && homes > 0 && jogs > 0,
            "$X sent {unlocks}, $H sent {homes}, $J sent {jogs}"
        );
    }
}
