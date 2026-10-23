use crate::protocol::ParseError;
use crate::protocol::build_info::BuildInfo;
use crate::protocol::parse::Response;
use crate::protocol::parser_state::ParserState;
use crate::protocol::readonly::ReadOnlyRequest;
use crate::protocol::settings::GrblSettings;
use crate::protocol::status::{Positions, StatusReport};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// Opening the serial port.
    Opening,
    /// The port is open; waiting for GRBL to identify itself.
    Identifying,
    /// A GRBL 1.1 controller answered.
    Ready { version: String },
}

/// Everything a connection reports, in the order it happened.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceEvent {
    ConnectionChanged(ConnectionState),
    /// The connection ended; no more events follow.
    Disconnected {
        reason: String,
    },
    /// A line from the controller, for the console.
    Received {
        line: String,
        response: Response,
    },
    /// A request written to the controller, for the console.
    Sent(ReadOnlyRequest),
    Status {
        report: StatusReport,
        /// Machine and work positions, using the last known WCO.
        positions: Positions,
    },
    SettingsLoaded {
        settings: GrblSettings,
        /// Settings whose value could not be read; the others loaded.
        errors: Vec<ParseError>,
    },
    BuildInfoLoaded(BuildInfo),
    ParserStateLoaded(ParserState),
    /// A request was answered with `error:code`.
    CommandFailed {
        request: ReadOnlyRequest,
        code: u8,
    },
    /// A request got no answer in time and was given up.
    RequestTimedOut(ReadOnlyRequest),
    Alarm(u8),
    /// A `[MSG:…]` feedback message.
    Message(String),
    /// GRBL printed its welcome message again: it was reset (e.g. by its
    /// reset button or a DTR pulse). Pending requests were dropped.
    ControllerReset,
    /// No status report for a while (GRBL ignores `?` while homing).
    Unresponsive,
    /// Status reports arrive again after `Unresponsive`.
    Responsive,
    /// Something was received that could not be used, e.g. a malformed
    /// `[VER:]` line.
    Warning(String),
}
