//! GRBL `$$` settings: `$x=val` lines, completed by `ok`.
//!
//! Message format: GRBL v1.1h `doc/markdown/interface.md`, "Grbl `$`
//! Settings Message". Names, units and descriptions: GRBL v1.1h
//! `doc/csv/setting_codes_en_US.csv`. Meanings: `doc/markdown/settings.md`.

use std::collections::BTreeMap;

use super::ParseError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingInfo {
    pub name: &'static str,
    pub units: &'static str,
    pub description: &'static str,
}

/// How a setting's value is written, from the CSV's units column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Decimal,
    Boolean,
    Mask,
}

const SETTINGS: [(u16, &str, &str, &str); 34] = [
    (
        0,
        "Step pulse time",
        "microseconds",
        "Sets time length per step. Minimum 3usec.",
    ),
    (
        1,
        "Step idle delay",
        "milliseconds",
        "Sets a short hold delay when stopping to let dynamics settle before disabling steppers. Value 255 keeps motors enabled with no delay.",
    ),
    (
        2,
        "Step pulse invert",
        "mask",
        "Inverts the step signal. Set axis bit to invert (00000ZYX).",
    ),
    (
        3,
        "Step direction invert",
        "mask",
        "Inverts the direction signal. Set axis bit to invert (00000ZYX).",
    ),
    (
        4,
        "Invert step enable pin",
        "boolean",
        "Inverts the stepper driver enable pin signal.",
    ),
    (
        5,
        "Invert limit pins",
        "boolean",
        "Inverts the all of the limit input pins.",
    ),
    (
        6,
        "Invert probe pin",
        "boolean",
        "Inverts the probe input pin signal.",
    ),
    (
        10,
        "Status report options",
        "mask",
        "Alters data included in status reports.",
    ),
    (
        11,
        "Junction deviation",
        "millimeters",
        "Sets how fast Grbl travels through consecutive motions. Lower value slows it down.",
    ),
    (
        12,
        "Arc tolerance",
        "millimeters",
        "Sets the G2 and G3 arc tracing accuracy based on radial error. Beware: A very small value may effect performance.",
    ),
    (
        13,
        "Report in inches",
        "boolean",
        "Enables inch units when returning any position and rate value that is not a settings value.",
    ),
    (
        20,
        "Soft limits enable",
        "boolean",
        "Enables soft limits checks within machine travel and sets alarm when exceeded. Requires homing.",
    ),
    (
        21,
        "Hard limits enable",
        "boolean",
        "Enables hard limits. Immediately halts motion and throws an alarm when switch is triggered.",
    ),
    (
        22,
        "Homing cycle enable",
        "boolean",
        "Enables homing cycle. Requires limit switches on all axes.",
    ),
    (
        23,
        "Homing direction invert",
        "mask",
        "Homing searches for a switch in the positive direction. Set axis bit (00000ZYX) to search in negative direction.",
    ),
    (
        24,
        "Homing locate feed rate",
        "mm/min",
        "Feed rate to slowly engage limit switch to determine its location accurately.",
    ),
    (
        25,
        "Homing search seek rate",
        "mm/min",
        "Seek rate to quickly find the limit switch before the slower locating phase.",
    ),
    (
        26,
        "Homing switch debounce delay",
        "milliseconds",
        "Sets a short delay between phases of homing cycle to let a switch debounce.",
    ),
    (
        27,
        "Homing switch pull-off distance",
        "millimeters",
        "Retract distance after triggering switch to disengage it. Homing will fail if switch isn't cleared.",
    ),
    (
        30,
        "Maximum spindle speed",
        "RPM",
        "Maximum spindle speed. Sets PWM to 100% duty cycle.",
    ),
    (
        31,
        "Minimum spindle speed",
        "RPM",
        "Minimum spindle speed. Sets PWM to 0.4% or lowest duty cycle.",
    ),
    (
        32,
        "Laser-mode enable",
        "boolean",
        "Enables laser mode. Consecutive G1/2/3 commands will not halt when spindle speed is changed.",
    ),
    (
        100,
        "X-axis travel resolution",
        "step/mm",
        "X-axis travel resolution in steps per millimeter.",
    ),
    (
        101,
        "Y-axis travel resolution",
        "step/mm",
        "Y-axis travel resolution in steps per millimeter.",
    ),
    (
        102,
        "Z-axis travel resolution",
        "step/mm",
        "Z-axis travel resolution in steps per millimeter.",
    ),
    (
        110,
        "X-axis maximum rate",
        "mm/min",
        "X-axis maximum rate. Used as G0 rapid rate.",
    ),
    (
        111,
        "Y-axis maximum rate",
        "mm/min",
        "Y-axis maximum rate. Used as G0 rapid rate.",
    ),
    (
        112,
        "Z-axis maximum rate",
        "mm/min",
        "Z-axis maximum rate. Used as G0 rapid rate.",
    ),
    (
        120,
        "X-axis acceleration",
        "mm/sec^2",
        "X-axis acceleration. Used for motion planning to not exceed motor torque and lose steps.",
    ),
    (
        121,
        "Y-axis acceleration",
        "mm/sec^2",
        "Y-axis acceleration. Used for motion planning to not exceed motor torque and lose steps.",
    ),
    (
        122,
        "Z-axis acceleration",
        "mm/sec^2",
        "Z-axis acceleration. Used for motion planning to not exceed motor torque and lose steps.",
    ),
    (
        130,
        "X-axis maximum travel",
        "millimeters",
        "Maximum X-axis travel distance from homing switch. Determines valid machine space for soft-limits and homing search distances.",
    ),
    (
        131,
        "Y-axis maximum travel",
        "millimeters",
        "Maximum Y-axis travel distance from homing switch. Determines valid machine space for soft-limits and homing search distances.",
    ),
    (
        132,
        "Z-axis maximum travel",
        "millimeters",
        "Maximum Z-axis travel distance from homing switch. Determines valid machine space for soft-limits and homing search distances.",
    ),
];

