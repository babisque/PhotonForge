//! A connection to a GRBL controller. One thread owns the serial port and
//! runs the `Session`; the app talks to it through channels: read-only and
//! control requests in, `DeviceEvent`s out. Dropping the `Connection` closes
//! the port.

use std::ops::RangeInclusive;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use crate::control::ControlRequest;
use crate::error::DeviceError;
use crate::event::{ConnectionState, DeviceEvent};
use crate::protocol::outgoing::Outgoing;
use crate::protocol::readonly::ReadOnlyRequest;
use crate::session::{Action, Session};
use crate::transport::{SerialSettings, SerialTransport, Transport};

pub const DEFAULT_POLL_HZ: u8 = 5;
pub const POLL_HZ_RANGE: RangeInclusive<u8> = 1..=10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectOptions {
    pub serial: SerialSettings,
    /// Status reports per second, clamped to `POLL_HZ_RANGE`.
    pub poll_hz: u8,
}

impl ConnectOptions {
    pub fn new(port: impl Into<String>) -> ConnectOptions {
        ConnectOptions {
            serial: SerialSettings::new(port),
            poll_hz: DEFAULT_POLL_HZ,
        }
    }

    fn poll_interval(&self) -> Duration {
        let hz = self
            .poll_hz
            .clamp(*POLL_HZ_RANGE.start(), *POLL_HZ_RANGE.end());
        Duration::from_secs(1) / u32::from(hz)
    }
}

/// A live connection.
#[derive(Debug)]
pub struct Connection {
    requests: Sender<ReadOnlyRequest>,
    controls: Sender<ControlRequest>,
    events: Receiver<DeviceEvent>,
}

impl Connection {
    /// Asks the controller for a report. Returns false once the connection
    /// has ended.
    pub fn request(&self, request: ReadOnlyRequest) -> bool {
        self.requests.send(request).is_ok()
    }

    /// Acts on the machine: start, pause, resume or stop a job. A refused
    /// request is reported as `DeviceEvent::ControlRejected`. Returns false
    /// once the connection has ended.
    pub fn control(&self, control: ControlRequest) -> bool {
        self.controls.send(control).is_ok()
    }

    /// Events since the last call, oldest first. Never blocks.
    pub fn poll_events(&self) -> Vec<DeviceEvent> {
        self.events.try_iter().collect()
    }
}

/// Opens the port on a new thread and starts talking to the controller.
/// Failures, including a port that cannot be opened, arrive as a
/// `DeviceEvent::Disconnected`; the UI never waits on the port.
pub fn connect(options: ConnectOptions) -> Result<Connection, DeviceError> {
    let serial = options.serial.clone();
    start(
        move || SerialTransport::open(&serial),
        options.poll_interval(),
    )
}

