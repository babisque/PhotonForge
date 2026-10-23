//! Classifies one line received from GRBL.
//!
//! Message types: GRBL v1.1h `doc/markdown/interface.md`, sections "Message
//! Summary", "Grbl Response Messages" and "Grbl Push Messages".

use super::ParseError;
use super::status::{StatusReport, parse_status};

/// One line from GRBL. Unrecognized lines are kept, never dropped, so the
/// console can always show what the controller said.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    /// The last line command was executed.
    Ok,
    /// The last line command was rejected; see `codes::error_info`.
    Error(u8),
    /// GRBL entered the alarm state; see `codes::alarm_info`.
    Alarm(u8),
    Status(StatusReport),
    /// `Grbl 1.1h ['$' for help]`, printed at power-up and after a reset.
    Welcome {
        version: String,
    },
    /// `$x=val` from a `$$` query.
    Setting {
        number: u16,
        value: String,
    },
    /// `$Nx=line`: a stored startup line, from a `$N` query.
    StartupLineSetting {
        index: u8,
        line: String,
    },
    /// `[MSG:…]`, a feedback message that was not queried.
    Message(String),
    /// `[GC:…]`, the parser state from a `$G` query.
    ParserState(String),
    /// `[VER:…]`, the build info from a `$I` query.
    Version(String),
    /// `[OPT:…]`, the build options from a `$I` query.
    Options(String),
    /// `[echo:…]`, a line echo (compile-time option).
    Echo(String),
    /// Any other `[TYPE:data]`, e.g. `[HLP:…]`, `[G54:…]` or `[PRB:…]`.
    Feedback {
        kind: String,
        data: String,
    },
    /// `>G54G20:ok` or `>G54G20:error:X`: a startup line was executed.
    StartupLineRun {
        line: String,
        error: Option<u8>,
    },
    /// Looked like a known message type but could not be read.
    Malformed {
        line: String,
        error: ParseError,
    },
    /// Anything else, including blank lines and other firmware.
    Unknown(String),
}

pub fn parse_line(raw: &str) -> Response {
    let line = raw.trim();
    if line == "ok" {
        return Response::Ok;
    }
    if let Some(code) = line.strip_prefix("error:") {
        return code_or_malformed(line, "error", code, Response::Error);
    }
    if let Some(code) = line.strip_prefix("ALARM:") {
        return code_or_malformed(line, "ALARM", code, Response::Alarm);
    }
    if line.starts_with('<') {
        return match parse_status(line) {
            Ok(report) => Response::Status(report),
            Err(error) => malformed(line, error),
        };
    }
    if let Some(rest) = line.strip_prefix('>') {
        return startup_line_run(line, rest);
    }
    if let Some(rest) = line.strip_prefix('$') {
        return setting(line, rest);
    }
    if let Some(inner) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return feedback(inner);
    }
    if let Some(version) = welcome_version(line) {
        return Response::Welcome { version };
    }
    Response::Unknown(line.to_owned())
}

/// `Grbl X.Xx ['$' for help]` gives `X.Xx`. Forks with another name are
/// not matched (e.g. `GrblHAL 1.1f …`).
fn welcome_version(line: &str) -> Option<String> {
    let rest = line.strip_prefix("Grbl ")?;
    let version = rest.split_whitespace().next()?;
    let digit_first = version.chars().next().is_some_and(|c| c.is_ascii_digit());
    digit_first.then(|| version.to_owned())
}

fn code_or_malformed(
    line: &str,
    field: &'static str,
    code: &str,
    make: fn(u8) -> Response,
) -> Response {
    match code.parse::<u8>() {
        Ok(code) => make(code),
        Err(_) => malformed(
            line,
            ParseError::BadValue {
                field,
                value: code.to_owned(),
            },
        ),
    }
}

/// `$130=300.000` or `$N0=G54G21`. Section "Grbl `$` Settings Message".
fn setting(line: &str, rest: &str) -> Response {
    let Some((key, value)) = rest.split_once('=') else {
        return Response::Unknown(line.to_owned());
    };
    if let Some(index) = key.strip_prefix('N') {
        return match index.parse::<u8>() {
            Ok(index) => Response::StartupLineSetting {
                index,
                line: value.to_owned(),
            },
            Err(_) => malformed(
                line,
                ParseError::BadValue {
                    field: "$N",
                    value: index.to_owned(),
                },
            ),
        };
    }
    match key.parse::<u16>() {
        Ok(number) => Response::Setting {
            number,
            value: value.to_owned(),
        },
        Err(_) => malformed(
            line,
            ParseError::BadValue {
                field: "$",
                value: key.to_owned(),
            },
        ),
    }
}

/// `[TYPE:data]`. Section "Feedback Messages".
fn feedback(inner: &str) -> Response {
    let (kind, data) = inner.split_once(':').unwrap_or((inner, ""));
    let data = data.to_owned();
    match kind {
        "MSG" => Response::Message(data),
        "GC" => Response::ParserState(data),
        "VER" => Response::Version(data),
        "OPT" => Response::Options(data),
        "echo" => Response::Echo(data),
        _ => Response::Feedback {
            kind: kind.to_owned(),
            data,
        },
    }
}

