//! Device communication for PhotonForge: GRBL serial connection, status, and job streaming.
//!
//! Everything that can be written to the controller is a closed set
//! (`protocol::outgoing`), written from one place on the connection thread.
//! Without a `ControlRequest` only read-only requests are ever sent.

mod connection;
mod control;
mod error;
mod event;
mod job;
mod jog;
mod preflight;
pub mod protocol;
mod session;
#[cfg(test)]
mod sim;
mod snapshot;
mod stream;
mod transport;

pub use connection::{ConnectOptions, Connection, DEFAULT_POLL_HZ, POLL_HZ_RANGE, connect};
pub use control::{ControlRequest, JobRequest};
pub use error::DeviceError;
pub use event::{ConnectionState, DeviceEvent};
pub use job::{
    FaultReason, InvalidTransition, JobEvent, JobKind, JobProgress, JobState, StopStage, transition,
};
pub use jog::{JogError, JogMove};
pub use preflight::{JobSummary, PreflightError, ScanError, preflight, scan_job};
pub use protocol::ParseError;
pub use protocol::build_info::{
    BuildInfo, BuildOptions, Version, is_supported_version, option_info, parse_options,
    parse_version,
};
pub use protocol::codes::{CodeInfo, alarm_info, error_info};
pub use protocol::outgoing::Realtime;
pub use protocol::parse::{Response, parse_line};
pub use protocol::parser_state::{
    DistanceMode, ParserState, SpindleState, Units, parse_parser_state,
};
pub use protocol::profile_check::{ProfileHint, compare_with_profile};
pub use protocol::readonly::ReadOnlyRequest;
pub use protocol::settings::{
    GrblSettings, Setting, SettingInfo, SettingValue, StatusReportMask, setting_info,
};
pub use protocol::status::{
    BufferState, MachineState, Overrides, Positions, ReportedPosition, StatusReport, parse_status,
};
pub use snapshot::{DeviceSnapshot, LinkState};
pub use stream::{DEFAULT_RX_BUFFER_BYTES, StreamError, StreamMode, Streamer, rx_capacity};
pub use transport::{COMMON_BAUD_RATES, DEFAULT_BAUD, PortInfo, SerialSettings, list_ports};
