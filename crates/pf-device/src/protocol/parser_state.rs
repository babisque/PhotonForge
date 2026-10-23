//! `$G` parser state: `[GC:G0 G54 G17 G21 G90 G94 M5 M9 T0 F0 S0]`.
//!
//! Format: GRBL v1.1h `doc/markdown/interface.md`, "Queried Feedback
//! Messages", `[GC:]`. These are the parser's modal states, which may run
//! ahead of what the machine is executing.

use super::ParseError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Units {
    Inches,      // G20
    Millimeters, // G21
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistanceMode {
    Absolute,    // G90
    Incremental, // G91
}

/// For a laser, `M3`/`M4` mean the laser output is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpindleState {
    Clockwise,        // M3
    CounterClockwise, // M4
    Off,              // M5
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParserState {
    /// Every word, as sent.
    pub words: Vec<String>,
    /// `G0`, `G1`, `G2`, `G3`, `G38.2`–`G38.5` or `G80`.
    pub motion: Option<String>,
    /// `G54`–`G59`.
    pub coordinate_system: Option<String>,
    pub units: Option<Units>,
    pub distance: Option<DistanceMode>,
    pub spindle: Option<SpindleState>,
    pub tool: Option<u32>,
    pub feed: Option<f64>,          // in the parser's units per minute
    pub spindle_speed: Option<f64>, // S value
}

pub fn parse_parser_state(data: &str) -> Result<ParserState, ParseError> {
    let mut state = ParserState {
        words: data.split_whitespace().map(str::to_owned).collect(),
        motion: None,
        coordinate_system: None,
        units: None,
        distance: None,
        spindle: None,
        tool: None,
        feed: None,
        spindle_speed: None,
    };
    for word in data.split_whitespace() {
        match word {
            "G0" | "G1" | "G2" | "G3" | "G38.2" | "G38.3" | "G38.4" | "G38.5" | "G80" => {
                state.motion = Some(word.to_owned());
            }
            "G54" | "G55" | "G56" | "G57" | "G58" | "G59" => {
                state.coordinate_system = Some(word.to_owned());
            }
            "G20" => state.units = Some(Units::Inches),
            "G21" => state.units = Some(Units::Millimeters),
            "G90" => state.distance = Some(DistanceMode::Absolute),
            "G91" => state.distance = Some(DistanceMode::Incremental),
            "M3" => state.spindle = Some(SpindleState::Clockwise),
            "M4" => state.spindle = Some(SpindleState::CounterClockwise),
            "M5" => state.spindle = Some(SpindleState::Off),
            _ => {
                if let Some(value) = word.strip_prefix('T') {
                    state.tool = Some(value.parse().map_err(|_| bad(word))?);
                } else if let Some(value) = word.strip_prefix('F') {
                    state.feed = Some(finite(value).ok_or_else(|| bad(word))?);
                } else if let Some(value) = word.strip_prefix('S') {
                    state.spindle_speed = Some(finite(value).ok_or_else(|| bad(word))?);
                }
                // Other modal words (plane, feed mode, coolant, program
                // state, tool length) stay in `words` only.
            }
        }
    }
    Ok(state)
}

fn finite(value: &str) -> Option<f64> {
    value.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn bad(word: &str) -> ParseError {
    ParseError::BadValue {
        field: "GC",
        value: word.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_the_documented_example() {
        let s = parse_parser_state("G0 G54 G17 G21 G90 G94 M5 M9 T0 F0.0 S0").unwrap();
        assert_eq!(s.words.len(), 11);
        assert_eq!(s.motion.as_deref(), Some("G0"));
        assert_eq!(s.coordinate_system.as_deref(), Some("G54"));
        assert_eq!(s.units, Some(Units::Millimeters));
        assert_eq!(s.distance, Some(DistanceMode::Absolute));
        assert_eq!(s.spindle, Some(SpindleState::Off));
        assert_eq!(s.tool, Some(0));
        assert_eq!(s.feed, Some(0.0));
        assert_eq!(s.spindle_speed, Some(0.0));
    }

    #[test]
    fn parse_laser_running_in_inches() {
        let s = parse_parser_state("G1 G55 G17 G20 G91 G94 M4 M9 T1 F1500 S800").unwrap();
        assert_eq!(s.motion.as_deref(), Some("G1"));
        assert_eq!(s.units, Some(Units::Inches));
        assert_eq!(s.distance, Some(DistanceMode::Incremental));
        assert_eq!(s.spindle, Some(SpindleState::CounterClockwise));
        assert_eq!((s.feed, s.spindle_speed), (Some(1500.0), Some(800.0)));
        // Program state words appear only while active (e.g. M0).
        let paused =
            parse_parser_state("G38.2 G54 G17 G21 G90 G94 M0 M3 M9 T0 F100 S1000").unwrap();
        assert_eq!(paused.motion.as_deref(), Some("G38.2"));
        assert_eq!(paused.spindle, Some(SpindleState::Clockwise));
        assert!(paused.words.contains(&"M0".to_owned()));
    }

    #[test]
    fn parse_bad_numbers_are_errors() {
        for data in ["G0 Fabc", "G0 S", "G0 T-1", "G0 Finf"] {
            assert!(
                matches!(
                    parse_parser_state(data),
                    Err(ParseError::BadValue { field: "GC", .. })
                ),
                "{data:?}"
            );
        }
        assert_eq!(parse_parser_state("").unwrap().words.len(), 0);
    }
}
