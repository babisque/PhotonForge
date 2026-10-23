//! A connection to a GRBL controller. One thread owns the serial port and
//! runs the `Session`; the app talks to it through channels: read-only
//! requests in, `DeviceEvent`s out. Dropping the `Connection` closes the port.

use std::ops::RangeInclusive;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::DeviceError;
use crate::event::{ConnectionState, DeviceEvent};
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

/// A live connection. The only thing it can send is a `ReadOnlyRequest`.
#[derive(Debug)]
pub struct Connection {
    requests: Sender<ReadOnlyRequest>,
    events: Receiver<DeviceEvent>,
}

impl Connection {
    /// Asks the controller for a report. Returns false once the connection
    /// has ended.
    pub fn request(&self, request: ReadOnlyRequest) -> bool {
        self.requests.send(request).is_ok()
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
            if let Some(reason) = run(transport, poll_interval, &request_rx, &event_tx) {
                let _ = event_tx.send(DeviceEvent::Disconnected { reason });
            }
        })
        .map_err(|err| DeviceError::Thread(err.to_string()))?;
    Ok(Connection {
        requests: request_tx,
        events: event_rx,
    })
}

/// Runs until the link fails (returns why) or the app drops the connection
/// (returns `None`). Each pass waits at most one transport read timeout.
fn run<T: Transport>(
    mut transport: T,
    poll_interval: Duration,
    requests: &Receiver<ReadOnlyRequest>,
    events: &Sender<DeviceEvent>,
) -> Option<String> {
    let mut session = Session::new(poll_interval, Instant::now());
    let mut buf = [0u8; 512];
    loop {
        let mut actions = Vec::new();
        loop {
            match requests.try_recv() {
                Ok(request) => actions.extend(session.request(request, Instant::now())),
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
                Action::Send(request) => {
                    // The only write in this crate outside tests.
                    if let Err(err) = transport.write_all(request.bytes()) {
                        return Some(format!("Connection lost: {err}"));
                    }
                    if events.send(DeviceEvent::Sent(request)).is_err() {
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

    fn wait_for_close(handle: &MockHandle) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !handle.is_closed() {
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
        wait_for_close(&handle);
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
        wait_for_close(&handle);
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
        wait_for_close(&handle);
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
        wait_for_close(&handle);
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
