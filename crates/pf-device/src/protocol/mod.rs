//! Parsing what GRBL v1.1 sends. Pure functions, no I/O.
//!
//! Reference: GRBL v1.1h (tag `v1.1h.20190825`), `doc/markdown/interface.md`.

pub mod build_info;
pub mod codes;
pub mod parse;
pub mod parser_state;
pub mod profile_check;
pub mod readonly;
pub mod settings;
pub mod status;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum ParseError {
    #[error("not a status report: it must be enclosed in '<' and '>'")]
    NotAStatusReport,
    #[error("unknown machine state {0:?}")]
    UnknownState(String),
    #[error("the status report has no MPos or WPos field")]
    MissingPosition,
    #[error("{field} has {found} value(s), expected {expected}")]
    WrongValueCount {
        field: &'static str,
        expected: usize,
        found: usize,
    },
    #[error("{field} has an invalid value {value:?}")]
    BadValue { field: &'static str, value: String },
    #[error("${number}={value:?} is not a valid {expected}")]
    BadSetting {
        number: u16,
        value: String,
        expected: &'static str,
    },
}
