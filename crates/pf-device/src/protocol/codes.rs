//! Meanings of GRBL `error:N` and `ALARM:N` codes.
//!
//! Source: GRBL v1.1h (tag `v1.1h.20190825`), `doc/csv/error_codes_en_US.csv`
//! and `doc/csv/alarm_codes_en_US.csv`, which `doc/markdown/interface.md`
//! names as the maintained reference. GRBL is GPL-3.0, like PhotonForge.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeInfo {
    /// The short message GRBL v1.0 and earlier printed.
    pub name: &'static str,
    pub description: &'static str,
}

/// Codes 18 and 19 are unassigned in GRBL v1.1.
const ERRORS: [(u8, &str, &str); 36] = [
    (
        1,
        "Expected command letter",
        "G-code words consist of a letter and a value. Letter was not found.",
    ),
    (
        2,
        "Bad number format",
        "Missing the expected G-code word value or numeric value format is not valid.",
    ),
    (
        3,
        "Invalid statement",
        "Grbl '$' system command was not recognized or supported.",
    ),
    (
        4,
        "Value < 0",
        "Negative value received for an expected positive value.",
    ),
    (
        5,
        "Setting disabled",
        "Homing cycle failure. Homing is not enabled via settings.",
    ),
    (
        6,
        "Value < 3 usec",
        "Minimum step pulse time must be greater than 3usec.",
    ),
    (
        7,
        "EEPROM read fail. Using defaults",
        "An EEPROM read failed. Auto-restoring affected EEPROM to default values.",
    ),
    (
        8,
        "Not idle",
        "Grbl '$' command cannot be used unless Grbl is IDLE. Ensures smooth operation during a job.",
    ),
    (
        9,
        "G-code lock",
        "G-code commands are locked out during alarm or jog state.",
    ),
    (
        10,
        "Homing not enabled",
        "Soft limits cannot be enabled without homing also enabled.",
    ),
    (
        11,
        "Line overflow",
        "Max characters per line exceeded. Received command line was not executed.",
    ),
    (
        12,
        "Step rate > 30kHz",
        "Grbl '$' setting value cause the step rate to exceed the maximum supported.",
    ),
    (
        13,
        "Check Door",
        "Safety door detected as opened and door state initiated.",
    ),
    (
        14,
        "Line length exceeded",
        "Build info or startup line exceeded EEPROM line length limit. Line not stored.",
    ),
    (
        15,
        "Travel exceeded",
        "Jog target exceeds machine travel. Jog command has been ignored.",
    ),
    (
        16,
        "Invalid jog command",
        "Jog command has no '=' or contains prohibited g-code.",
    ),
    (17, "Setting disabled", "Laser mode requires PWM output."),
    (
        20,
        "Unsupported command",
        "Unsupported or invalid g-code command found in block.",
    ),
    (
        21,
        "Modal group violation",
        "More than one g-code command from same modal group found in block.",
    ),
    (
        22,
        "Undefined feed rate",
        "Feed rate has not yet been set or is undefined.",
    ),
    (
        23,
        "Invalid gcode ID:23",
        "G-code command in block requires an integer value.",
    ),
    (
        24,
        "Invalid gcode ID:24",
        "More than one g-code command that requires axis words found in block.",
    ),
    (
        25,
        "Invalid gcode ID:25",
        "Repeated g-code word found in block.",
    ),
    (
        26,
        "Invalid gcode ID:26",
        "No axis words found in block for g-code command or current modal state which requires them.",
    ),
    (27, "Invalid gcode ID:27", "Line number value is invalid."),
    (
        28,
        "Invalid gcode ID:28",
        "G-code command is missing a required value word.",
    ),
    (
        29,
        "Invalid gcode ID:29",
        "G59.x work coordinate systems are not supported.",
    ),
    (
        30,
        "Invalid gcode ID:30",
        "G53 only allowed with G0 and G1 motion modes.",
    ),
    (
        31,
        "Invalid gcode ID:31",
        "Axis words found in block when no command or current modal state uses them.",
    ),
    (
        32,
        "Invalid gcode ID:32",
        "G2 and G3 arcs require at least one in-plane axis word.",
    ),
    (
        33,
        "Invalid gcode ID:33",
        "Motion command target is invalid.",
    ),
    (34, "Invalid gcode ID:34", "Arc radius value is invalid."),
    (
        35,
        "Invalid gcode ID:35",
        "G2 and G3 arcs require at least one in-plane offset word.",
    ),
    (
        36,
        "Invalid gcode ID:36",
        "Unused value words found in block.",
    ),
    (
        37,
        "Invalid gcode ID:37",
        "G43.1 dynamic tool length offset is not assigned to configured tool length axis.",
    ),
    (
        38,
        "Invalid gcode ID:38",
        "Tool number greater than max supported value.",
    ),
];