/// Name, units and description of a GRBL v1.1 setting, or `None` for a
/// number GRBL does not define (forks add their own).
pub fn setting_info(number: u16) -> Option<SettingInfo> {
    SETTINGS
        .iter()
        .find(|s| s.0 == number)
        .map(|&(_, name, units, description)| SettingInfo {
            name,
            units,
            description,
        })
}

fn kind(number: u16) -> Option<Kind> {
    let info = setting_info(number)?;
    Some(match info.units {
        "boolean" => Kind::Boolean,
        "mask" => Kind::Mask,
        _ => Kind::Decimal,
    })
}

/// A setting's value, typed when GRBL v1.1 defines the setting.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    Decimal(f64),
    Boolean(bool),
    Mask(u8),
    /// A setting GRBL v1.1 does not define, kept as sent.
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    /// The value exactly as GRBL printed it.
    pub raw: String,
    pub value: SettingValue,
}

/// `$10`. See `settings.md`, "$10 - Status report, mask".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusReportMask {
    /// Value 1: reports carry `MPos:`, otherwise `WPos:`.
    pub machine_position: bool,
    /// Value 2: reports carry the `Bf:` buffer field.
    pub buffer_data: bool,
}

/// Settings from a `$$` query, by number.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GrblSettings {
    entries: BTreeMap<u16, Setting>,
}

impl GrblSettings {
    /// Adds one `$number=raw` setting. A value that does not fit the
    /// setting's type is rejected; the other settings are unaffected.
    pub fn insert(&mut self, number: u16, raw: &str) -> Result<(), ParseError> {
        let bad = |expected| ParseError::BadSetting {
            number,
            value: raw.to_owned(),
            expected,
        };
        let value = match kind(number) {
            None => SettingValue::Other(raw.to_owned()),
            Some(Kind::Decimal) => match raw.parse::<f64>() {
                Ok(v) if v.is_finite() => SettingValue::Decimal(v),
                _ => return Err(bad("number")),
            },
            Some(Kind::Boolean) => match raw {
                "0" => SettingValue::Boolean(false),
                "1" => SettingValue::Boolean(true),
                _ => return Err(bad("boolean (0 or 1)")),
            },
            Some(Kind::Mask) => SettingValue::Mask(raw.parse().map_err(|_| bad("mask (0-255)"))?),
        };
        self.entries.insert(
            number,
            Setting {
                raw: raw.to_owned(),
                value,
            },
        );
        Ok(())
    }

