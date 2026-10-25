//! Checks that must all pass before a job or frame may start. There is no
//! way to override them.
//!
//! `scan_job` reads the exact lines that will be streamed, independently of
//! how they were generated, so a bug upstream cannot slip past the limits.

use pf_core::{MachineProfile, Point, Rect, power_to_s};
use thiserror::Error;

use crate::job::JobKind;
use crate::protocol::status::MachineState;
use crate::snapshot::{DeviceSnapshot, LinkState};

/// Smaller work offsets than this count as zero, in mm or inches.
const WCO_TOLERANCE: f64 = 0.001;

/// What a job does, as far as the checks need to know.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JobSummary {
    /// Every XY position the job moves to, in work coordinates (mm).
    /// `None` when nothing moves.
    pub bounds: Option<Rect>,
    /// The highest `S` word anywhere in the job.
    pub max_s: u32,
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum ScanError {
    #[error("line {line}: {word} is not supported in a job, so the job's limits cannot be checked")]
    Unsupported { line: usize, word: String },
    #[error("line {line}: {text:?} is a GRBL system command, which a job may not contain")]
    SystemCommand { line: usize, text: String },
    #[error("line {line}: cannot read {text:?}")]
    BadWord { line: usize, text: String },
    #[error("line {line}: the first move gives only one of X and Y, so where it goes is unknown")]
    IncompleteFirstMove { line: usize },
}

/// Reads G-code lines as GRBL would and records where they move and the
/// highest power. Anything that would make positions uncertain (relative
/// or inch moves, arcs, other coordinate systems, offsets, homing) is
/// refused rather than guessed at. Z moves are allowed and not checked.
pub fn scan_job(lines: &[String]) -> Result<JobSummary, ScanError> {
    let mut bounds: Option<Rect> = None;
    let mut max_s = 0;
    let (mut x, mut y) = (None, None);
    for (line, raw) in lines.iter().enumerate() {
        let code = strip_comments(raw);
        let compact: String = code
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_uppercase();
        if compact.is_empty() {
            continue;
        }
        if compact.starts_with('$') {
            return Err(ScanError::SystemCommand {
                line,
                text: raw.trim().to_owned(),
            });
        }
        let mut moved = false;
        for (letter, value) in words(&compact).map_err(|text| ScanError::BadWord { line, text })? {
            let unsupported = || ScanError::Unsupported {
                line,
                word: format!("{letter}{value}"),
            };
            match letter {
                'G' if is_one_of(value, &[0.0, 1.0, 4.0, 17.0, 21.0, 54.0, 90.0, 94.0]) => {}
                'M' if is_one_of(value, &[3.0, 4.0, 5.0, 7.0, 8.0, 9.0]) => {}
                'S' if value >= 0.0 => {
                    // In range: GRBL S values are far below u32::MAX.
                    max_s = max_s.max(value.ceil() as u32);
                }
                'X' => {
                    x = Some(value);
                    moved = true;
                }
                'Y' => {
                    y = Some(value);
                    moved = true;
                }
                'Z' | 'F' | 'P' | 'N' => {}
                _ => return Err(unsupported()),
            }
        }
        if moved {
            let (Some(x), Some(y)) = (x, y) else {
                return Err(ScanError::IncompleteFirstMove { line });
            };
            let point = Point::new(x, y);
            bounds = Some(bounds.map_or(Rect::new(point, point), |b| b.expand_to(point)));
        }
    }
    Ok(JobSummary { bounds, max_s })
}