const ALARMS: [(u8, &str, &str); 10] = [
    (
        1,
        "Hard limit",
        "Hard limit has been triggered. Machine position is likely lost due to sudden halt. Re-homing is highly recommended.",
    ),
    (
        2,
        "Soft limit",
        "Soft limit alarm. G-code motion target exceeds machine travel. Machine position retained. Alarm may be safely unlocked.",
    ),
    (
        3,
        "Abort during cycle",
        "Reset while in motion. Machine position is likely lost due to sudden halt. Re-homing is highly recommended.",
    ),
    (
        4,
        "Probe fail",
        "Probe fail. Probe is not in the expected initial state before starting probe cycle when G38.2 and G38.3 is not triggered and G38.4 and G38.5 is triggered.",
    ),
    (
        5,
        "Probe fail",
        "Probe fail. Probe did not contact the workpiece within the programmed travel for G38.2 and G38.4.",
    ),
    (
        6,
        "Homing fail",
        "Homing fail. The active homing cycle was reset.",
    ),
    (
        7,
        "Homing fail",
        "Homing fail. Safety door was opened during homing cycle.",
    ),
    (
        8,
        "Homing fail",
        "Homing fail. Pull off travel failed to clear limit switch. Try increasing pull-off setting or check wiring.",
    ),
    (
        9,
        "Homing fail",
        "Homing fail. Could not find limit switch within search distances. Try increasing max travel, decreasing pull-off distance, or check wiring.",
    ),
    (
        10,
        "Homing fail",
        "Homing fail. Second dual axis limit switch failed to trigger within configured search distance after first. Try increasing trigger fail distance or check wiring.",
    ),
];

/// Meaning of `error:code`, or `None` for a code GRBL v1.1 does not define.
pub fn error_info(code: u8) -> Option<CodeInfo> {
    lookup(&ERRORS, code)
}

/// Meaning of `ALARM:code`, or `None` for a code GRBL v1.1 does not define.
pub fn alarm_info(code: u8) -> Option<CodeInfo> {
    lookup(&ALARMS, code)
}

fn lookup(table: &[(u8, &'static str, &'static str)], code: u8) -> Option<CodeInfo> {
    table
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|&(_, name, description)| CodeInfo { name, description })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tables_cover_the_documented_codes() {
        let errors: Vec<u8> = ERRORS.iter().map(|e| e.0).collect();
        let expected: Vec<u8> = (1..=17).chain(20..=38).collect();
        assert_eq!(errors, expected);
        let alarms: Vec<u8> = ALARMS.iter().map(|a| a.0).collect();
        assert_eq!(alarms, (1..=10).collect::<Vec<u8>>());
    }

    #[test]
    fn parse_lookups_return_the_csv_text() {
        assert_eq!(
            error_info(8),
            Some(CodeInfo {
                name: "Not idle",
                description: "Grbl '$' command cannot be used unless Grbl is IDLE. \
                              Ensures smooth operation during a job.",
            })
        );
        assert_eq!(alarm_info(2).map(|a| a.name), Some("Soft limit"));
        assert_eq!(error_info(18), None);
        assert_eq!(error_info(0), None);
        assert_eq!(alarm_info(11), None);
    }
}
