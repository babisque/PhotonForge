//! Bytes to and from the controller.
//!
//! Everything that can write is `pub(crate)`: code outside this crate can
//! only send the fixed `ReadOnlyRequest` commands through a connection.

use std::io;
use std::time::Duration;

use serialport::{DataBits, FlowControl, Parity, SerialPortType, StopBits};

use crate::error::DeviceError;

/// How long one `read` waits for data. Short, so the connection thread
/// stays responsive to requests and closing.
const READ_TIMEOUT: Duration = Duration::from_millis(20);

pub const DEFAULT_BAUD: u32 = 115_200;

/// Rates offered in the GUI. GRBL 1.1 defaults to 115200.
pub const COMMON_BAUD_RATES: [u32; 8] = [
    9_600, 19_200, 38_400, 57_600, 115_200, 230_400, 250_000, 460_800,
];

/// How to open a serial port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialSettings {
    pub port: String,
    pub baud: u32,
    /// Assert DTR after opening. Off by default: on Arduino-based boards a
    /// DTR edge resets GRBL. Some USB boards only send data with DTR on.
    /// See `docs/hardware/dtr.md` for what each OS does on open.
    pub dtr: bool,
}

impl SerialSettings {
    pub fn new(port: impl Into<String>) -> SerialSettings {
        SerialSettings {
            port: port.into(),
            baud: DEFAULT_BAUD,
            dtr: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    /// What to open, e.g. `COM3` or `/dev/ttyUSB0`.
    pub name: String,
    /// E.g. `USB-SERIAL CH340 (1A86:7523)`, when the OS reports it.
    pub description: Option<String>,
}

/// Serial ports present right now, sorted by name.
pub fn list_ports() -> Result<Vec<PortInfo>, DeviceError> {
    let mut ports: Vec<PortInfo> = serialport::available_ports()
        .map_err(|err| DeviceError::ListPorts(err.to_string()))?
        .into_iter()
        .map(|p| PortInfo {
            description: describe(&p.port_type),
            name: p.port_name,
        })
        .collect();
    ports.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ports)
}

fn describe(port_type: &SerialPortType) -> Option<String> {
    match port_type {
        SerialPortType::UsbPort(usb) => {
            let ids = format!("{:04X}:{:04X}", usb.vid, usb.pid);
            Some(match usb.product.as_ref().or(usb.manufacturer.as_ref()) {
                Some(name) => format!("{name} ({ids})"),
                None => format!("USB ({ids})"),
            })
        }
        SerialPortType::BluetoothPort => Some(String::from("Bluetooth")),
        SerialPortType::PciPort | SerialPortType::Unknown => None,
    }
}

/// A byte channel to the controller.
pub(crate) trait Transport: Send {
    /// Reads what has arrived. `Ok(0)` means nothing arrived in time; an
    /// error means the link is gone.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()>;
}

pub(crate) struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
}

impl SerialTransport {
    /// Opens the port 8N1 without flow control. DTR is driven to
    /// `settings.dtr` right after opening (best effort, see the docs).
    pub(crate) fn open(settings: &SerialSettings) -> Result<SerialTransport, DeviceError> {
        let port = serialport::new(&settings.port, settings.baud)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(StopBits::One)
            .flow_control(FlowControl::None)
            .dtr_on_open(settings.dtr)
            .timeout(READ_TIMEOUT)
            .open()
            .map_err(|err| DeviceError::Open {
                port: settings.port.clone(),
                reason: err.to_string(),
            })?;
        Ok(SerialTransport { port })
    }
}

impl Transport for SerialTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.port.read(buf) {
            Err(err) if err.kind() == io::ErrorKind::TimedOut => Ok(0),
            other => other,
        }
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.port.write_all(bytes)?;
        self.port.flush()
    }
}

/// A scripted controller for tests.
#[cfg(test)]
pub(crate) mod mock {
    use std::collections::VecDeque;
    use std::io;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::Transport;

    #[derive(Debug)]
    enum Incoming {
        Bytes(Vec<u8>),
        Fail(io::ErrorKind),
    }

    #[derive(Debug, Default)]
    struct Shared {
        incoming: VecDeque<Incoming>,
        written: Vec<u8>,
        /// When exactly these bytes are written, queue this reply.
        replies: Vec<(Vec<u8>, Vec<u8>)>,
        fail_writes: Option<io::ErrorKind>,
    }

    /// The transport side, moved into the connection under test.
    pub(crate) struct MockTransport {
        shared: Arc<Mutex<Shared>>,
    }

    /// The test's side: feeds data in and inspects what was written.
    #[derive(Clone)]
    pub(crate) struct MockHandle {
        shared: Arc<Mutex<Shared>>,
    }

    pub(crate) fn mock() -> (MockTransport, MockHandle) {
        let shared = Arc::new(Mutex::new(Shared::default()));
        (
            MockTransport {
                shared: Arc::clone(&shared),
            },
            MockHandle { shared },
        )
    }

    impl MockHandle {
        /// Bytes the controller sends next, as one read.
        pub(crate) fn push(&self, bytes: &str) {
            let mut shared = self.shared.lock().unwrap();
            shared
                .incoming
                .push_back(Incoming::Bytes(bytes.as_bytes().to_vec()));
        }

        /// The next read fails, as when the cable is pulled.
        pub(crate) fn fail_next_read(&self, kind: io::ErrorKind) {
            self.shared
                .lock()
                .unwrap()
                .incoming
                .push_back(Incoming::Fail(kind));
        }

