//! Differences between the controller's `$$` settings and PhotonForge's
//! machine profile. Hints only: nothing is changed on either side.

use pf_core::MachineProfile;

use super::settings::{GrblSettings, SettingValue};

/// Work-area differences below this (mm) are not reported.
const TRAVEL_TOLERANCE_MM: f64 = 0.5;

#[derive(Debug, Clone, PartialEq)]
pub enum ProfileHint {
    /// `$30` differs from the profile's max S, so power percentages would
    /// map to different laser output than intended.
    MaxSpindleMismatch { grbl: f64, profile: u32 },
    /// `$130`/`$131` differ from the profile's work area.
    TravelMismatch {
        axis: char,
        grbl_mm: f64,
        profile_mm: f64,
    },
    /// `$32=0`: GRBL pauses at every power change, so cuts burn unevenly.
    LaserModeOff,
    /// `$13=1`: status positions and rates arrive in inches.
    ReportsInches,
}

impl std::fmt::Display for ProfileHint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileHint::MaxSpindleMismatch { grbl, profile } => write!(
                f,
                "GRBL max S ($30) is {grbl}, but the machine profile uses {profile}."
            ),
            ProfileHint::TravelMismatch {
                axis,
                grbl_mm,
                profile_mm,
            } => write!(
                f,
                "GRBL {axis} travel is {grbl_mm} mm, but the work area is {profile_mm} mm."
            ),
            ProfileHint::LaserModeOff => write!(
                f,
                "Laser mode ($32) is off: GRBL stops at every power change."
            ),
            ProfileHint::ReportsInches => write!(
                f,
                "GRBL reports in inches ($13=1); PhotonForge works in millimeters."
            ),
        }
    }
}

pub fn compare_with_profile(settings: &GrblSettings, profile: &MachineProfile) -> Vec<ProfileHint> {
    let mut hints = Vec::new();
    if let Some(grbl) = settings.max_spindle_speed()
        && grbl != f64::from(profile.max_s_value())
    {
        hints.push(ProfileHint::MaxSpindleMismatch {
            grbl,
            profile: profile.max_s_value(),
        });
    }
    let area = profile.work_area();
    for (axis, number, profile_mm) in [('X', 130, area.width()), ('Y', 131, area.height())] {
        if let Some(setting) = settings.get(number)
            && let SettingValue::Decimal(grbl_mm) = setting.value
            && (grbl_mm - profile_mm).abs() > TRAVEL_TOLERANCE_MM
        {
            hints.push(ProfileHint::TravelMismatch {
                axis,
                grbl_mm,
                profile_mm,
            });
        }
    }
    if settings.laser_mode() == Some(false) {
        hints.push(ProfileHint::LaserModeOff);
    }
    if settings.report_inches() == Some(true) {
        hints.push(ProfileHint::ReportsInches);
    }
    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(u16, &str)]) -> GrblSettings {
        let mut s = GrblSettings::default();
        for (number, value) in pairs {
            s.insert(*number, value).unwrap();
        }
        s
    }

    #[test]
    fn parse_matching_settings_give_no_hints() {
        // The default profile: 300 × 200 mm, S1000.
        let s = settings(&[
            (30, "1000"),
            (130, "300.2"),
            (131, "200"),
            (32, "1"),
            (13, "0"),
        ]);
        assert!(compare_with_profile(&s, &MachineProfile::default()).is_empty());
        // Nothing known, nothing to say.
        assert!(
            compare_with_profile(&GrblSettings::default(), &MachineProfile::default()).is_empty()
        );
    }

    #[test]
    fn parse_differences_are_reported() {
        let s = settings(&[
            (30, "255"),
            (130, "400"),
            (131, "200"),
            (32, "0"),
            (13, "1"),
        ]);
        let hints = compare_with_profile(&s, &MachineProfile::default());
        assert_eq!(
            hints,
            vec![
                ProfileHint::MaxSpindleMismatch {
                    grbl: 255.0,
                    profile: 1000
                },
                ProfileHint::TravelMismatch {
                    axis: 'X',
                    grbl_mm: 400.0,
                    profile_mm: 300.0
                },
                ProfileHint::LaserModeOff,
                ProfileHint::ReportsInches,
            ]
        );
        assert_eq!(
            hints[1].to_string(),
            "GRBL X travel is 400 mm, but the work area is 300 mm."
        );
    }
}