fn strip_comments(line: &str) -> String {
    let code = line.split(';').next().unwrap_or("");
    let mut out = String::new();
    let mut depth = 0;
    for c in code.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Splits `G0X1.5Y-2` into letter/number pairs. Errors carry the bad text.
fn words(compact: &str) -> Result<Vec<(char, f64)>, String> {
    let mut out = Vec::new();
    let mut chars = compact.char_indices().peekable();
    while let Some((start, letter)) = chars.next() {
        if !letter.is_ascii_alphabetic() {
            return Err(compact[start..].to_owned());
        }
        let mut end = start + 1;
        while let Some(&(i, c)) = chars.peek() {
            if c.is_ascii_digit() || matches!(c, '.' | '-' | '+') {
                end = i + c.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let number = &compact[start + 1..end];
        match number.parse::<f64>() {
            Ok(value) if value.is_finite() => out.push((letter, value)),
            _ => return Err(compact[start..end].to_owned()),
        }
    }
    Ok(out)
}

fn is_one_of(value: f64, allowed: &[f64]) -> bool {
    allowed.contains(&value)
}

#[derive(Debug, Clone, PartialEq, Error)]
pub enum PreflightError {
    #[error("not connected to a controller")]
    NotConnected,
    #[error("the controller is not answering")]
    Unresponsive,
    #[error("no status report from the controller yet")]
    NoStatus,
    #[error("the machine is {0:?}; it must be Idle")]
    NotIdle(MachineState),
    #[error("an alarm is active (ALARM:{code:?}); unlock or home first")]
    Alarm { code: Option<u8> },
    #[error("the controller settings have not been read yet")]
    SettingsNotLoaded,
    #[error("laser mode ($32) is off or unknown; GRBL would pause at every power change")]
    LaserModeOff,
    #[error("max spindle speed ($30) is unknown")]
    MaxSpindleUnknown,
    #[error("waiting for the controller's override values")]
    OverridesUnknown,
    #[error("spindle override is {0}%; it must be at most 100% so power stays within limits")]
    SpindleOverride(u16),
    #[error("the work offset is unknown; waiting for the controller")]
    WorkOffsetUnknown,
    #[error(
        "a work offset of {0:?} is active; jobs need work coordinates equal to machine coordinates"
    )]
    WorkOffset([f64; 3]),
    #[error("the job does not move")]
    NothingToRun,
    #[error("the job reaches {bounds:?}, outside the work area {area:?}")]
    OutsideWorkArea { bounds: Rect, area: Rect },
    #[error(
        "the job reaches {bounds:?}, beyond the controller's max travel ($130/$131) of {travel:?} mm"
    )]
    OutsideTravel { bounds: Rect, travel: [f64; 2] },
    #[error("the job uses S{s}, above the controller's maximum ($30) of S{max}")]
    PowerAboveController { s: u32, max: u32 },
    #[error("the job uses S{s}, above the profile's {kind:?} limit of S{max}")]
    PowerAboveProfile { s: u32, max: u32, kind: JobKind },
}

