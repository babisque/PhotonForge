use pf_core::{Document, MachineProfile, Point, PowerMode, power_to_s};

use crate::options::JobOptions;
use crate::toolpath::Toolpath;

/// Writes G-code for toolpaths that already passed `validate`, in machine
/// coordinates and in cut order. Each layer is repeated for its passes.
pub fn emit(
    toolpaths: &[Toolpath],
    doc: &Document,
    profile: &MachineProfile,
    options: &JobOptions,
) -> String {
    let mut w = Writer::default();
    w.comment("PhotonForge job");
    w.line("G21 G90");
    // S0 here, so the laser cannot fire before the first G0.
    w.laser_mode(
        toolpaths
            .first()
            .map_or(PowerMode::M4Dynamic, |t| t.power_mode),
    );
    if let Some(header) = &options.header {
        w.block(header);
    }
    for layer_paths in toolpaths.chunk_by(|a, b| a.layer_id == b.layer_id) {
        let Some(first) = layer_paths.first() else {
            continue;
        };
        let name = doc.layer(first.layer_id).map_or("", |l| l.name.as_str());
        for pass in 1..=first.passes {
            w.comment(&format!("Layer {name} pass {pass} of {}", first.passes));
            w.laser_mode(first.power_mode);
            for toolpath in layer_paths {
                emit_toolpath(&mut w, toolpath, profile);
            }
        }
    }
    if let Some(footer) = &options.footer {
        w.block(footer);
    }
    w.line("M5");
    w.out
}

fn emit_toolpath(w: &mut Writer, toolpath: &Toolpath, profile: &MachineProfile) {
    let Some((&start, rest)) = toolpath.points.split_first() else {
        return;
    };
    // Validated speed is within [1, max feed], so it fits in u32.
    let feed = toolpath.speed.round() as u32;
    let s = power_to_s(profile, toolpath.power);
    w.rapid(start);
    for &point in rest {
        w.cut(point, feed, s);
    }
}

/// Coordinate in mm with 3 decimals and trailing zeros trimmed.
pub fn format_coord(value: f64) -> String {
    let mut text = format!("{value:.3}");
    if text.contains('.') {
        let trimmed = text.trim_end_matches('0').trim_end_matches('.').len();
        text.truncate(trimmed);
    }
    if text == "-0" {
        text = String::from("0");
    }
    text
}

