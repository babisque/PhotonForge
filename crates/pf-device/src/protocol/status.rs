//! GRBL v1.1 real-time status reports, e.g.
//! `<Idle|MPos:0.000,0.000,0.000|FS:0,0|WCO:0.000,0.000,0.000>`.
//!
//! Format: GRBL v1.1h `doc/markdown/interface.md`, "Real-time Status
//! Reports". Values are in mm, or inches when `$13=1`.

use super::ParseError;

/// Machine state, the first field. Section "Machine State".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineState {
    Idle,
    Run,
    /// `Hold:0` hold complete, `Hold:1` hold in progress.
    Hold(Option<u8>),
    Jog,
    Alarm,
    /// `Door:0`–`Door:3`, see the GRBL docs for each sub-state.
    Door(Option<u8>),
    Check,
    Home,
    Sleep,
}

/// The one position vector a report carries, chosen by `$10`. Section
/// "Current Position".
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReportedPosition {
    Machine([f64; 3]),
    Work([f64; 3]),
}

/// Section "Buffer State": free space, not space in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferState {
    pub planner_blocks_free: u16,
    pub rx_bytes_free: u16,
}

/// Section "Override Values", in percent of the programmed values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overrides {
    pub feed: u16,
    pub rapid: u16,
    pub spindle: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatusReport {
    pub state: MachineState,
    pub position: ReportedPosition,
    /// Sent only every 10–30 reports and on change: keep the last one.
    pub wco: Option<[f64; 3]>,
    pub buffer: Option<BufferState>,
    pub line_number: Option<u32>,
    pub feed: Option<f64>,    // mm/min (or in/min with $13=1)
    pub spindle: Option<f64>, // S value; absent with the `F:` form
    /// Triggered input pins, letters from `XYZPDHRS`, in GRBL's order.
    pub pins: Option<String>,
    pub overrides: Option<Overrides>,
    /// Active accessories, letters from `SCFM`.
    pub accessories: Option<String>,
    /// Fields this parser does not know, kept as sent (e.g. from forks).
    pub other_fields: Vec<String>,
}

/// Both position vectors, where they can be known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Positions {
    pub machine: Option<[f64; 3]>,
    pub work: Option<[f64; 3]>,
}

impl StatusReport {
    /// Machine and work positions using `WPos = MPos - WCO` (section "Work
    /// Coordinate Offset"). The report's own `WCO` wins over `last_wco`;
    /// without either, only the reported vector is known.
    pub fn positions(&self, last_wco: Option<[f64; 3]>) -> Positions {
        let wco = self.wco.or(last_wco);
        match self.position {
            ReportedPosition::Machine(m) => Positions {
                machine: Some(m),
                work: wco.map(|o| [m[0] - o[0], m[1] - o[1], m[2] - o[2]]),
            },
            ReportedPosition::Work(w) => Positions {
                machine: wco.map(|o| [w[0] + o[0], w[1] + o[1], w[2] + o[2]]),
                work: Some(w),
            },
        }
    }
}

