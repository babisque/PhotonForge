//! `$I` build info: `[VER:…]` then `[OPT:…]`, completed by `ok`.
//!
//! Format: GRBL v1.1h `doc/markdown/interface.md`, "Feedback Messages",
//! `[VER:]` and `[OPT:]`. Option codes: `doc/csv/build_option_codes_en_US.csv`.

use super::ParseError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// E.g. `1.1h`.
    pub version: String,
    /// `YYYYMMDD`, when present.
    pub build_date: Option<String>,
    /// Text stored with `$I=…` by the user or the machine maker.
    pub user_text: String,
}

/// Everything a `$I` query reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: Version,
    pub options: Option<BuildOptions>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOptions {
    /// Option codes, see `option_info`.
    pub codes: String,
    pub planner_blocks: u16,
    /// Serial receive buffer size. Streaming must never overfill it.
    pub rx_buffer_bytes: u16,
}

impl BuildOptions {
    pub fn has(&self, code: char) -> bool {
        self.codes.contains(code)
    }
}

/// Parses the data of `[VER:…]`, e.g. `1.1h.20190825:` or
/// `v1.1f.20170131:Some string`.
pub fn parse_version(data: &str) -> Result<Version, ParseError> {
    let (build, user_text) = data.split_once(':').unwrap_or((data, ""));
    let build = build.strip_prefix('v').unwrap_or(build);
    let (version, build_date) = match build.rsplit_once('.') {
        Some((version, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => {
            (version, Some(date.to_owned()))
        }
        _ => (build, None),
    };
    if !version.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(ParseError::BadValue {
            field: "VER",
            value: data.to_owned(),
        });
    }
    Ok(Version {
        version: version.to_owned(),
        build_date,
        user_text: user_text.to_owned(),
    })
}

/// Parses the data of `[OPT:…]`, e.g. `V,15,128`.
pub fn parse_options(data: &str) -> Result<BuildOptions, ParseError> {
    let bad = || ParseError::BadValue {
        field: "OPT",
        value: data.to_owned(),
    };
    // Codes come first; the two numbers are always last.
    let mut parts = data.rsplitn(3, ',');
    let rx = parts.next().ok_or_else(bad)?;
    let blocks = parts.next().ok_or_else(bad)?;
    let codes = parts.next().ok_or_else(bad)?;
    Ok(BuildOptions {
        codes: codes.to_owned(),
        planner_blocks: blocks.parse().map_err(|_| bad())?,
        rx_buffer_bytes: rx.parse().map_err(|_| bad())?,
    })
}

/// Whether the controller speaks the GRBL v1.1 protocol this crate parses.
/// Accepts the welcome or `[VER:]` version, e.g. `1.1h`.
pub fn is_supported_version(version: &str) -> bool {
    version.starts_with("1.1")
}

const OPTIONS: [(char, &str); 22] = [
    ('V', "Variable spindle enabled"),
    ('N', "Line numbers enabled"),
    ('M', "Mist coolant M7 enabled"),
    ('C', "CoreXY enabled"),
    ('P', "Parking motion enabled"),
    ('Z', "Homing force origin enabled"),
    ('H', "Homing single axis commands enabled"),
    ('T', "Two limit switches on axis enabled"),
    ('A', "Allow feed rate overrides in probe cycles enabled"),
    ('D', "Use spindle direction as enable pin enabled"),
    ('0', "Spindle enable off when speed is zero enabled"),
    ('S', "Software limit pin debouncing enabled"),
    ('R', "Parking override control enabled"),
    ('+', "Safety door input pin enabled"),
    ('*', "Restore all EEPROM command disabled"),
    ('$', "Restore EEPROM `$` settings command disabled"),
    ('#', "Restore EEPROM parameter data command disabled"),
    ('I', "Build info write user string command disabled"),
    ('E', "Force sync upon EEPROM write disabled"),
    (
        'W',
        "Force sync upon work coordinate offset change disabled",
    ),
    ('L', "Homing initialization auto-lock disabled"),
    ('2', "Dual axis motors enabled"),
];

/// What an `[OPT:]` code means, or `None` for codes GRBL v1.1 does not define.
pub fn option_info(code: char) -> Option<&'static str> {
    OPTIONS.iter().find(|o| o.0 == code).map(|o| o.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_versions() {
        assert_eq!(
            parse_version("1.1h.20190825:"),
            Ok(Version {
                version: "1.1h".into(),
                build_date: Some("20190825".into()),
                user_text: String::new(),
            })
        );
        assert_eq!(
            parse_version("v1.1f.20170131:Some string"),
            Ok(Version {
                version: "1.1f".into(),
                build_date: Some("20170131".into()),
                user_text: "Some string".into(),
            })
        );
        // The user text may itself contain colons.
        assert_eq!(
            parse_version("1.1h.20190825:Laser: 40W").unwrap().user_text,
            "Laser: 40W"
        );
        // No build date.
        assert_eq!(
            parse_version("1.1h"),
            Ok(Version {
                version: "1.1h".into(),
                build_date: None,
                user_text: String::new(),
            })
        );
        assert!(parse_version("abc:").is_err());
        assert!(parse_version("").is_err());
    }

    #[test]
    fn parse_options_line() {
        let opt = parse_options("VL,15,128").unwrap();
        assert_eq!(
            opt,
            BuildOptions {
                codes: "VL".into(),
                planner_blocks: 15,
                rx_buffer_bytes: 128,
            }
        );
        assert!(opt.has('V') && opt.has('L') && !opt.has('C'));
        // No option codes at all.
        assert_eq!(parse_options(",15,128").unwrap().codes, "");
        // `,` is not an option code, but `+`, `*`, `$` and `#` are.
        assert_eq!(parse_options("V+*$#,16,256").unwrap().codes, "V+*$#");
        for bad in ["", "V", "V,15", "V,x,128", "V,15,-1"] {
            assert!(parse_options(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn parse_supported_versions() {
        assert!(is_supported_version("1.1h"));
        assert!(is_supported_version("1.1f"));
        assert!(!is_supported_version("0.9j"));
        assert!(!is_supported_version("1.0c"));
        assert!(!is_supported_version(""));
    }

    #[test]
    fn parse_option_info_from_the_csv() {
        assert_eq!(option_info('V'), Some("Variable spindle enabled"));
        assert_eq!(option_info('2'), Some("Dual axis motors enabled"));
        assert_eq!(option_info('?'), None);
    }
}
