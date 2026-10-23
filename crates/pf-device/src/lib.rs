//! Device communication for PhotonForge: GRBL serial connection, status, and job streaming.
//!
//! Phase 5 is read-only: nothing here can move the machine or turn on the
//! laser.

mod connection;
mod error;
mod event;
pub mod protocol;
mod session;
mod snapshot;
mod transport;

pub use connection::{ConnectOptions, Connection, DEFAULT_POLL_HZ, POLL_HZ_RANGE, connect};
pub use error::DeviceError;
pub use event::{ConnectionState, DeviceEvent};
pub use protocol::ParseError;
pub use protocol::build_info::{
    BuildInfo, BuildOptions, Version, is_supported_version, option_info, parse_options,
    parse_version,
};
pub use protocol::codes::{CodeInfo, alarm_info, error_info};
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
pub use transport::{COMMON_BAUD_RATES, DEFAULT_BAUD, PortInfo, SerialSettings, list_ports};