/// Parses one status report line, chevrons included.
pub fn parse_status(line: &str) -> Result<StatusReport, ParseError> {
    let body = line
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .ok_or(ParseError::NotAStatusReport)?;
    let mut fields = body.split('|');
    // The first two fields are always present, in this order.
    let state = parse_state(fields.next().unwrap_or(""))?;
    let position = match fields.next().and_then(|f| f.split_once(':')) {
        Some(("MPos", values)) => ReportedPosition::Machine(axes("MPos", values)?),
        Some(("WPos", values)) => ReportedPosition::Work(axes("WPos", values)?),
        _ => return Err(ParseError::MissingPosition),
    };

    let mut report = StatusReport {
        state,
        position,
        wco: None,
        buffer: None,
        line_number: None,
        feed: None,
        spindle: None,
        pins: None,
        overrides: None,
        accessories: None,
        other_fields: Vec::new(),
    };
    // The remaining fields may be absent and come in any order.
    for field in fields {
        let Some((name, values)) = field.split_once(':') else {
            report.other_fields.push(field.to_owned());
            continue;
        };
        match name {
            "WCO" => report.wco = Some(axes("WCO", values)?),
            "Bf" => {
                let [planner, rx] = integers::<2>("Bf", values)?;
                report.buffer = Some(BufferState {
                    planner_blocks_free: planner,
                    rx_bytes_free: rx,
                });
            }
            "Ln" => report.line_number = Some(number("Ln", values)?),
            "F" => report.feed = Some(decimal("F", values)?),
            "FS" => {
                let [feed, spindle] = decimals::<2>("FS", values)?;
                report.feed = Some(feed);
                report.spindle = Some(spindle);
            }
            "Pn" => report.pins = Some(letters("Pn", values, "XYZPDHRSA")?),
            "Ov" => {
                let [feed, rapid, spindle] = integers::<3>("Ov", values)?;
                report.overrides = Some(Overrides {
                    feed,
                    rapid,
                    spindle,
                });
            }
            "A" => report.accessories = Some(letters("A", values, "SCFM")?),
            _ => report.other_fields.push(field.to_owned()),
        }
    }
    Ok(report)
}

fn parse_state(field: &str) -> Result<MachineState, ParseError> {
    let (name, sub) = match field.split_once(':') {
        Some((name, sub)) => {
            let sub = sub
                .parse::<u8>()
                .map_err(|_| ParseError::UnknownState(field.to_owned()))?;
            (name, Some(sub))
        }
        None => (field, None),
    };
    let state = match (name, sub) {
        ("Idle", None) => MachineState::Idle,
        ("Run", None) => MachineState::Run,
        ("Hold", sub) => MachineState::Hold(sub),
        ("Jog", None) => MachineState::Jog,
        ("Alarm", None) => MachineState::Alarm,
        ("Door", sub) => MachineState::Door(sub),
        ("Check", None) => MachineState::Check,
        ("Home", None) => MachineState::Home,
        ("Sleep", None) => MachineState::Sleep,
        _ => return Err(ParseError::UnknownState(field.to_owned())),
    };
    Ok(state)
}

/// X, Y and Z. Later GRBL versions may add an A axis, which is ignored.
fn axes(field: &'static str, values: &str) -> Result<[f64; 3], ParseError> {
    let parsed = values
        .split(',')
        .map(|v| parse_decimal(field, v))
        .collect::<Result<Vec<f64>, ParseError>>()?;
    match parsed[..] {
        [x, y, z, ..] => Ok([x, y, z]),
        _ => Err(ParseError::WrongValueCount {
            field,
            expected: 3,
            found: parsed.len(),
        }),
    }
}

fn decimals<const N: usize>(field: &'static str, values: &str) -> Result<[f64; N], ParseError> {
    let parsed = values
        .split(',')
        .map(|v| parse_decimal(field, v))
        .collect::<Result<Vec<f64>, ParseError>>()?;
    let found = parsed.len();
    parsed.try_into().map_err(|_| ParseError::WrongValueCount {
        field,
        expected: N,
        found,
    })
}

fn integers<const N: usize>(field: &'static str, values: &str) -> Result<[u16; N], ParseError> {
    let parsed = values
        .split(',')
        .map(|v| number(field, v))
        .collect::<Result<Vec<u16>, ParseError>>()?;
    let found = parsed.len();
    parsed.try_into().map_err(|_| ParseError::WrongValueCount {
        field,
        expected: N,
        found,
    })
}

fn decimal(field: &'static str, value: &str) -> Result<f64, ParseError> {
    decimals::<1>(field, value).map(|[v]| v)
}

fn number<T: std::str::FromStr>(field: &'static str, value: &str) -> Result<T, ParseError> {
    value.parse().map_err(|_| ParseError::BadValue {
        field,
        value: value.to_owned(),
    })
}

/// GRBL prints plain decimals; anything non-finite is malformed.
fn parse_decimal(field: &'static str, value: &str) -> Result<f64, ParseError> {
    match value.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(ParseError::BadValue {
            field,
            value: value.to_owned(),
        }),
    }
}