/// Keeps a comment on one line and strips characters GRBL would act on as
/// real-time commands (see `validate_template`).
fn sanitize_comment(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c == ' ' || (c.is_ascii_graphic() && !matches!(c, '?' | '!' | '~')) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Tracks modal state so `F` and `S` are written only when they change.
#[derive(Default)]
struct Writer {
    out: String,
    feed: Option<u32>,
    power: Option<u32>,
    mode: Option<PowerMode>,
}

impl Writer {
    fn line(&mut self, text: &str) {
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn block(&mut self, text: &str) {
        for line in text.lines() {
            self.line(line);
        }
    }

    fn comment(&mut self, text: &str) {
        self.line(&format!("; {}", sanitize_comment(text)));
    }

    fn laser_mode(&mut self, mode: PowerMode) {
        if self.mode == Some(mode) {
            return;
        }
        self.line(match mode {
            PowerMode::M4Dynamic => "M4 S0",
            PowerMode::M3Constant => "M3 S0",
        });
        self.mode = Some(mode);
        self.power = Some(0);
    }

    /// GRBL laser mode keeps the laser off during G0. `S` is restated on the
    /// next G1 so every cut is self-describing.
    fn rapid(&mut self, to: Point) {
        self.line(&format!(
            "G0 X{} Y{}",
            format_coord(to.x),
            format_coord(to.y)
        ));
        self.power = None;
    }

    fn cut(&mut self, to: Point, feed: u32, s: u32) {
        let mut text = format!("G1 X{} Y{}", format_coord(to.x), format_coord(to.y));
        if self.feed != Some(feed) {
            text.push_str(&format!(" F{feed}"));
            self.feed = Some(feed);
        }
        if self.power != Some(s) {
            text.push_str(&format!(" S{s}"));
            self.power = Some(s);
        }
        self.line(&text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_core::LayerId;

    #[test]
    fn coordinates_are_trimmed() {
        assert_eq!(format_coord(10.0), "10");
        assert_eq!(format_coord(10.5), "10.5");
        assert_eq!(format_coord(10.500), "10.5");
        assert_eq!(format_coord(0.125), "0.125");
        assert_eq!(format_coord(-3.25), "-3.25");
        assert_eq!(format_coord(1_000_000.0), "1000000");
        assert_eq!(format_coord(100.0), "100");
    }

    #[test]
    fn coordinates_round_to_three_decimals() {
        assert_eq!(format_coord(1.23456), "1.235");
        assert_eq!(format_coord(0.0004), "0");
        assert_eq!(format_coord(9.9999), "10");
    }

    #[test]
    fn negative_zero_is_written_as_zero() {
        assert_eq!(format_coord(-0.0), "0");
        assert_eq!(format_coord(-0.0004), "0");
    }

    #[test]
    fn feed_and_power_are_written_only_on_change() {
        let mut w = Writer::default();
        w.laser_mode(PowerMode::M4Dynamic);
        w.rapid(Point::new(1.0, 2.0));
        w.cut(Point::new(3.0, 2.0), 1200, 500);
        w.cut(Point::new(3.0, 4.0), 1200, 500);
        w.cut(Point::new(5.0, 4.0), 900, 500);
        w.cut(Point::new(5.0, 6.0), 900, 300);
        assert_eq!(
            w.out,
            "M4 S0\nG0 X1 Y2\nG1 X3 Y2 F1200 S500\nG1 X3 Y4\nG1 X5 Y4 F900\nG1 X5 Y6 S300\n"
        );
    }

    #[test]
    fn rapid_forces_power_to_be_restated() {
        let mut w = Writer::default();
        w.rapid(Point::new(0.0, 0.0));
        w.cut(Point::new(1.0, 0.0), 1000, 100);
        w.rapid(Point::new(5.0, 0.0));
        w.cut(Point::new(6.0, 0.0), 1000, 100);
        assert_eq!(
            w.out,
            "G0 X0 Y0\nG1 X1 Y0 F1000 S100\nG0 X5 Y0\nG1 X6 Y0 S100\n"
        );
    }

    #[test]
    fn laser_mode_is_written_only_when_it_changes() {
        let mut w = Writer::default();
        w.laser_mode(PowerMode::M4Dynamic);
        w.laser_mode(PowerMode::M4Dynamic);
        w.laser_mode(PowerMode::M3Constant);
        assert_eq!(w.out, "M4 S0\nM3 S0\n");
    }

    #[test]
    fn comments_cannot_break_lines_or_trigger_realtime_commands() {
        let mut w = Writer::default();
        w.comment("Cut!\nG1 X0 ~ é ?");
        assert_eq!(w.out, "; Cut__G1 X0 _ _ _\n");
    }

    #[test]
    fn blocks_are_split_into_lines() {
        let mut w = Writer::default();
        w.block("M8\r\nG4 P1\n");
        assert_eq!(w.out, "M8\nG4 P1\n");
    }

    #[test]
    fn empty_job_has_header_and_footer_only() {
        let out = emit(
            &[],
            &Document::new(),
            &MachineProfile::default(),
            &JobOptions::default(),
        );
        assert_eq!(out, "; PhotonForge job\nG21 G90\nM4 S0\nM5\n");
    }

    #[test]
    fn toolpath_without_points_emits_nothing() {
        let t = Toolpath {
            layer_id: LayerId(0),
            points: vec![],
            closed: false,
            speed: 1000.0,
            power: 10.0,
            passes: 1,
            power_mode: PowerMode::M4Dynamic,
        };
        let mut w = Writer::default();
        emit_toolpath(&mut w, &t, &MachineProfile::default());
        assert_eq!(w.out, "");
    }
}