/// `>line:ok` or `>line:error:X`. Section "Startup Line Execution".
fn startup_line_run(line: &str, rest: &str) -> Response {
    if let Some(executed) = rest.strip_suffix(":ok") {
        return Response::StartupLineRun {
            line: executed.to_owned(),
            error: None,
        };
    }
    if let Some((executed, code)) = rest.rsplit_once(":error:") {
        return match code.parse::<u8>() {
            Ok(code) => Response::StartupLineRun {
                line: executed.to_owned(),
                error: Some(code),
            },
            Err(_) => malformed(
                line,
                ParseError::BadValue {
                    field: "error",
                    value: code.to_owned(),
                },
            ),
        };
    }
    Response::Unknown(line.to_owned())
}

fn malformed(line: &str, error: ParseError) -> Response {
    Response::Malformed {
        line: line.to_owned(),
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::status::MachineState;

    fn text(s: &str) -> String {
        s.to_owned()
    }

    #[test]
    fn parse_classifies_real_grbl_output() {
        let cases = [
            ("ok", Response::Ok),
            ("ok\r\n", Response::Ok),
            ("error:20", Response::Error(20)),
            ("error:9\r", Response::Error(9)),
            ("ALARM:2", Response::Alarm(2)),
            (
                "Grbl 1.1h ['$' for help]",
                Response::Welcome {
                    version: text("1.1h"),
                },
            ),
            (
                "Grbl 0.9j ['$' for help]",
                Response::Welcome {
                    version: text("0.9j"),
                },
            ),
            (
                "$130=300.000",
                Response::Setting {
                    number: 130,
                    value: text("300.000"),
                },
            ),
            (
                "$32=1",
                Response::Setting {
                    number: 32,
                    value: text("1"),
                },
            ),
            (
                "$N0=G54G21",
                Response::StartupLineSetting {
                    index: 0,
                    line: text("G54G21"),
                },
            ),
            (
                "$N1=",
                Response::StartupLineSetting {
                    index: 1,
                    line: String::new(),
                },
            ),
            (
                "[MSG:'$H'|'$X' to unlock]",
                Response::Message(text("'$H'|'$X' to unlock")),
            ),
            (
                "[MSG:Reset to continue]",
                Response::Message(text("Reset to continue")),
            ),
            (
                "[GC:G0 G54 G17 G21 G90 G94 M5 M9 T0 F0 S0]",
                Response::ParserState(text("G0 G54 G17 G21 G90 G94 M5 M9 T0 F0 S0")),
            ),
            (
                "[VER:1.1h.20190825:]",
                Response::Version(text("1.1h.20190825:")),
            ),
            ("[OPT:V,15,128]", Response::Options(text("V,15,128"))),
            (
                "[echo:G1X0.540Y10.4F100]",
                Response::Echo(text("G1X0.540Y10.4F100")),
            ),
            (
                "[HLP:$$ $# $G $I $N $x=val $Nx=line $J=line $SLP $C $X $H ~ ! ? ctrl-x]",
                Response::Feedback {
                    kind: text("HLP"),
                    data: text("$$ $# $G $I $N $x=val $Nx=line $J=line $SLP $C $X $H ~ ! ? ctrl-x"),
                },
            ),
            (
                "[G54:0.000,0.000,0.000]",
                Response::Feedback {
                    kind: text("G54"),
                    data: text("0.000,0.000,0.000"),
                },
            ),
            (
                "[PRB:0.000,0.000,1.492:1]",
                Response::Feedback {
                    kind: text("PRB"),
                    data: text("0.000,0.000,1.492:1"),
                },
            ),
            (
                ">G54G20:ok",
                Response::StartupLineRun {
                    line: text("G54G20"),
                    error: None,
                },
            ),
            (
                ">G54G20:error:20",
                Response::StartupLineRun {
                    line: text("G54G20"),
                    error: Some(20),
                },
            ),
            (
                ">:error:7",
                Response::StartupLineRun {
                    line: String::new(),
                    error: Some(7),
                },
            ),
        ];
        for (line, expected) in cases {
            assert_eq!(parse_line(line), expected, "{line:?}");
        }
    }

    #[test]
    fn parse_status_lines_become_reports() {
        match parse_line("<Idle|MPos:0.000,0.000,0.000|FS:0,0|WCO:0.000,0.000,0.000>\r\n") {
            Response::Status(report) => assert_eq!(report.state, MachineState::Idle),
            other => panic!("expected a status report, got {other:?}"),
        }
    }

    #[test]
    fn parse_malformed_known_types_keep_the_line_and_reason() {
        let cases = [
            ("error:abc", "error", "abc"),
            ("error:", "error", ""),
            ("ALARM:999", "ALARM", "999"),
            ("$1x=5", "$", "1x"),
            ("$Nq=G54", "$N", "q"),
            (">G0:error:z", "error", "z"),
        ];
        for (line, field, value) in cases {
            assert_eq!(
                parse_line(line),
                Response::Malformed {
                    line: text(line),
                    error: ParseError::BadValue {
                        field,
                        value: text(value),
                    },
                },
                "{line:?}"
            );
        }
        assert!(matches!(
            parse_line("<Flying|MPos:0,0,0>"),
            Response::Malformed {
                error: ParseError::UnknownState(_),
                ..
            }
        ));
    }

    #[test]
    fn parse_everything_else_is_unknown() {
        let cases = [
            "",
            "   ",
            "GrblHAL 1.1f ['$' or '$HELP' for help]",
            "Grbl help",
            "okay",
            "$$",
            ">G54",
            "Marlin 2.1",
            "[unterminated",
        ];
        for line in cases {
            assert_eq!(
                parse_line(line),
                Response::Unknown(text(line.trim())),
                "{line:?}"
            );
        }
    }
}