/// Every reason the job may not start, or `Ok` when it may. All checks run,
/// so the user sees every problem at once.
pub fn preflight(
    snapshot: &DeviceSnapshot,
    job: &JobSummary,
    kind: JobKind,
    profile: &MachineProfile,
) -> Result<(), Vec<PreflightError>> {
    let mut errors = Vec::new();
    check_link(snapshot, &mut errors);
    check_settings(snapshot, job, &mut errors);
    check_job(job, kind, profile, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn check_link(snapshot: &DeviceSnapshot, errors: &mut Vec<PreflightError>) {
    if !matches!(snapshot.link, LinkState::Ready { .. }) {
        errors.push(PreflightError::NotConnected);
        return;
    }
    if snapshot.unresponsive {
        errors.push(PreflightError::Unresponsive);
    }
    let state = snapshot.status.as_ref().map(|s| s.state);
    match state {
        None => errors.push(PreflightError::NoStatus),
        // An alarm gets its own error below.
        Some(MachineState::Idle | MachineState::Alarm) => {}
        Some(other) => errors.push(PreflightError::NotIdle(other)),
    }
    if state == Some(MachineState::Alarm) || snapshot.last_alarm.is_some() {
        errors.push(PreflightError::Alarm {
            code: snapshot.last_alarm,
        });
    }
    match snapshot.overrides {
        None => errors.push(PreflightError::OverridesUnknown),
        Some(ov) if ov.spindle > 100 => errors.push(PreflightError::SpindleOverride(ov.spindle)),
        Some(_) => {}
    }
    match snapshot.positions.and_then(|p| p.machine.zip(p.work)) {
        None if state.is_some() => errors.push(PreflightError::WorkOffsetUnknown),
        None => {}
        Some((m, w)) => {
            let wco = [m[0] - w[0], m[1] - w[1], m[2] - w[2]];
            if wco.iter().any(|v| v.abs() > WCO_TOLERANCE) {
                errors.push(PreflightError::WorkOffset(wco));
            }
        }
    }
}

fn check_settings(snapshot: &DeviceSnapshot, job: &JobSummary, errors: &mut Vec<PreflightError>) {
    if !matches!(snapshot.link, LinkState::Ready { .. }) {
        return;
    }
    let Some(settings) = &snapshot.settings else {
        errors.push(PreflightError::SettingsNotLoaded);
        return;
    };
    if settings.laser_mode() != Some(true) {
        errors.push(PreflightError::LaserModeOff);
    }
    match settings.max_spindle_speed() {
        None => errors.push(PreflightError::MaxSpindleUnknown),
        Some(max) if f64::from(job.max_s) > max => {
            errors.push(PreflightError::PowerAboveController {
                s: job.max_s,
                // In range: $30 is a positive S value.
                max: max.floor() as u32,
            });
        }
        Some(_) => {}
    }
    if let (Some(bounds), Some(travel)) = (job.bounds, settings.max_travel()) {
        let machine = Rect::new(Point::ORIGIN, Point::new(travel[0], travel[1]));
        if !machine.contains_rect(&bounds) {
            errors.push(PreflightError::OutsideTravel {
                bounds,
                travel: [travel[0], travel[1]],
            });
        }
    }
}

fn check_job(
    job: &JobSummary,
    kind: JobKind,
    profile: &MachineProfile,
    errors: &mut Vec<PreflightError>,
) {
    match job.bounds {
        None => errors.push(PreflightError::NothingToRun),
        Some(bounds) => {
            let area = profile.work_area();
            if !area.contains_rect(&bounds) {
                errors.push(PreflightError::OutsideWorkArea { bounds, area });
            }
        }
    }
    let limit_percent = match kind {
        JobKind::Job => profile.max_power_percent(),
        JobKind::Frame => profile.frame_power_percent(),
    };
    let max = power_to_s(profile, limit_percent);
    if job.max_s > max {
        errors.push(PreflightError::PowerAboveProfile {
            s: job.max_s,
            max,
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{ConnectionState, DeviceEvent};
    use crate::protocol::settings::GrblSettings;
    use crate::protocol::status::parse_status;

    const IDLE: &str = "<Idle|MPos:0.000,0.000,0.000|FS:0,0|WCO:0.000,0.000,0.000|Ov:100,100,100>";

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    fn settings(pairs: &[(u16, &str)]) -> GrblSettings {
        let mut s = GrblSettings::default();
        for (n, v) in pairs {
            s.insert(*n, v).unwrap();
        }
        s
    }

    /// As the session reports it, with a zero WCO seen earlier.
    fn status(line: &str) -> DeviceEvent {
        let report = parse_status(line).unwrap();
        let positions = report.positions(Some([0.0; 3]));
        DeviceEvent::Status { report, positions }
    }

    /// Connected, idle, laser mode on, $30=1000, 300 x 200 mm travel.
    fn ready_snapshot() -> DeviceSnapshot {
        let mut s = DeviceSnapshot::default();
        s.apply(&DeviceEvent::ConnectionChanged(ConnectionState::Opening));
        s.apply(&DeviceEvent::ConnectionChanged(ConnectionState::Ready {
            version: "1.1h".into(),
        }));
        s.apply(&DeviceEvent::SettingsLoaded {
            settings: settings(&[
                (30, "1000"),
                (32, "1"),
                (130, "300"),
                (131, "200"),
                (132, "50"),
            ]),
            errors: Vec::new(),
        });
        s.apply(&status(IDLE));
        s
    }

    fn summary(x0: f64, y0: f64, x1: f64, y1: f64, max_s: u32) -> JobSummary {
        JobSummary {
            bounds: Some(Rect::new(Point::new(x0, y0), Point::new(x1, y1))),
            max_s,
        }
    }

    fn check(snapshot: &DeviceSnapshot, job: &JobSummary, kind: JobKind) -> Vec<PreflightError> {
        preflight(snapshot, job, kind, &MachineProfile::default())
            .err()
            .unwrap_or_default()
    }

    #[test]
    fn scan_reads_bounds_and_max_power_of_generated_gcode() {
        let job = lines(
            "; PhotonForge job\nG21 G90\nM4 S0\nG0 X10 Y20\nG1 X30 Y20 F1200 S500\n\
             G1 X30 Y45.5\nM3 S0\nG1 X10 Y20 S999.2\nM5\n",
        );
        assert_eq!(scan_job(&job), Ok(summary(10.0, 20.0, 30.0, 45.5, 1000)));
    }

    #[test]
    fn scan_handles_modal_axes_comments_and_spacing() {
        let job =
            lines("g0x1 y2 (go)\nX5 ; modal move\nG1Y-0.5F300S10\nG4 P0.5\nM8\nN10 M9\nG1 Z1");
        assert_eq!(scan_job(&job), Ok(summary(1.0, -0.5, 5.0, 2.0, 10)));
        assert_eq!(
            scan_job(&lines("G21\nM5")),
            Ok(JobSummary {
                bounds: None,
                max_s: 0
            })
        );
    }

    #[test]
    fn scan_refuses_what_it_cannot_verify() {
        for (text, word) in [
            ("G91\nG0 X1 Y1", "G91"),
            ("G20", "G20"),
            ("G2 X1 Y1 I1 J0", "G2"),
            ("G28", "G28"),
            ("G92 X0 Y0", "G92"),
            ("G55", "G55"),
            ("G53 G0 X0 Y0", "G53"),
            ("G38.2 Z-5", "G38.2"),
            ("M2", "M2"),
            ("T1", "T1"),
            ("S-5", "S-5"),
        ] {
            match scan_job(&lines(text)) {
                Err(ScanError::Unsupported { word: w, .. }) => assert_eq!(w, word, "{text}"),
                other => panic!("{text}: {other:?}"),
            }
        }
        assert!(matches!(
            scan_job(&lines("G0 X0 Y0\n$H")),
            Err(ScanError::SystemCommand { line: 1, .. })
        ));
        assert!(matches!(
            scan_job(&lines("$32=0")),
            Err(ScanError::SystemCommand { .. })
        ));
        assert!(matches!(
            scan_job(&lines("G0 X1.2.3")),
            Err(ScanError::BadWord { .. })
        ));
        assert!(matches!(
            scan_job(&lines("G0 #1")),
            Err(ScanError::BadWord { .. })
        ));
        assert_eq!(
            scan_job(&lines("G21\nG0 X5")),
            Err(ScanError::IncompleteFirstMove { line: 1 })
        );
    }

    #[test]
    fn preflight_passes_when_everything_is_in_order() {
        let s = ready_snapshot();
        assert_eq!(
            preflight(
                &s,
                &summary(0.0, 0.0, 300.0, 200.0, 1000),
                JobKind::Job,
                &MachineProfile::default()
            ),
            Ok(())
        );
        let frame = summary(10.0, 10.0, 50.0, 50.0, 0);
        assert!(check(&s, &frame, JobKind::Frame).is_empty());
    }

    #[test]
    fn preflight_needs_a_ready_connection() {
        let job = summary(0.0, 0.0, 10.0, 10.0, 100);
        assert_eq!(
            check(&DeviceSnapshot::default(), &job, JobKind::Job),
            vec![PreflightError::NotConnected]
        );
        let mut s = ready_snapshot();
        s.apply(&DeviceEvent::Unresponsive);
        assert_eq!(
            check(&s, &job, JobKind::Job),
            vec![PreflightError::Unresponsive]
        );

        let mut s = DeviceSnapshot::default();
        s.apply(&DeviceEvent::ConnectionChanged(ConnectionState::Ready {
            version: "1.1h".into(),
        }));
        let errors = check(&s, &job, JobKind::Job);
        assert!(errors.contains(&PreflightError::NoStatus));
        assert!(errors.contains(&PreflightError::SettingsNotLoaded));
        assert!(errors.contains(&PreflightError::OverridesUnknown));
    }

    #[test]
    fn preflight_needs_idle_without_alarm() {
        let job = summary(0.0, 0.0, 10.0, 10.0, 100);
        let mut s = ready_snapshot();
        s.apply(&status("<Run|MPos:0,0,0|FS:0,0>"));
        assert_eq!(
            check(&s, &job, JobKind::Job),
            vec![PreflightError::NotIdle(MachineState::Run)]
        );
        s.apply(&status("<Hold:0|MPos:0,0,0|FS:0,0>"));
        assert_eq!(
            check(&s, &job, JobKind::Job),
            vec![PreflightError::NotIdle(MachineState::Hold(Some(0)))]
        );

        let mut s = ready_snapshot();
        s.apply(&DeviceEvent::Alarm(2));
        s.apply(&status("<Alarm|MPos:0,0,0|FS:0,0>"));
        assert_eq!(
            check(&s, &job, JobKind::Job),
            vec![PreflightError::Alarm { code: Some(2) }]
        );
    }

    #[test]
    fn preflight_needs_laser_mode_and_a_known_s_max() {
        let job = summary(0.0, 0.0, 10.0, 10.0, 100);
        for laser_mode in [Some("0"), None] {
            let mut s = ready_snapshot();
            let mut pairs = vec![(30, "1000")];
            if let Some(v) = laser_mode {
                pairs.push((32, v));
            }
            s.apply(&DeviceEvent::SettingsLoaded {
                settings: settings(&pairs),
                errors: Vec::new(),
            });
            assert_eq!(
                check(&s, &job, JobKind::Job),
                vec![PreflightError::LaserModeOff]
            );
        }
        let mut s = ready_snapshot();
        s.apply(&DeviceEvent::SettingsLoaded {
            settings: settings(&[(32, "1")]),
            errors: Vec::new(),
        });
        assert_eq!(
            check(&s, &job, JobKind::Job),
            vec![PreflightError::MaxSpindleUnknown]
        );
    }

    #[test]
    fn preflight_refuses_a_spindle_override_above_100() {
        let job = summary(0.0, 0.0, 10.0, 10.0, 100);
        let mut s = ready_snapshot();
        s.apply(&status("<Idle|MPos:0,0,0|FS:0,0|Ov:100,100,110>"));
        assert_eq!(
            check(&s, &job, JobKind::Job),
            vec![PreflightError::SpindleOverride(110)]
        );
        // A report without `Ov` keeps the last known values.
        s.apply(&status("<Idle|MPos:0,0,0|FS:0,0>"));
        assert_eq!(
            check(&s, &job, JobKind::Job),
            vec![PreflightError::SpindleOverride(110)]
        );
        s.apply(&status("<Idle|MPos:0,0,0|FS:0,0|Ov:150,100,90>"));
        assert!(check(&s, &job, JobKind::Job).is_empty());
    }

    #[test]
    fn preflight_refuses_a_work_offset() {
        let job = summary(0.0, 0.0, 10.0, 10.0, 100);
        let mut s = ready_snapshot();
        s.apply(&status("<Idle|MPos:5,0,0|FS:0,0|WCO:5.000,0.000,0.000>"));
        assert_eq!(
            check(&s, &job, JobKind::Job),
            vec![PreflightError::WorkOffset([5.0, 0.0, 0.0])]
        );
        let mut s = ready_snapshot();
        s.positions = s.positions.map(|mut p| {
            p.work = None;
            p
        });
        assert_eq!(
            check(&s, &job, JobKind::Job),
            vec![PreflightError::WorkOffsetUnknown]
        );
    }

    #[test]
    fn preflight_keeps_jobs_inside_the_work_area_and_travel() {
        let s = ready_snapshot();
        let errors = check(&s, &summary(-1.0, 0.0, 10.0, 10.0, 0), JobKind::Job);
        assert!(matches!(
            errors[..],
            [
                PreflightError::OutsideTravel { .. },
                PreflightError::OutsideWorkArea { .. }
            ]
        ));
        // The controller's travel can be smaller than the profile's area.
        let mut s = ready_snapshot();
        s.apply(&DeviceEvent::SettingsLoaded {
            settings: settings(&[
                (30, "1000"),
                (32, "1"),
                (130, "200"),
                (131, "200"),
                (132, "50"),
            ]),
            errors: Vec::new(),
        });
        assert!(matches!(
            check(&s, &summary(0.0, 0.0, 250.0, 10.0, 0), JobKind::Job)[..],
            [PreflightError::OutsideTravel { .. }]
        ));
        let nothing = JobSummary {
            bounds: None,
            max_s: 0,
        };
        assert_eq!(
            check(&ready_snapshot(), &nothing, JobKind::Job),
            vec![PreflightError::NothingToRun]
        );
    }

    #[test]
    fn preflight_limits_power_to_the_controller_and_the_profile() {
        let s = ready_snapshot();
        assert_eq!(
            check(&s, &summary(0.0, 0.0, 1.0, 1.0, 1001), JobKind::Job),
            vec![
                PreflightError::PowerAboveController { s: 1001, max: 1000 },
                PreflightError::PowerAboveProfile {
                    s: 1001,
                    max: 1000,
                    kind: JobKind::Job
                }
            ]
        );
        let limited = MachineProfile::new(
            Rect::new(Point::ORIGIN, Point::new(300.0, 200.0)),
            pf_core::Origin::BottomLeft,
            1000,
            6000.0,
            50.0,
            5.0,
        )
        .unwrap();
        let job = summary(0.0, 0.0, 1.0, 1.0, 600);
        assert_eq!(
            preflight(&s, &job, JobKind::Job, &limited),
            Err(vec![PreflightError::PowerAboveProfile {
                s: 600,
                max: 500,
                kind: JobKind::Job
            }])
        );
        // A frame is limited to the frame power, 0 by default.
        let frame = summary(0.0, 0.0, 1.0, 1.0, 1);
        assert_eq!(
            check(&s, &frame, JobKind::Frame),
            vec![PreflightError::PowerAboveProfile {
                s: 1,
                max: 0,
                kind: JobKind::Frame
            }]
        );
        assert_eq!(
            preflight(
                &s,
                &summary(0.0, 0.0, 1.0, 1.0, 50),
                JobKind::Frame,
                &limited
            ),
            Ok(())
        );
    }

    #[test]
    fn preflight_reports_every_problem_at_once() {
        let mut s = ready_snapshot();
        s.apply(&DeviceEvent::SettingsLoaded {
            settings: settings(&[(30, "1000"), (32, "0")]),
            errors: Vec::new(),
        });
        s.apply(&status("<Run|MPos:0,0,0|FS:0,0|Ov:100,100,200>"));
        let errors = check(&s, &summary(0.0, 0.0, 400.0, 10.0, 2000), JobKind::Job);
        assert_eq!(errors.len(), 6, "{errors:?}");
    }
}