fn start<T, F>(open: F, poll_interval: Duration) -> Result<Connection, DeviceError>
where
    T: Transport + 'static,
    F: FnOnce() -> Result<T, DeviceError> + Send + 'static,
{
    let (request_tx, request_rx) = mpsc::channel();
    let (control_tx, control_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    thread::Builder::new()
        .name("grbl-connection".into())
        .spawn(move || {
            // Send errors mean the app dropped the connection; nobody is
            // left to tell, so they are ignored.
            let _ = event_tx.send(DeviceEvent::ConnectionChanged(ConnectionState::Opening));
            let transport = match open() {
                Ok(transport) => transport,
                Err(err) => {
                    let _ = event_tx.send(DeviceEvent::Disconnected {
                        reason: err.to_string(),
                    });
                    return;
                }
            };
            let _ = event_tx.send(DeviceEvent::ConnectionChanged(ConnectionState::Identifying));
            let channels = Channels {
                requests: request_rx,
                controls: control_rx,
                events: event_tx.clone(),
            };
            if let Some(reason) = run(transport, poll_interval, &channels) {
                let _ = event_tx.send(DeviceEvent::Disconnected { reason });
            }
        })
        .map_err(|err| DeviceError::Thread(err.to_string()))?;
    Ok(Connection {
        requests: request_tx,
        controls: control_tx,
        events: event_rx,
    })
}

struct Channels {
    requests: Receiver<ReadOnlyRequest>,
    controls: Receiver<ControlRequest>,
    events: Sender<DeviceEvent>,
}

/// Runs until the link fails (returns why) or the app drops the connection
/// (returns `None`). Each pass waits at most one transport read timeout.
fn run<T: Transport>(
    mut transport: T,
    poll_interval: Duration,
    channels: &Channels,
) -> Option<String> {
    let events = &channels.events;
    let mut session = Session::new(poll_interval, Instant::now());
    let mut buf = [0u8; 512];
    loop {
        let mut actions = Vec::new();
        loop {
            match channels.requests.try_recv() {
                Ok(request) => actions.extend(session.request(request, Instant::now())),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return None,
            }
        }
        loop {
            match channels.controls.try_recv() {
                Ok(control) => actions.extend(session.control(control, Instant::now())),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return None,
            }
        }
        match transport.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => actions.extend(session.on_bytes(&buf[..n], Instant::now())),
            Err(err) => return Some(format!("Connection lost: {err}")),
        }
        actions.extend(session.on_tick(Instant::now()));

        for action in actions {
            match action {
                Action::Send(outgoing) => {
                    // The only write in this crate outside tests.
                    if let Err(err) = transport.write_all(&outgoing.bytes()) {
                        return Some(format!("Connection lost: {err}"));
                    }
                    if let Some(event) = sent_event(&outgoing)
                        && events.send(event).is_err()
                    {
                        return None;
                    }
                }
                Action::Emit(event) => {
                    if events.send(event).is_err() {
                        return None;
                    }
                }
                Action::Disconnect(reason) => return Some(reason),
            }
        }
    }
}