        pub(crate) fn fail_writes(&self, kind: io::ErrorKind) {
            self.shared.lock().unwrap().fail_writes = Some(kind);
        }

        /// Whenever `request` is written, the controller answers `reply`.
        pub(crate) fn reply_to(&self, request: &str, reply: &str) {
            self.shared
                .lock()
                .unwrap()
                .replies
                .push((request.as_bytes().to_vec(), reply.as_bytes().to_vec()));
        }

        /// True once the transport was dropped, i.e. the port is closed.
        pub(crate) fn is_closed(&self) -> bool {
            Arc::strong_count(&self.shared) == 1
        }

        pub(crate) fn written(&self) -> Vec<u8> {
            self.shared.lock().unwrap().written.clone()
        }
    }

    impl Transport for MockTransport {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let next = self.shared.lock().unwrap().incoming.pop_front();
            match next {
                Some(Incoming::Bytes(bytes)) => {
                    let n = bytes.len().min(buf.len());
                    buf[..n].copy_from_slice(&bytes[..n]);
                    if n < bytes.len() {
                        let rest = Incoming::Bytes(bytes[n..].to_vec());
                        self.shared.lock().unwrap().incoming.push_front(rest);
                    }
                    Ok(n)
                }
                Some(Incoming::Fail(kind)) => Err(io::Error::from(kind)),
                None => {
                    // Like a serial read timing out with no data.
                    std::thread::sleep(Duration::from_millis(1));
                    Ok(0)
                }
            }
        }

        fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
            let mut shared = self.shared.lock().unwrap();
            if let Some(kind) = shared.fail_writes {
                return Err(io::Error::from(kind));
            }
            shared.written.extend_from_slice(bytes);
            let replies: Vec<Vec<u8>> = shared
                .replies
                .iter()
                .filter(|(request, _)| request.as_slice() == bytes)
                .map(|(_, reply)| reply.clone())
                .collect();
            shared
                .incoming
                .extend(replies.into_iter().map(Incoming::Bytes));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::mock;
    use super::*;
    use serialport::UsbPortInfo;

    fn usb(product: Option<&str>, manufacturer: Option<&str>) -> SerialPortType {
        SerialPortType::UsbPort(UsbPortInfo {
            vid: 0x1a86,
            pid: 0x7523,
            serial_number: None,
            manufacturer: manufacturer.map(str::to_owned),
            product: product.map(str::to_owned),
        })
    }

    #[test]
    fn usb_ports_are_described_with_ids() {
        assert_eq!(
            describe(&usb(Some("USB-SERIAL CH340"), Some("wch.cn"))),
            Some("USB-SERIAL CH340 (1A86:7523)".into())
        );
        assert_eq!(
            describe(&usb(None, Some("wch.cn"))),
            Some("wch.cn (1A86:7523)".into())
        );
        assert_eq!(describe(&usb(None, None)), Some("USB (1A86:7523)".into()));
        assert_eq!(
            describe(&SerialPortType::BluetoothPort),
            Some("Bluetooth".into())
        );
        assert_eq!(describe(&SerialPortType::PciPort), None);
    }

    #[test]
    fn settings_default_to_115200_without_dtr() {
        let s = SerialSettings::new("COM3");
        assert_eq!((s.port.as_str(), s.baud, s.dtr), ("COM3", 115_200, false));
        assert!(COMMON_BAUD_RATES.contains(&DEFAULT_BAUD));
    }

    #[test]
    fn listing_ports_works_without_hardware() {
        let ports = list_ports().unwrap();
        let mut sorted = ports.clone();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(ports, sorted);
    }

    #[test]
    fn opening_a_missing_port_is_an_error_not_a_panic() {
        let name = if cfg!(windows) {
            "COM250"
        } else {
            "/dev/pf-no-such-port"
        };
        match SerialTransport::open(&SerialSettings::new(name)) {
            Err(DeviceError::Open { port, reason }) => {
                assert_eq!(port, name);
                assert!(!reason.is_empty());
            }
            Err(other) => panic!("unexpected error {other:?}"),
            Ok(_) => panic!("{name} unexpectedly exists"),
        }
    }

    #[test]
    fn mock_replays_reads_and_records_writes() {
        let (mut transport, handle) = mock();
        handle.reply_to("$I\n", "[VER:1.1h.20190825:]\r\nok\r\n");
        handle.push("Grbl 1.1h ['$' for help]\r\n");

        let mut buf = [0u8; 8];
        let n = transport.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"Grbl 1.1");
        // A long chunk is delivered across several reads.
        let mut rest = Vec::new();
        loop {
            let n = transport.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            rest.extend_from_slice(&buf[..n]);
        }
        assert_eq!(rest, b"h ['$' for help]\r\n");

        transport.write_all(b"?").unwrap();
        transport.write_all(b"$I\n").unwrap();
        assert_eq!(handle.written(), b"?$I\n");
        let mut reply = [0u8; 64];
        let n = transport.read(&mut reply).unwrap();
        assert_eq!(&reply[..n], b"[VER:1.1h.20190825:]\r\nok\r\n");
    }

    #[test]
    fn mock_simulates_unplugging() {
        let (mut transport, handle) = mock();
        handle.fail_next_read(io::ErrorKind::BrokenPipe);
        let mut buf = [0u8; 4];
        assert_eq!(
            transport.read(&mut buf).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        handle.fail_writes(io::ErrorKind::PermissionDenied);
        assert!(transport.write_all(b"?").is_err());
        assert!(handle.written().is_empty());
    }
}