fn letters(field: &'static str, value: &str, allowed: &str) -> Result<String, ParseError> {
    if value.is_empty() || !value.chars().all(|c| allowed.contains(c)) {
        return Err(ParseError::BadValue {
            field,
            value: value.to_owned(),
        });
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(line: &str) -> StatusReport {
        parse_status(line).unwrap()
    }

    #[test]
    fn parse_real_status_reports() {
        // (line, state, reported position)
        let cases = [
            (
                "<Idle|MPos:0.000,0.000,0.000|FS:0,0|WCO:0.000,0.000,0.000>",
                MachineState::Idle,
                ReportedPosition::Machine([0.0, 0.0, 0.0]),
            ),
            (
                "<Run|MPos:12.500,4.250,0.000|FS:1500,350|Ov:100,100,100|A:S>",
                MachineState::Run,
                ReportedPosition::Machine([12.5, 4.25, 0.0]),
            ),
            (
                "<Hold:0|WPos:-2.500,0.000,11.000|FS:0,0>",
                MachineState::Hold(Some(0)),
                ReportedPosition::Work([-2.5, 0.0, 11.0]),
            ),
            (
                "<Door:1|MPos:1.000,2.000,3.000|Bf:15,128|FS:0,0|Pn:D>",
                MachineState::Door(Some(1)),
                ReportedPosition::Machine([1.0, 2.0, 3.0]),
            ),
            (
                "<Alarm|MPos:0.000,-10.000,5.000|FS:0,0|Pn:XZ>",
                MachineState::Alarm,
                ReportedPosition::Machine([0.0, -10.0, 5.0]),
            ),
            (
                "<Jog|WPos:5.000,5.000,0.000|Ln:99999|F:500>",
                MachineState::Jog,
                ReportedPosition::Work([5.0, 5.0, 0.0]),
            ),
            (
                "<Check|MPos:0.000,0.000,0.000|FS:0,0>",
                MachineState::Check,
                ReportedPosition::Machine([0.0, 0.0, 0.0]),
            ),
            (
                "<Home|MPos:0.000,0.000,0.000|FS:0,0>",
                MachineState::Home,
                ReportedPosition::Machine([0.0, 0.0, 0.0]),
            ),
            (
                "<Sleep|MPos:0.000,0.000,0.000|FS:0,0>",
                MachineState::Sleep,
                ReportedPosition::Machine([0.0, 0.0, 0.0]),
            ),
            (
                "<Hold|MPos:0.000,0.000,0.000|FS:0,0>",
                MachineState::Hold(None),
                ReportedPosition::Machine([0.0, 0.0, 0.0]),
            ),
        ];
        for (line, state, position) in cases {
            let r = report(line);
            assert_eq!(r.state, state, "{line}");
            assert_eq!(r.position, position, "{line}");
        }
    }

    #[test]
    fn parse_optional_fields() {
        let r = report(
            "<Run|MPos:1.000,2.000,3.000|Bf:15,128|Ln:42|FS:1500.5,350|Pn:PZ|WCO:0.500,1.551,5.664|Ov:110,50,100|A:SF>",
        );
        assert_eq!(r.wco, Some([0.5, 1.551, 5.664]));
        assert_eq!(
            r.buffer,
            Some(BufferState {
                planner_blocks_free: 15,
                rx_bytes_free: 128
            })
        );
        assert_eq!(r.line_number, Some(42));
        assert_eq!(r.feed, Some(1500.5));
        assert_eq!(r.spindle, Some(350.0));
        assert_eq!(r.pins.as_deref(), Some("PZ"));
        assert_eq!(
            r.overrides,
            Some(Overrides {
                feed: 110,
                rapid: 50,
                spindle: 100
            })
        );
        assert_eq!(r.accessories.as_deref(), Some("SF"));
        assert!(r.other_fields.is_empty());

        // The `F:` form has no spindle value.
        let f = report("<Idle|MPos:0.000,0.000,0.000|F:250>");
        assert_eq!((f.feed, f.spindle), (Some(250.0), None));
        // Only the two mandatory fields.
        let bare = report("<Idle|WPos:0.000,0.000,0.000>");
        assert_eq!(bare.feed, None);
        assert_eq!(bare.wco, None);
    }

    #[test]
    fn parse_keeps_unknown_fields_and_extra_axes() {
        let r = report("<Idle|MPos:1.000,2.000,3.000,4.000|FS:0,0|SD:1,2|Flag>");
        assert_eq!(r.position, ReportedPosition::Machine([1.0, 2.0, 3.0]));
        assert_eq!(r.other_fields, vec!["SD:1,2", "Flag"]);
    }

    #[test]
    fn parse_malformed_reports_fail_with_a_reason() {
        let cases = [
            ("Idle|MPos:0,0,0", ParseError::NotAStatusReport),
            ("<Idle|MPos:0,0,0", ParseError::NotAStatusReport),
            ("<Idle>", ParseError::MissingPosition),
            ("<Idle|FS:0,0>", ParseError::MissingPosition),
            (
                "<Flying|MPos:0,0,0>",
                ParseError::UnknownState("Flying".into()),
            ),
            (
                "<Idle:1|MPos:0,0,0>",
                ParseError::UnknownState("Idle:1".into()),
            ),
            (
                "<Hold:x|MPos:0,0,0>",
                ParseError::UnknownState("Hold:x".into()),
            ),
            ("<|MPos:0,0,0>", ParseError::UnknownState(String::new())),
            (
                "<Idle|MPos:1,2>",
                ParseError::WrongValueCount {
                    field: "MPos",
                    expected: 3,
                    found: 2,
                },
            ),
            (
                "<Idle|MPos:1,x,3>",
                ParseError::BadValue {
                    field: "MPos",
                    value: "x".into(),
                },
            ),
            (
                "<Idle|MPos:1,inf,3>",
                ParseError::BadValue {
                    field: "MPos",
                    value: "inf".into(),
                },
            ),
            (
                "<Idle|MPos:0,0,0|FS:abc,0>",
                ParseError::BadValue {
                    field: "FS",
                    value: "abc".into(),
                },
            ),
            (
                "<Idle|MPos:0,0,0|Ov:100,100>",
                ParseError::WrongValueCount {
                    field: "Ov",
                    expected: 3,
                    found: 2,
                },
            ),
            (
                "<Idle|MPos:0,0,0|Pn:>",
                ParseError::BadValue {
                    field: "Pn",
                    value: String::new(),
                },
            ),
            (
                "<Idle|MPos:0,0,0|A:Q>",
                ParseError::BadValue {
                    field: "A",
                    value: "Q".into(),
                },
            ),
            (
                "<Idle|MPos:0,0,0|Bf:-1,128>",
                ParseError::BadValue {
                    field: "Bf",
                    value: "-1".into(),
                },
            ),
        ];
        for (line, expected) in cases {
            assert_eq!(parse_status(line), Err(expected), "{line}");
        }
    }

    #[test]
    fn parse_positions_follow_the_wco_equation() {
        let wco = [0.5, 1.0, -2.0];
        let m = report("<Idle|MPos:10.000,20.000,0.000|FS:0,0>");
        assert_eq!(
            m.positions(Some(wco)),
            Positions {
                machine: Some([10.0, 20.0, 0.0]),
                work: Some([9.5, 19.0, 2.0]),
            }
        );
        let w = report("<Idle|WPos:9.500,19.000,2.000|FS:0,0>");
        assert_eq!(w.positions(Some(wco)).machine, Some([10.0, 20.0, 0.0]));
        // Without any WCO only the reported vector is known.
        assert_eq!(
            m.positions(None),
            Positions {
                machine: Some([10.0, 20.0, 0.0]),
                work: None,
            }
        );
        // A WCO in the report itself wins over the remembered one.
        let own = report("<Idle|MPos:1.000,1.000,1.000|WCO:1.000,1.000,1.000>");
        assert_eq!(own.positions(Some(wco)).work, Some([0.0, 0.0, 0.0]));
    }
}