    pub fn get(&self, number: u16) -> Option<&Setting> {
        self.entries.get(&number)
    }

    /// In setting-number order.
    pub fn iter(&self) -> impl Iterator<Item = (u16, &Setting)> {
        self.entries.iter().map(|(n, s)| (*n, s))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn decimal(&self, number: u16) -> Option<f64> {
        match self.get(number)?.value {
            SettingValue::Decimal(v) => Some(v),
            _ => None,
        }
    }

    fn boolean(&self, number: u16) -> Option<bool> {
        match self.get(number)?.value {
            SettingValue::Boolean(v) => Some(v),
            _ => None,
        }
    }

    fn axes(&self, first: u16) -> Option<[f64; 3]> {
        Some([
            self.decimal(first)?,
            self.decimal(first + 1)?,
            self.decimal(first + 2)?,
        ])
    }

    pub fn status_report_mask(&self) -> Option<StatusReportMask> {
        match self.get(10)?.value {
            SettingValue::Mask(m) => Some(StatusReportMask {
                machine_position: m & 1 != 0,
                buffer_data: m & 2 != 0,
            }),
            _ => None,
        }
    }

    /// `$13`: positions and rates in status reports are in inches.
    pub fn report_inches(&self) -> Option<bool> {
        self.boolean(13)
    }

    pub fn soft_limits(&self) -> Option<bool> {
        self.boolean(20)
    }

    pub fn hard_limits(&self) -> Option<bool> {
        self.boolean(21)
    }

    pub fn homing(&self) -> Option<bool> {
        self.boolean(22)
    }

    /// `$30`: the S value for 100 % PWM (laser power).
    pub fn max_spindle_speed(&self) -> Option<f64> {
        self.decimal(30)
    }

    /// `$31`.
    pub fn min_spindle_speed(&self) -> Option<f64> {
        self.decimal(31)
    }

    /// `$32`.
    pub fn laser_mode(&self) -> Option<bool> {
        self.boolean(32)
    }

    /// `$100`–`$102`, steps/mm.
    pub fn steps_per_mm(&self) -> Option<[f64; 3]> {
        self.axes(100)
    }

    /// `$110`–`$112`, mm/min.
    pub fn max_rate(&self) -> Option<[f64; 3]> {
        self.axes(110)
    }

    /// `$120`–`$122`, mm/s².
    pub fn acceleration(&self) -> Option<[f64; 3]> {
        self.axes(120)
    }

    /// `$130`–`$132`, mm.
    pub fn max_travel(&self) -> Option<[f64; 3]> {
        self.axes(130)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::parse::{Response, parse_line};

    /// A `$$` dump with GRBL v1.1h's compiled-in defaults (`defaults.h`),
    /// laser mode on.
    const DUMP: &str = "$0=10\r\n$1=25\r\n$2=0\r\n$3=0\r\n$4=0\r\n$5=0\r\n$6=0\r\n$10=1\r\n\
        $11=0.010\r\n$12=0.002\r\n$13=0\r\n$20=0\r\n$21=0\r\n$22=0\r\n$23=0\r\n$24=25.000\r\n\
        $25=500.000\r\n$26=250\r\n$27=1.000\r\n$30=1000\r\n$31=0\r\n$32=1\r\n$100=250.000\r\n\
        $101=250.000\r\n$102=250.000\r\n$110=500.000\r\n$111=500.000\r\n$112=500.000\r\n\
        $120=10.000\r\n$121=10.000\r\n$122=10.000\r\n$130=200.000\r\n$131=200.000\r\n\
        $132=200.000\r\nok\r\n";

    /// Feeds every `$x=val` line of `text` through `parse_line`, as the
    /// connection does.
    fn collect(text: &str) -> (GrblSettings, Vec<ParseError>) {
        let mut settings = GrblSettings::default();
        let mut errors = Vec::new();
        for line in text.lines() {
            if let Response::Setting { number, value } = parse_line(line)
                && let Err(err) = settings.insert(number, &value)
            {
                errors.push(err);
            }
        }
        (settings, errors)
    }

    #[test]
    fn parse_a_full_dump() {
        let (s, errors) = collect(DUMP);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(s.len(), 34);
        assert_eq!(
            s.status_report_mask(),
            Some(StatusReportMask {
                machine_position: true,
                buffer_data: false
            })
        );
        assert_eq!(s.report_inches(), Some(false));
        assert_eq!(s.soft_limits(), Some(false));
        assert_eq!(s.hard_limits(), Some(false));
        assert_eq!(s.homing(), Some(false));
        assert_eq!(s.max_spindle_speed(), Some(1000.0));
        assert_eq!(s.min_spindle_speed(), Some(0.0));
        assert_eq!(s.laser_mode(), Some(true));
        assert_eq!(s.steps_per_mm(), Some([250.0; 3]));
        assert_eq!(s.max_rate(), Some([500.0; 3]));
        assert_eq!(s.acceleration(), Some([10.0; 3]));
        assert_eq!(s.max_travel(), Some([200.0; 3]));
        // The raw text is kept for display.
        assert_eq!(s.get(11).unwrap().raw, "0.010");
        let numbers: Vec<u16> = s.iter().map(|(n, _)| n).collect();
        assert_eq!(numbers.first(), Some(&0));
        assert_eq!(numbers.last(), Some(&132));
    }

    #[test]
    fn parse_bad_values_reject_only_that_setting() {
        let (s, errors) = collect("$32=2\n$30=abc\n$10=300\n$110=inf\n$31=0\n");
        assert_eq!(
            errors,
            vec![
                ParseError::BadSetting {
                    number: 32,
                    value: "2".into(),
                    expected: "boolean (0 or 1)"
                },
                ParseError::BadSetting {
                    number: 30,
                    value: "abc".into(),
                    expected: "number"
                },
                ParseError::BadSetting {
                    number: 10,
                    value: "300".into(),
                    expected: "mask (0-255)"
                },
                ParseError::BadSetting {
                    number: 110,
                    value: "inf".into(),
                    expected: "number"
                },
            ]
        );
        assert_eq!(s.len(), 1);
        assert_eq!(s.min_spindle_speed(), Some(0.0));
        assert_eq!(s.laser_mode(), None);
    }

    #[test]
    fn parse_unknown_settings_are_kept_raw() {
        // Forks add settings such as grblHAL's $33 (PWM frequency).
        let (s, errors) = collect("$33=5000.0\n$341=text value\n");
        assert!(errors.is_empty());
        assert_eq!(
            s.get(33).unwrap().value,
            SettingValue::Other("5000.0".into())
        );
        assert_eq!(s.get(341).unwrap().raw, "text value");
        assert_eq!(setting_info(33), None);
    }

    #[test]
    fn parse_partial_axes_give_no_vector() {
        let (s, _) = collect("$130=300\n$131=200\n");
        assert_eq!(s.max_travel(), None);
    }

    #[test]
    fn parse_work_position_mask() {
        let (s, _) = collect("$10=2\n");
        assert_eq!(
            s.status_report_mask(),
            Some(StatusReportMask {
                machine_position: false,
                buffer_data: true
            })
        );
    }

    #[test]
    fn parse_setting_info_comes_from_the_csv() {
        let info = setting_info(32).unwrap();
        assert_eq!(info.name, "Laser-mode enable");
        assert_eq!(info.units, "boolean");
        assert_eq!(setting_info(130).unwrap().units, "millimeters");
        assert_eq!(setting_info(7), None);
    }
}
