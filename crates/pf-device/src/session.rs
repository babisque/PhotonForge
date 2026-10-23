//! The protocol logic of one connection, without threads or I/O: bytes and
//! the current time go in, actions come out. The connection thread performs
//! the actions; tests drive the session directly with chosen times.
//!
//! An action can only send a `ReadOnlyRequest`, so nothing else can ever be
//! written to the controller.

use std::collections::VecDeque;
use std::mem;
use std::time::{Duration, Instant};

use crate::event::{ConnectionState, DeviceEvent};
use crate::protocol::ParseError;
use crate::protocol::build_info::{
    BuildInfo, BuildOptions, Version, is_supported_version, parse_options, parse_version,
};
use crate::protocol::parse::{Response, parse_line};
use crate::protocol::parser_state::{ParserState, parse_parser_state};
use crate::protocol::readonly::ReadOnlyRequest;
use crate::protocol::settings::GrblSettings;
use crate::protocol::status::MachineState;

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

#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "a few actions per loop pass, consumed at once; boxing would only add allocations"
)]
pub(crate) enum Action {
    Send(ReadOnlyRequest),
    Emit(DeviceEvent),
    /// End the connection with this reason.
    Disconnect(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Identifying { since: Instant },
    Ready { since: Instant },
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct InFlight {
    request: ReadOnlyRequest,
    sent_at: Instant,
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
    /// GRBL answers line commands in order; only one is outstanding.
    in_flight: Option<InFlight>,
    queue: VecDeque<ReadOnlyRequest>,
    collected: Collected,
    asked_version: bool,
    last_poll: Option<Instant>,
    last_status: Option<Instant>,
    silent: bool,
    state: Option<MachineState>,
    /// GRBL sends WCO only now and then; positions need the last one.
    last_wco: Option<[f64; 3]>,
}

impl Session {
    pub(crate) fn new(poll_interval: Duration, now: Instant) -> Session {
        Session {
            poll_interval,
            phase: Phase::Identifying { since: now },
            partial: Vec::new(),
            in_flight: None,
            queue: VecDeque::new(),
            collected: Collected::default(),
            asked_version: false,
            last_poll: None,
            last_status: None,
            silent: false,
            state: None,
            last_wco: None,
        }
    }

    /// Queues a request from the user. `?` is sent right away once ready;
    /// line commands wait for their turn and a state GRBL accepts them in.
    pub(crate) fn request(&mut self, request: ReadOnlyRequest, now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        if request.is_realtime() {
            if matches!(self.phase, Phase::Ready { .. }) {
                self.last_poll = Some(now);
                out.push(Action::Send(request));
            }
        } else {
            self.enqueue(request);
            self.dispatch(now, &mut out);
        }
        out
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
                return out;
            }
        }
        self.dispatch(now, &mut out);
        out
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
                    return out;
                }
                if waited >= BANNER_WAIT && !self.asked_version && self.in_flight.is_none() {
                    self.asked_version = true;
                    self.send_line(ReadOnlyRequest::BuildInfo, now, &mut out);
                }
            }
            Phase::Ready { since } => {
                let due = self
                    .last_poll
                    .is_none_or(|t| now.saturating_duration_since(t) >= self.poll_interval);
                if due {
                    self.last_poll = Some(now);
                    out.push(Action::Send(ReadOnlyRequest::StatusQuery));
                }
                let heard = self.last_status.unwrap_or(since);
                if !self.silent && now.saturating_duration_since(heard) >= STATUS_SILENCE {
                    self.silent = true;
                    out.push(Action::Emit(DeviceEvent::Unresponsive));
                }
            }
        }
        if let Some(flight) = self.in_flight
            && now.saturating_duration_since(flight.sent_at) >= REQUEST_TIMEOUT
        {
            self.in_flight = None;
            self.collected = Collected::default();
            out.push(Action::Emit(DeviceEvent::RequestTimedOut(flight.request)));
        }
        self.dispatch(now, &mut out);
        out
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
        let collecting = self.in_flight.map(|f| f.request);

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
                out.push(Action::Emit(DeviceEvent::Status { report, positions }));
            }
            Response::Welcome { version } => self.on_welcome(version, now, out),
            Response::Ok => self.complete(None, out),
            Response::Error(code) => self.complete(Some(code), out),
            Response::Alarm(code) => out.push(Action::Emit(DeviceEvent::Alarm(code))),
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
        let was_ready = matches!(self.phase, Phase::Ready { .. });
        self.in_flight = None;
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

    /// `ok` or `error:N` answers the line command in flight. An answer with
    /// nothing in flight (e.g. after a timeout) is only shown in the console.
    fn complete(&mut self, error: Option<u8>, out: &mut Vec<Action>) {
        let Some(flight) = self.in_flight.take() else {
            return;
        };
        let collected = mem::take(&mut self.collected);
        if let Some(code) = error {
            out.push(Action::Emit(DeviceEvent::CommandFailed {
                request: flight.request,
                code,
            }));
            return;
        }
        let event = match flight.request {
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
        let in_flight = self.in_flight.is_some_and(|f| f.request == request);
        if !in_flight && !self.queue.contains(&request) {
            self.queue.push_back(request);
        }
    }

    /// Sends the next queued line command GRBL accepts in its current state,
    /// if nothing is in flight.
    fn dispatch(&mut self, now: Instant, out: &mut Vec<Action>) {
        if self.in_flight.is_some() || !matches!(self.phase, Phase::Ready { .. }) {
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
        self.in_flight = Some(InFlight {
            request,
            sent_at: now,
        });
        out.push(Action::Send(request));
    }
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
    use crate::protocol::status::ReportedPosition;
    use ReadOnlyRequest::{BuildInfo as I, ParserState as G, StatusQuery as Q, ViewSettings as S};

    const POLL: Duration = Duration::from_millis(200);
    const BANNER: &str = "Grbl 1.1h ['$' for help]\r\n";
    const VERSION_REPLY: &str = "[VER:1.1h.20190825:]\r\n[OPT:V,15,128]\r\nok\r\n";
    const SETTINGS_REPLY: &str = "$10=1\r\n$30=1000\r\n$32=1\r\n$130=300.000\r\nok\r\n";
    const GC_REPLY: &str = "[GC:G0 G54 G17 G21 G90 G94 M5 M9 T0 F0 S0]\r\nok\r\n";

    fn ms(t0: Instant, millis: u64) -> Instant {
        t0 + Duration::from_millis(millis)
    }

    fn sends(actions: &[Action]) -> Vec<ReadOnlyRequest> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Send(r) => Some(*r),
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

    /// A session that saw the banner and answered its startup reads.
    fn ready_session(t0: Instant) -> Session {
        let mut s = Session::new(POLL, t0);
        s.on_bytes(BANNER.as_bytes(), t0);
        s.on_bytes(VERSION_REPLY.as_bytes(), t0);
        s.on_bytes(SETTINGS_REPLY.as_bytes(), t0);
        s.on_bytes(GC_REPLY.as_bytes(), t0);
        assert!(s.in_flight.is_none() && s.queue.is_empty());
        s
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
        assert_eq!(s.in_flight.map(|f| f.request), Some(G));
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
        assert!(s.in_flight.is_none());
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
        assert!(s.in_flight.is_none());
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

    /// After one session call: at most one line command was sent, and it is
    /// now the one in flight. `ReadOnlyRequest` admits nothing but the four
    /// read-only commands, so every send is one of them by construction.
    fn check_call(s: &Session, actions: &[Action], seed: u64) {
        let lines: Vec<ReadOnlyRequest> = sends(actions)
            .into_iter()
            .filter(|r| !r.is_realtime())
            .collect();
        assert!(lines.len() <= 1, "seed {seed}: {lines:?} sent in one call");
        if let Some(sent) = lines.first() {
            assert_eq!(
                s.in_flight.map(|f| f.request),
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

    /// Feeds random mixes of real GRBL output, garbage, user requests and
    /// time jumps. After every call the session has sent at most one line
    /// command and tracks it, and it never panics.
    #[test]
    fn under_random_input_only_read_only_requests_are_sent() {
        let corpus: [&[u8]; 16] = [
            BANNER.as_bytes(),
            b"Grbl 0.9j ['$' for help]\r\n",
            VERSION_REPLY.as_bytes(),
            SETTINGS_REPLY.as_bytes(),
            GC_REPLY.as_bytes(),
            b"ok\r\n",
            b"error:8\r\n",
            b"ALARM:2\r\n",
            b"[MSG:'$H'|'$X' to unlock]\r\n",
            b"<Idle|MPos:0.000,0.000,0.000|FS:0,0|WCO:0.000,0.000,0.000>\r\n",
            b"<Run|MPos:5.000,1.000,0.000|FS:900,250>\r\n",
            b"<Hold:1|WPos:1.000,1.000,1.000>\r\n",
            b"<Alarm|MPos:0,0,0|Pn:X>\r\n",
            b"\xff\xfe\x00junk\r",
            b"[VER:garbage]\r\n",
            b"\r\n\r\n",
        ];
        for seed in 1..=40u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let t0 = Instant::now();
            let mut now = t0;
            let mut s = Session::new(POLL, t0);
            for _ in 0..400 {
                let actions = match rng.below(4) {
                    0 => {
                        let chunk = corpus[rng.below(corpus.len())];
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
                if disconnect_reason(&actions).is_some() {
                    break;
                }
            }
        }
    }
}