/// What the console shows for a write. Job lines are left out: a job has
/// thousands, and its progress is reported instead.
fn sent_event(outgoing: &Outgoing) -> Option<DeviceEvent> {
    match outgoing {
        Outgoing::ReadOnly(request) => Some(DeviceEvent::Sent(*request)),
        Outgoing::Realtime(command) => Some(DeviceEvent::SentRealtime(*command)),
        Outgoing::LaserOff => Some(DeviceEvent::SentLine(String::from("M5"))),
        Outgoing::Unlock => Some(DeviceEvent::SentLine(String::from("$X"))),
        Outgoing::Home => Some(DeviceEvent::SentLine(String::from("$H"))),
        Outgoing::Jog(line) => Some(DeviceEvent::SentLine(line.as_str().to_owned())),
        Outgoing::JobLine(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::mock::{MockHandle, MockTransport, mock};
    use std::io;

    const POLL: Duration = Duration::from_millis(20);
    const BANNER: &str = "Grbl 1.1h ['$' for help]\r\n";

    fn scripted_grbl() -> (MockTransport, MockHandle) {
        let (transport, handle) = mock();
        handle.reply_to("$I\n", "[VER:1.1h.20190825:]\r\n[OPT:V,15,128]\r\nok\r\n");
        handle.reply_to("$$\n", "$10=1\r\n$30=1000\r\n$32=1\r\nok\r\n");
        handle.reply_to(
            "$G\n",
            "[GC:G0 G54 G17 G21 G90 G94 M5 M9 T0 F0 S0]\r\nok\r\n",
        );
        handle.reply_to(
            "?",
            "<Idle|MPos:1.000,2.000,0.000|FS:0,0|WCO:0.000,0.000,0.000>\r\n",
        );
        (transport, handle)
    }

    /// Collects events until `done` matches one, or panics after 5 s.
    fn collect_until(
        connection: &Connection,
        done: impl Fn(&DeviceEvent) -> bool,
    ) -> Vec<DeviceEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            for event in connection.poll_events() {
                let finished = done(&event);
                seen.push(event);
                if finished {
                    return seen;
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("timed out; events so far: {seen:#?}");
    }

    fn wait_for_close(is_closed: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !is_closed() {
            assert!(Instant::now() < deadline, "the port was not closed");
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Splits written bytes into requests; fails on anything else.
    fn split_requests(mut bytes: &[u8]) -> Result<Vec<ReadOnlyRequest>, Vec<u8>> {
        let mut requests = Vec::new();
        while !bytes.is_empty() {
            let next = ReadOnlyRequest::ALL
                .into_iter()
                .find(|r| bytes.starts_with(r.bytes()))
                .ok_or_else(|| bytes.to_vec())?;
            requests.push(next);
            bytes = &bytes[next.bytes().len()..];
        }
        Ok(requests)
    }

    #[test]
    fn a_full_session_only_writes_read_only_requests() {
        let (transport, handle) = scripted_grbl();
        handle.push(BANNER);
        let connection = start(move || Ok(transport), POLL).unwrap();

        let events = collect_until(&connection, |e| {
            matches!(e, DeviceEvent::ParserStateLoaded(_))
        });
        assert_eq!(
            events[..3],
            [
                DeviceEvent::ConnectionChanged(ConnectionState::Opening),
                DeviceEvent::ConnectionChanged(ConnectionState::Identifying),
                DeviceEvent::Received {
                    line: "Grbl 1.1h ['$' for help]".into(),
                    response: crate::protocol::parse::Response::Welcome {
                        version: "1.1h".into()
                    },
                },
            ]
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, DeviceEvent::BuildInfoLoaded(_)))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, DeviceEvent::SettingsLoaded { .. }))
        );

        // Status polling runs, and an app request is answered.
        collect_until(&connection, |e| matches!(e, DeviceEvent::Status { .. }));
        assert!(connection.request(ReadOnlyRequest::ParserState));
        collect_until(&connection, |e| {
            matches!(e, DeviceEvent::ParserStateLoaded(_))
        });

        drop(connection);
        wait_for_close(|| handle.is_closed());
        let written = split_requests(&handle.written()).unwrap();
        assert!(written.contains(&ReadOnlyRequest::StatusQuery));
        assert_eq!(
            written
                .iter()
                .filter(|r| **r == ReadOnlyRequest::ParserState)
                .count(),
            2
        );
    }

    #[test]
    fn a_session_runs_against_the_simulated_controller() {
        use crate::sim::{SimConfig, sim};
        let (transport, handle) = sim(SimConfig::default());
        let connection = start(move || Ok(transport), POLL).unwrap();
        collect_until(&connection, |e| {
            matches!(e, DeviceEvent::ParserStateLoaded(_))
        });
        let events = collect_until(&connection, |e| matches!(e, DeviceEvent::Status { .. }));
        match events.last() {
            Some(DeviceEvent::Status { report, positions }) => {
                assert_eq!(report.state, crate::protocol::status::MachineState::Idle);
                assert_eq!(positions.work, Some([0.0, 0.0, 0.0]));
            }
            other => panic!("{other:?}"),
        }
        drop(connection);
        wait_for_close(|| handle.is_closed());
        assert!(split_requests(&handle.written()).is_ok());
        assert!(!handle.overflowed());
    }

    mod with_simulator {
        use super::*;
        use crate::control::{ControlRequest, JobRequest};
        use crate::job::{FaultReason, JobKind, JobState, StopStage};
        use crate::sim::{SimConfig, SimHandle, Spindle, sim};
        use crate::snapshot::DeviceSnapshot;
        use crate::stream::StreamMode;
        use pf_core::MachineProfile;

        /// A connection to a simulated GRBL that has read its settings and
        /// reported its status, so a job may start.
        fn ready_connection(config: SimConfig) -> (Connection, SimHandle) {
            let (transport, handle) = sim(config);
            let connection = start(move || Ok(transport), POLL).unwrap();
            collect_until(&connection, |e| {
                matches!(e, DeviceEvent::ParserStateLoaded(_))
            });
            collect_until(&connection, |e| matches!(e, DeviceEvent::Status { .. }));
            (connection, handle)
        }

        fn job_lines(n: usize, laser: &str) -> Vec<String> {
            let mut lines = vec![String::from("G21 G90"), format!("{laser} S0")];
            for i in 0..n {
                lines.push(format!("G1 X{}.5 Y{}.25 F1200 S500", i % 100, i % 50));
            }
            lines.push(String::from("M5"));
            lines
        }

        fn start_job(connection: &Connection, lines: &[String]) {
            assert!(connection.control(ControlRequest::Start(JobRequest {
                kind: JobKind::Job,
                lines: lines.to_vec(),
                profile: MachineProfile::default(),
                mode: StreamMode::CharacterCounting,
            })));
        }

        fn wait_for_progress(connection: &Connection, acked: usize) -> Vec<DeviceEvent> {
            collect_until(
                connection,
                |e| matches!(e, DeviceEvent::JobProgress(p) if p.lines_acked >= acked),
            )
        }

        fn wait_for_job(
            connection: &Connection,
            done: impl Fn(&JobState) -> bool,
        ) -> Vec<DeviceEvent> {
            let events = collect_until(connection, |e| match e {
                DeviceEvent::JobStateChanged(state) => done(state),
                DeviceEvent::ControlRejected { .. } => true,
                _ => false,
            });
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, DeviceEvent::ControlRejected { .. })),
                "{events:#?}"
            );
            events
        }

        fn wrote(handle: &SimHandle, needle: &[u8]) -> bool {
            handle.written().windows(needle.len()).any(|w| w == needle)
        }

        #[test]
        fn a_job_streams_without_overflowing_grbl() {
            let (connection, handle) = ready_connection(SimConfig::default());
            let lines = job_lines(150, "M4");
            start_job(&connection, &lines);
            wait_for_job(&connection, |s| *s == JobState::Done);
            assert!(!handle.overflowed());
            // The buffer was used, not one line at a time.
            assert!(handle.max_rx_used() > 100, "{}", handle.max_rx_used());
            assert_eq!(handle.gcode_lines(), lines);
            assert_eq!(handle.spindle().0, Spindle::Off);
            drop(connection);
            wait_for_close(|| handle.is_closed());
        }

        #[test]
        fn pause_and_resume_finish_the_job() {
            let (connection, handle) = ready_connection(SimConfig::default());
            let lines = job_lines(300, "M3");
            start_job(&connection, &lines);
            wait_for_progress(&connection, 20);
            assert!(connection.control(ControlRequest::Hold));
            wait_for_job(&connection, |s| *s == JobState::Held);
            let held_at = handle.gcode_lines().len();
            assert!(held_at < lines.len());
            assert!(connection.control(ControlRequest::Resume));
            wait_for_job(&connection, |s| *s == JobState::Done);
            assert!(!handle.overflowed());
            assert_eq!(handle.gcode_lines(), lines);
        }

        #[test]
        fn stop_resets_and_leaves_the_laser_off_without_unlocking() {
            let (connection, handle) = ready_connection(SimConfig::default());
            let lines = job_lines(300, "M3");
            start_job(&connection, &lines);
            wait_for_progress(&connection, 20);
            assert!(connection.control(ControlRequest::Stop));
            let events = wait_for_job(&connection, |s| {
                matches!(
                    s,
                    JobState::Stopping {
                        stage: StopStage::AwaitingUnlock,
                        ..
                    } | JobState::Idle
                        | JobState::Faulted(_)
                )
            });
            // Reset while held counts as moving in the simulator: ALARM:3.
            assert!(
                events.contains(&DeviceEvent::JobStateChanged(JobState::Stopping {
                    stage: StopStage::AwaitingUnlock,
                    fault: None,
                }))
            );
            assert_eq!(handle.spindle(), (Spindle::Off, 0.0));
            assert!(wrote(&handle, b"!") && wrote(&handle, &[0x18]));
            assert!(!wrote(&handle, b"$X"));
            assert!(handle.gcode_lines().len() < lines.len());
            assert!(!handle.overflowed());
        }

        #[test]
        fn unlocking_after_a_stop_ends_with_the_laser_off() {
            let (connection, handle) = ready_connection(SimConfig::default());
            start_job(&connection, &job_lines(300, "M3"));
            wait_for_progress(&connection, 20);
            assert!(connection.control(ControlRequest::Stop));
            wait_for_job(&connection, |s| {
                matches!(
                    s,
                    JobState::Stopping {
                        stage: StopStage::AwaitingUnlock,
                        ..
                    }
                )
            });
            assert!(!wrote(&handle, b"$X"));
            assert!(connection.control(ControlRequest::Unlock));
            wait_for_job(&connection, |s| *s == JobState::Idle);
            let written = handle.written();
            let unlock = written.windows(3).position(|w| w == b"$X\n").unwrap();
            let laser_off = written.windows(3).rposition(|w| w == b"M5\n").unwrap();
            assert!(unlock < laser_off);
            assert_eq!(handle.state(), crate::protocol::status::MachineState::Idle);
            assert_eq!(handle.spindle(), (Spindle::Off, 0.0));
        }

        fn start_frame(connection: &Connection, gcode: &str, profile: MachineProfile) {
            assert!(connection.control(ControlRequest::Start(JobRequest {
                kind: JobKind::Frame,
                lines: gcode.lines().map(str::to_owned).collect(),
                profile,
                mode: StreamMode::CharacterCounting,
            })));
        }

        /// `pf_gcode::frame` output for a 10..60 x 10..40 mm job (see the
        /// `frame_*` golden files in pf-gcode).
        const FRAME_OFF: &str = "; PhotonForge frame\nG21 G90\nM5\nG0 X10 Y10\n\
            G1 X60 Y10 F3000\nG1 X60 Y40\nG1 X10 Y40\nG1 X10 Y10\nM5\n";
        const FRAME_1_PERCENT: &str = "; PhotonForge frame\nG21 G90\nM4 S0\nG0 X10 Y10\n\
            G1 X60 Y10 F3000 S10\nG1 X60 Y40\nG1 X10 Y40\nG1 X10 Y10\nM5\n";

        #[test]
        fn a_frame_traces_the_box_without_firing_the_laser() {
            let (connection, handle) = ready_connection(SimConfig::default());
            start_frame(&connection, FRAME_OFF, MachineProfile::default());
            wait_for_job(&connection, |s| *s == JobState::Done);
            assert_eq!(handle.position(), [10.0, 10.0]);
            assert_eq!(handle.gcode_lines().len(), 8);
            assert!(!handle.laser_fired());

            // A real job does fire it, so the check above means something.
            start_job(&connection, &job_lines(5, "M4"));
            wait_for_job(&connection, |s| *s == JobState::Done);
            assert!(handle.laser_fired());
        }

        #[test]
        fn a_frame_above_the_frame_power_is_refused() {
            let (connection, handle) = ready_connection(SimConfig::default());
            // A 1 % frame, sent with the default profile, which allows 0 %.
            start_frame(&connection, FRAME_1_PERCENT, MachineProfile::default());
            let events = collect_until(&connection, |e| {
                matches!(e, DeviceEvent::ControlRejected { .. })
            });
            match events.last() {
                Some(DeviceEvent::ControlRejected { control, reasons }) => {
                    assert_eq!(*control, "Frame");
                    assert!(reasons[0].contains("Frame limit"), "{reasons:?}");
                }
                other => panic!("{other:?}"),
            }
            assert!(handle.gcode_lines().is_empty());
        }

        #[test]
        fn step_jogs_move_the_head_and_stop_at_the_edge() {
            use crate::jog::JogMove;
            let (connection, handle) = ready_connection(SimConfig::default());
            let jog = |dx, dy| ControlRequest::Jog {
                step: JogMove {
                    dx,
                    dy,
                    feed_mm_min: 2000.0,
                },
                profile: MachineProfile::default(),
            };
            assert!(connection.control(jog(10.0, 5.0)));
            collect_until(&connection, |e| {
                matches!(e, DeviceEvent::Status { positions, .. }
                    if positions.machine == Some([10.0, 5.0, 0.0]))
            });
            // The default work area is 300 x 200 mm: this stops at y = 200.
            assert!(connection.control(jog(0.0, 1000.0)));
            collect_until(&connection, |e| {
                matches!(e, DeviceEvent::Status { positions, .. }
                    if positions.machine == Some([10.0, 200.0, 0.0]))
            });
            assert!(wrote(&handle, b"$J=G91 G21 Y195 F2000\n"));
            assert_eq!(handle.spindle().0, Spindle::Off);
        }

        #[test]
        fn homing_clears_the_startup_alarm() {
            let (connection, handle) = ready_connection(SimConfig {
                homing: true,
                ..SimConfig::default()
            });
            assert!(connection.control(ControlRequest::Home));
            let events = collect_until(&connection, |e| {
                matches!(
                    e,
                    DeviceEvent::ControlCompleted { .. } | DeviceEvent::ControlFailed { .. }
                )
            });
            assert_eq!(
                events.last(),
                Some(&DeviceEvent::ControlCompleted { control: "Home" })
            );
            assert_eq!(handle.state(), crate::protocol::status::MachineState::Idle);
            assert!(!wrote(&handle, b"$X"));
        }

        #[test]
        fn stop_interrupts_homing() {
            // Long enough that the stop always lands mid-cycle.
            let (connection, handle) = ready_connection(SimConfig {
                homing: true,
                homing_reads: 1_000_000,
                ..SimConfig::default()
            });
            assert!(connection.control(ControlRequest::Home));
            collect_until(
                &connection,
                |e| matches!(e, DeviceEvent::SentLine(line) if line == "$H"),
            );
            assert!(connection.control(ControlRequest::Stop));
            // GRBL ignores a feed hold while homing; the stop resets anyway.
            let events = wait_for_job(&connection, |s| {
                matches!(
                    s,
                    JobState::Stopping {
                        stage: StopStage::AwaitingUnlock,
                        ..
                    }
                )
            });
            assert!(events.iter().any(|e| matches!(
                e,
                DeviceEvent::ControlFailed {
                    control: "Home",
                    ..
                }
            )));
            assert!(wrote(&handle, &[0x18]));
            assert_eq!(handle.state(), crate::protocol::status::MachineState::Alarm);
        }

        #[test]
        fn a_laser_left_on_during_a_pause_stops_the_job() {
            let (connection, handle) = ready_connection(SimConfig {
                laser_off_in_hold: false,
                ..SimConfig::default()
            });
            start_job(&connection, &job_lines(300, "M3"));
            wait_for_progress(&connection, 20);
            assert!(connection.control(ControlRequest::Hold));
            let events = wait_for_job(&connection, |s| {
                matches!(
                    s,
                    JobState::Stopping {
                        stage: StopStage::AwaitingUnlock,
                        ..
                    } | JobState::Held
                        | JobState::Faulted(_)
                )
            });
            assert!(!events.contains(&DeviceEvent::JobStateChanged(JobState::Held)));
            assert!(
                events.contains(&DeviceEvent::JobStateChanged(JobState::Stopping {
                    stage: StopStage::Holding,
                    fault: Some(FaultReason::LaserOnDuringHold),
                }))
            );
            assert_eq!(handle.spindle(), (Spindle::Off, 0.0));
        }

        #[test]
        fn unplugging_mid_job_is_a_fault_and_nothing_resumes() {
            let (connection, handle) = ready_connection(SimConfig::default());
            start_job(&connection, &job_lines(300, "M4"));
            let mut snapshot = DeviceSnapshot::default();
            for event in wait_for_progress(&connection, 20) {
                snapshot.apply(&event);
            }
            handle.unplug();
            for event in collect_until(&connection, |e| {
                matches!(e, DeviceEvent::Disconnected { .. })
            }) {
                snapshot.apply(&event);
            }
            assert!(matches!(
                snapshot.job,
                JobState::Faulted(FaultReason::Disconnected(_))
            ));
            assert!(!connection.control(ControlRequest::Resume));
            wait_for_close(|| handle.is_closed());
        }
    }

    #[test]
    fn unplugging_ends_with_a_disconnected_event() {
        let (transport, handle) = scripted_grbl();
        handle.push(BANNER);
        handle.fail_next_read(io::ErrorKind::BrokenPipe);
        let connection = start(move || Ok(transport), POLL).unwrap();
        let events = collect_until(&connection, |e| {
            matches!(e, DeviceEvent::Disconnected { .. })
        });
        match events.last() {
            Some(DeviceEvent::Disconnected { reason }) => {
                assert!(reason.starts_with("Connection lost"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        wait_for_close(|| handle.is_closed());
        assert!(!connection.request(ReadOnlyRequest::StatusQuery));
    }

    #[test]
    fn a_failed_write_ends_the_connection() {
        let (transport, handle) = scripted_grbl();
        handle.fail_writes(io::ErrorKind::PermissionDenied);
        handle.push(BANNER);
        let connection = start(move || Ok(transport), POLL).unwrap();
        let events = collect_until(&connection, |e| {
            matches!(e, DeviceEvent::Disconnected { .. })
        });
        assert!(!events.iter().any(|e| matches!(e, DeviceEvent::Sent(_))));
        wait_for_close(|| handle.is_closed());
    }

    #[test]
    fn a_port_that_cannot_open_is_reported() {
        let name = if cfg!(windows) {
            "COM250"
        } else {
            "/dev/pf-no-such-port"
        };
        let connection = connect(ConnectOptions::new(name)).unwrap();
        let events = collect_until(&connection, |e| {
            matches!(e, DeviceEvent::Disconnected { .. })
        });
        assert_eq!(
            events[0],
            DeviceEvent::ConnectionChanged(ConnectionState::Opening)
        );
        match events.last() {
            Some(DeviceEvent::Disconnected { reason }) => {
                assert!(reason.starts_with(&format!("could not open {name}")));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dropping_the_connection_closes_the_port() {
        let (transport, handle) = scripted_grbl();
        handle.push(BANNER);
        let connection = start(move || Ok(transport), POLL).unwrap();
        collect_until(&connection, |e| {
            matches!(
                e,
                DeviceEvent::ConnectionChanged(ConnectionState::Ready { .. })
            )
        });
        drop(connection);
        wait_for_close(|| handle.is_closed());
    }

    #[test]
    fn poll_rate_is_clamped() {
        let mut options = ConnectOptions::new("COM3");
        assert_eq!(options.poll_interval(), Duration::from_millis(200));
        options.poll_hz = 0;
        assert_eq!(options.poll_interval(), Duration::from_secs(1));
        options.poll_hz = 50;
        assert_eq!(options.poll_interval(), Duration::from_millis(100));
    }

    #[test]
    fn split_requests_rejects_anything_else() {
        assert_eq!(
            split_requests(b"?$$\n$I\n$G\n??"),
            Ok(vec![
                ReadOnlyRequest::StatusQuery,
                ReadOnlyRequest::ViewSettings,
                ReadOnlyRequest::BuildInfo,
                ReadOnlyRequest::ParserState,
                ReadOnlyRequest::StatusQuery,
                ReadOnlyRequest::StatusQuery,
            ])
        );
        assert!(split_requests(b"?$X\n").is_err());
        assert!(split_requests(b"$H\n").is_err());
        assert!(split_requests(b"~").is_err());
    }
}
