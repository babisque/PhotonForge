//! A simulated GRBL 1.1 controller for tests: a transport that behaves like
//! the firmware closely enough to test streaming and machine control
//! without hardware.
//!
//! - The serial receive buffer holds `rx_buffer - 1` bytes, as GRBL's ring
//!   buffer does; anything written past that is dropped and recorded as an
//!   overflow, which tests assert never happens.
//! - Real-time bytes (`?`, `!`, `~`, 0x18, 0x85) act at once and never take
//!   buffer space.
//! - Lines are parsed a few per `read` into a 15-block planner, as GRBL
//!   does; each move then takes a few reads to run. GRBL stops reading its
//!   buffer while the planner is full, so a fast sender fills the buffer.
//!   Positions jump to the target when a move is parsed.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::protocol::status::MachineState;
use crate::stream::DEFAULT_RX_BUFFER_BYTES;
use crate::transport::Transport;

const BANNER: &str = "\r\nGrbl 1.1h ['$' for help]\r\n";
const SETTINGS: &str = "$10=1\r\n$13=0\r\n$22=0\r\n$30=1000\r\n$32=1\r\n$130=300.000\r\n$131=200.000\r\n$132=50.000\r\nok\r\n";
const SETTINGS_HOMING: &str = "$10=1\r\n$13=0\r\n$22=1\r\n$30=1000\r\n$32=1\r\n$130=300.000\r\n$131=200.000\r\n$132=50.000\r\nok\r\n";
/// Reads a homing cycle takes by default.
const HOMING_READS: usize = 3;
/// GRBL's planner size, `BLOCK_BUFFER_SIZE` (the `15` in `[OPT:V,15,128]`).
const PLANNER_BLOCKS: usize = 15;

#[derive(Debug, Clone, Copy)]
pub(crate) struct SimConfig {
    pub(crate) rx_buffer: usize,
    /// Lines parsed per `read`.
    pub(crate) lines_per_read: usize,
    /// Reads each move takes to run.
    pub(crate) reads_per_block: usize,
    pub(crate) homing: bool,
    /// Reads a homing cycle takes before it completes.
    pub(crate) homing_reads: usize,
    /// Print the welcome message on connect, as after a DTR reset.
    pub(crate) banner_on_connect: bool,
    /// `DISABLE_LASER_DURING_HOLD` in GRBL's `config.h`, on by default: a
    /// spindle stop override turns the laser off once a hold completes.
    pub(crate) laser_off_in_hold: bool,
}

impl Default for SimConfig {
    fn default() -> Self {
        SimConfig {
            rx_buffer: DEFAULT_RX_BUFFER_BYTES,
            lines_per_read: 1,
            reads_per_block: 2,
            homing: false,
            homing_reads: HOMING_READS,
            banner_on_connect: true,
            laser_off_in_hold: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Spindle {
    Off,
    Constant, // M3
    Dynamic,  // M4
}

#[derive(Debug)]
struct Grbl {
    config: SimConfig,
    rx: VecDeque<u8>,
    out: Vec<u8>,
    written: Vec<u8>,
    overflowed: bool,
    max_rx_used: usize,
    state: MachineState,
    pos: [f64; 2],
    spindle: Spindle,
    s: f64,
    /// Spindle stop override active: the laser output is off while held.
    stop_override: bool,
    /// Moves parsed but not yet run.
    planner: usize,
    /// Modal motion is G1 (cutting) rather than G0.
    feed_motion: bool,
    /// A cutting move ran with the laser on: in laser mode, G1 with M3/M4
    /// and S above 0 (GRBL keeps the laser off during G0).
    laser_fired: bool,
    /// Reads spent on the move now running.
    block_reads: usize,
    gcode_lines: Vec<String>,
    /// Answer the G-code line with this index (from 0) with `error:code`.
    fail_at: Vec<(usize, u8)>,
    homing_reads_left: usize,
    unplugged: bool,
}

impl Grbl {
    fn new(config: SimConfig) -> Grbl {
        let mut grbl = Grbl {
            config,
            rx: VecDeque::new(),
            out: Vec::new(),
            written: Vec::new(),
            overflowed: false,
            max_rx_used: 0,
            state: MachineState::Idle,
            pos: [0.0, 0.0],
            spindle: Spindle::Off,
            s: 0.0,
            stop_override: false,
            planner: 0,
            feed_motion: false,
            laser_fired: false,
            block_reads: 0,
            gcode_lines: Vec::new(),
            fail_at: Vec::new(),
            homing_reads_left: 0,
            unplugged: false,
        };
        if config.banner_on_connect {
            grbl.boot(false);
        }
        grbl
    }

    fn capacity(&self) -> usize {
        self.config.rx_buffer.saturating_sub(1)
    }

    fn reply(&mut self, text: &str) {
        self.out.extend_from_slice(text.as_bytes());
        self.out.extend_from_slice(b"\r\n");
    }

    /// Start-up after power-on or a soft reset. GRBL raises ALARM:3 when it
    /// was reset while moving. This simulator counts a hold as moving, the
    /// cautious reading; callers must handle both outcomes anyway.
    fn boot(&mut self, aborted_motion: bool) {
        self.rx.clear();
        self.spindle = Spindle::Off;
        self.s = 0.0;
        self.stop_override = false;
        self.planner = 0;
        self.block_reads = 0;
        self.homing_reads_left = 0;
        if aborted_motion {
            self.reply("ALARM:3");
        }
        self.out.extend_from_slice(BANNER.as_bytes());
        if aborted_motion {
            self.state = MachineState::Alarm;
            self.reply("[MSG:Reset to continue]");
        } else if self.config.homing {
            self.state = MachineState::Alarm;
            self.reply("[MSG:'$H'|'$X' to unlock]");
        } else {
            self.state = MachineState::Idle;
        }
    }

    fn moving(&self) -> bool {
        matches!(
            self.state,
            MachineState::Run | MachineState::Jog | MachineState::Home | MachineState::Hold(_)
        )
    }

    fn on_write(&mut self, bytes: &[u8]) {
        self.written.extend_from_slice(bytes);
        for &byte in bytes {
            match byte {
                b'?' => {
                    let report = self.status_report();
                    self.reply(&report);
                }
                b'!' => {
                    if matches!(self.state, MachineState::Run | MachineState::Jog) {
                        self.state = MachineState::Hold(Some(1));
                    }
                }
                b'~' => {
                    if self.state == MachineState::Hold(Some(0)) {
                        self.state = MachineState::Run;
                        self.stop_override = false;
                    }
                }
                0x18 => {
                    let moving = self.moving();
                    self.boot(moving);
                }
                0x85 => {
                    if self.state == MachineState::Jog {
                        self.state = MachineState::Idle;
                    }
                }
                // Override commands; not simulated.
                0x80..=0xFF => {}
                b'\r' => {}
                _ => {
                    if self.rx.len() >= self.capacity() {
                        self.overflowed = true;
                    } else {
                        self.rx.push_back(byte);
                        self.max_rx_used = self.max_rx_used.max(self.rx.len());
                    }
                }
            }
        }
    }

    fn status_report(&self) -> String {
        let state = match self.state {
            MachineState::Idle => "Idle".to_owned(),
            MachineState::Run => "Run".to_owned(),
            MachineState::Hold(sub) => format!("Hold:{}", sub.unwrap_or(0)),
            MachineState::Jog => "Jog".to_owned(),
            MachineState::Alarm => "Alarm".to_owned(),
            MachineState::Home => "Home".to_owned(),
            other => format!("{other:?}"),
        };
        let free = self.capacity() - self.rx.len();
        let blocks_free = PLANNER_BLOCKS - self.planner;
        let accessories = match self.spindle {
            _ if self.stop_override || self.s <= 0.0 => "",
            Spindle::Off => "",
            Spindle::Constant => "|A:S",
            Spindle::Dynamic => "|A:C",
        };
        format!(
            "<{state}|MPos:{:.3},{:.3},0.000|Bf:{blocks_free},{free}|FS:0,{}|WCO:0.000,0.000,0.000|Ov:100,100,100{accessories}>",
            self.pos[0], self.pos[1], self.s
        )
    }

    /// One serial read's worth of controller time.
    fn advance(&mut self) {
        match self.state {
            MachineState::Hold(Some(1)) => {
                self.state = MachineState::Hold(Some(0));
                self.stop_override = self.config.laser_off_in_hold;
                return;
            }
            MachineState::Hold(_) => return,
            MachineState::Home => {
                self.homing_reads_left = self.homing_reads_left.saturating_sub(1);
                if self.homing_reads_left == 0 {
                    self.state = MachineState::Idle;
                    self.pos = [0.0, 0.0];
                    self.reply("ok");
                }
                return;
            }
            MachineState::Jog => self.state = MachineState::Idle,
            _ => {}
        }
        for _ in 0..self.config.lines_per_read {
            if self.planner >= PLANNER_BLOCKS {
                break;
            }
            let Some(line) = self.take_line() else {
                break;
            };
            self.execute(&line);
            if self.state == MachineState::Home {
                return;
            }
        }
        if self.planner > 0 {
            self.block_reads += 1;
            if self.block_reads >= self.config.reads_per_block {
                self.planner -= 1;
                self.block_reads = 0;
            }
        }
        if matches!(self.state, MachineState::Run | MachineState::Idle) {
            self.state = if self.planner > 0 || self.rx.contains(&b'\n') {
                MachineState::Run
            } else {
                MachineState::Idle
            };
        }
    }

    fn take_line(&mut self) -> Option<String> {
        let end = self.rx.iter().position(|&b| b == b'\n')?;
        let bytes: Vec<u8> = self.rx.drain(..=end).collect();
        Some(String::from_utf8_lossy(&bytes[..end]).trim().to_owned())
    }

    fn execute(&mut self, line: &str) {
        let alarm = self.state == MachineState::Alarm;
        match line {
            "" => self.reply("ok"),
            "$X" => {
                if alarm {
                    self.state = MachineState::Idle;
                    self.reply("[MSG:Caution: Unlocked]");
                }
                self.reply("ok");
            }
            "$H" => {
                if !self.config.homing {
                    self.reply("error:5");
                } else {
                    self.state = MachineState::Home;
                    self.homing_reads_left = self.config.homing_reads;
                }
            }
            "$$" => {
                let dump = if self.config.homing {
                    SETTINGS_HOMING
                } else {
                    SETTINGS
                };
                self.out.extend_from_slice(dump.as_bytes());
            }
            "$I" => self
                .out
                .extend_from_slice(b"[VER:1.1h.20190825:]\r\n[OPT:V,15,128]\r\nok\r\n" as &[u8]),
            "$G" => {
                let m = match self.spindle {
                    Spindle::Off => "M5",
                    Spindle::Constant => "M3",
                    Spindle::Dynamic => "M4",
                };
                let gc = format!("[GC:G0 G54 G17 G21 G90 G94 {m} M9 T0 F0 S{}]", self.s);
                self.reply(&gc);
                self.reply("ok");
            }
            _ if line.starts_with("$J=") => {
                if alarm {
                    self.reply("error:9");
                } else if let Some(target) = self.jog_target(&line[3..]) {
                    self.pos = target;
                    self.state = MachineState::Jog;
                    self.reply("ok");
                } else {
                    self.reply("error:16");
                }
            }
            _ if line.starts_with('$') => self.reply("error:3"),
            _ => self.execute_gcode(line, alarm),
        }
    }

    fn execute_gcode(&mut self, line: &str, alarm: bool) {
        let index = self.gcode_lines.len();
        self.gcode_lines.push(line.to_owned());
        if alarm {
            self.reply("error:9");
            return;
        }
        if let Some(&(_, code)) = self.fail_at.iter().find(|(i, _)| *i == index) {
            self.reply(&format!("error:{code}"));
            return;
        }
        let mut moves = false;
        for (letter, value) in sim_words(line) {
            match letter {
                'G' if value == 0.0 => self.feed_motion = false,
                'G' if value == 1.0 => self.feed_motion = true,
                'M' if value == 3.0 => self.spindle = Spindle::Constant,
                'M' if value == 4.0 => self.spindle = Spindle::Dynamic,
                'M' if value == 5.0 => self.spindle = Spindle::Off,
                'S' => self.s = value,
                'X' => {
                    self.pos[0] = value;
                    moves = true;
                }
                'Y' => {
                    self.pos[1] = value;
                    moves = true;
                }
                _ => {}
            }
        }
        if moves {
            self.planner += 1;
            if self.feed_motion && self.spindle != Spindle::Off && self.s > 0.0 {
                self.laser_fired = true;
            }
        }
        self.state = MachineState::Run;
        self.reply("ok");
    }

    /// `G91 X.. Y.. F..` relative, or `G90`/no mode absolute. Needs a feed.
    fn jog_target(&self, words: &str) -> Option<[f64; 2]> {
        let words = sim_words(words);
        let relative = words.iter().any(|&(l, v)| l == 'G' && v == 91.0);
        if !words.iter().any(|&(l, _)| l == 'F') {
            return None;
        }
        let mut target = self.pos;
        for (letter, value) in words {
            let axis = match letter {
                'X' => 0,
                'Y' => 1,
                _ => continue,
            };
            target[axis] = if relative {
                self.pos[axis] + value
            } else {
                value
            };
        }
        Some(target)
    }
}

fn sim_words(line: &str) -> Vec<(char, f64)> {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    let mut out = Vec::new();
    let mut rest = compact.as_str();
    while let Some(letter) = rest.chars().next() {
        let number_len = rest[1..]
            .find(|c: char| c.is_ascii_alphabetic())
            .unwrap_or(rest.len() - 1);
        if let Ok(value) = rest[1..1 + number_len].parse() {
            out.push((letter.to_ascii_uppercase(), value));
        }
        rest = &rest[1 + number_len..];
    }
    out
}

/// The transport side, moved into the connection under test.
pub(crate) struct SimTransport {
    grbl: Arc<Mutex<Grbl>>,
}

/// The test's side: inspects and disturbs the controller.
#[derive(Clone)]
pub(crate) struct SimHandle {
    grbl: Arc<Mutex<Grbl>>,
}

pub(crate) fn sim(config: SimConfig) -> (SimTransport, SimHandle) {
    let grbl = Arc::new(Mutex::new(Grbl::new(config)));
    (
        SimTransport {
            grbl: Arc::clone(&grbl),
        },
        SimHandle { grbl },
    )
}

impl SimHandle {
    fn with<R>(&self, f: impl FnOnce(&mut Grbl) -> R) -> R {
        f(&mut self.grbl.lock().unwrap())
    }

    pub(crate) fn written(&self) -> Vec<u8> {
        self.with(|g| g.written.clone())
    }

    pub(crate) fn overflowed(&self) -> bool {
        self.with(|g| g.overflowed)
    }

    pub(crate) fn max_rx_used(&self) -> usize {
        self.with(|g| g.max_rx_used)
    }

    pub(crate) fn state(&self) -> MachineState {
        self.with(|g| g.state)
    }

    pub(crate) fn position(&self) -> [f64; 2] {
        self.with(|g| g.pos)
    }

    pub(crate) fn spindle(&self) -> (Spindle, f64) {
        self.with(|g| (g.spindle, g.s))
    }

    /// Whether any cutting move ran with the laser on.
    pub(crate) fn laser_fired(&self) -> bool {
        self.with(|g| g.laser_fired)
    }

    /// G-code lines received, in order, `$` commands excluded.
    pub(crate) fn gcode_lines(&self) -> Vec<String> {
        self.with(|g| g.gcode_lines.clone())
    }

    pub(crate) fn fail_gcode_line(&self, index: usize, code: u8) {
        self.with(|g| g.fail_at.push((index, code)));
    }

    /// An alarm such as a hard limit: GRBL resets, which stops motion and
    /// the laser and drops unprocessed input.
    pub(crate) fn raise_alarm(&self, code: u8) {
        self.with(|g| {
            g.rx.clear();
            g.planner = 0;
            g.spindle = Spindle::Off;
            g.s = 0.0;
            g.state = MachineState::Alarm;
            g.reply(&format!("ALARM:{code}"));
        });
    }

    /// Every later read and write fails, as when the cable is pulled.
    pub(crate) fn unplug(&self) {
        self.with(|g| g.unplugged = true);
    }

    pub(crate) fn is_closed(&self) -> bool {
        Arc::strong_count(&self.grbl) == 1
    }
}

impl Transport for SimTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut grbl = self.grbl.lock().unwrap();
        if grbl.unplugged {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        grbl.advance();
        if grbl.out.is_empty() {
            drop(grbl);
            // Like a serial read timing out with no data.
            std::thread::sleep(Duration::from_millis(1));
            return Ok(0);
        }
        let n = grbl.out.len().min(buf.len());
        buf[..n].copy_from_slice(&grbl.out[..n]);
        grbl.out.drain(..n);
        Ok(n)
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut grbl = self.grbl.lock().unwrap();
        if grbl.unplugged {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        grbl.on_write(bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> (SimTransport, SimHandle) {
        sim(SimConfig {
            banner_on_connect: false,
            ..SimConfig::default()
        })
    }

    /// Everything the controller says over `reads` reads.
    fn read_text(t: &mut SimTransport, reads: usize) -> String {
        let mut text = Vec::new();
        let mut buf = [0u8; 512];
        for _ in 0..reads {
            let n = t.read(&mut buf).unwrap();
            text.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&text).into_owned()
    }

    #[test]
    fn sim_greets_and_answers_the_read_only_requests() {
        let (mut t, _h) = sim(SimConfig::default());
        assert!(read_text(&mut t, 1).contains("Grbl 1.1h ['$' for help]"));
        t.write_all(b"$I\n$$\n$G\n").unwrap();
        let text = read_text(&mut t, 3);
        assert!(text.contains("[OPT:V,15,128]"));
        assert!(text.contains("$32=1"));
        assert!(text.contains("[GC:G0 G54 G17 G21 G90 G94 M5"));
        assert_eq!(text.matches("ok").count(), 3);
    }

    #[test]
    fn sim_detects_an_overflowing_sender() {
        let (mut t, h) = quiet();
        t.write_all(&[b'G'; 127]).unwrap();
        assert!(!h.overflowed());
        assert_eq!(h.max_rx_used(), 127);
        t.write_all(b"\n").unwrap();
        assert!(h.overflowed());
    }

    #[test]
    fn sim_realtime_bytes_take_no_buffer_space() {
        let (mut t, h) = quiet();
        t.write_all(&[b'G'; 127]).unwrap();
        t.write_all(b"?!~\x18").unwrap();
        // The reset emptied the buffer; nothing was dropped.
        assert!(!h.overflowed());
        let text = read_text(&mut t, 1);
        assert!(text.starts_with("<Idle|MPos:0.000,0.000,0.000|Bf:15,0|"));
    }

    #[test]
    fn sim_executes_lines_a_few_per_read() {
        let (mut t, h) = quiet();
        t.write_all(b"M4 S0\nG0 X10 Y5\nG1 X20 S300 F600\n")
            .unwrap();
        assert_eq!(read_text(&mut t, 1), "ok\r\n");
        assert_eq!(h.state(), MachineState::Run);
        assert_eq!(read_text(&mut t, 2), "ok\r\nok\r\n");
        // Parsed, but the planner still runs the last move.
        assert_eq!(h.state(), MachineState::Run);
        t.write_all(b"?").unwrap();
        // The report is made on arrival; the read then runs the move.
        assert!(read_text(&mut t, 1).contains("|Bf:14,127|"));
        read_text(&mut t, 1);
        assert_eq!(h.state(), MachineState::Idle);
        assert_eq!(h.position(), [20.0, 5.0]);
        assert_eq!(h.spindle(), (Spindle::Dynamic, 300.0));
        assert_eq!(h.gcode_lines().len(), 3);
    }

    #[test]
    fn sim_stops_reading_while_the_planner_is_full() {
        let (mut t, h) = sim(SimConfig {
            banner_on_connect: false,
            lines_per_read: 4,
            ..SimConfig::default()
        });
        // 110 bytes: fits the receive buffer.
        for i in 0..20 {
            t.write_all(format!("G0X{i}\n").as_bytes()).unwrap();
        }
        assert!(!h.overflowed());
        read_text(&mut t, 6);
        // Parsing outpaces the moves (one every two reads), so the planner
        // filled up and the last lines still wait in the receive buffer.
        assert!(h.gcode_lines().len() < 20, "{}", h.gcode_lines().len());
        // Full, or one block freed by the move that just finished.
        t.write_all(b"?").unwrap();
        let report = read_text(&mut t, 1);
        assert!(
            report.contains("|Bf:0,") || report.contains("|Bf:1,"),
            "{report}"
        );
        read_text(&mut t, 60);
        assert_eq!(h.gcode_lines().len(), 20);
        assert_eq!(h.state(), MachineState::Idle);
    }

    #[test]
    fn sim_holds_and_resumes() {
        let (mut t, h) = quiet();
        t.write_all(b"G0 X1\nG0 X2\nG0 X3\n").unwrap();
        read_text(&mut t, 1);
        t.write_all(b"!").unwrap();
        assert_eq!(h.state(), MachineState::Hold(Some(1)));
        read_text(&mut t, 1);
        assert_eq!(h.state(), MachineState::Hold(Some(0)));
        // Nothing runs while held.
        assert_eq!(read_text(&mut t, 3), "");
        t.write_all(b"~").unwrap();
        assert_eq!(read_text(&mut t, 2), "ok\r\nok\r\n");
        assert_eq!(h.position(), [3.0, 0.0]);
    }

    #[test]
    fn sim_turns_the_laser_off_in_a_hold_unless_built_without_it() {
        for (laser_off_in_hold, held_report) in [(true, ">"), (false, "|A:S>")] {
            let (mut t, _h) = sim(SimConfig {
                banner_on_connect: false,
                laser_off_in_hold,
                ..SimConfig::default()
            });
            t.write_all(
                b"M3 S500
G1 X1 F100
G1 X2
",
            )
            .unwrap();
            read_text(&mut t, 1);
            t.write_all(b"?").unwrap();
            assert!(read_text(&mut t, 1).contains("|A:S>"));
            t.write_all(b"!").unwrap();
            read_text(&mut t, 1);
            t.write_all(b"?").unwrap();
            let report = read_text(&mut t, 1);
            assert!(report.starts_with("<Hold:0"), "{report}");
            assert!(
                report
                    .trim_end()
                    .ends_with(&format!("Ov:100,100,100{held_report}")),
                "{report}"
            );
        }
    }

    #[test]
    fn sim_reset_while_moving_alarms_and_turns_the_laser_off() {
        let (mut t, h) = quiet();
        t.write_all(b"M3 S500\nG1 X1 F100\nG1 X2\n").unwrap();
        read_text(&mut t, 1);
        t.write_all(b"\x18").unwrap();
        let text = read_text(&mut t, 1);
        assert!(text.starts_with("ALARM:3\r\n\r\nGrbl 1.1h"), "{text:?}");
        assert_eq!(h.state(), MachineState::Alarm);
        assert_eq!(h.spindle(), (Spindle::Off, 0.0));
        // Locked until unlocked.
        t.write_all(b"G0 X5\n$X\nM5\n").unwrap();
        let text = read_text(&mut t, 3);
        assert_eq!(text, "error:9\r\n[MSG:Caution: Unlocked]\r\nok\r\nok\r\n");
        assert_eq!(h.state(), MachineState::Idle);

        // A reset while idle does not alarm.
        t.write_all(b"\x18").unwrap();
        assert!(read_text(&mut t, 1).starts_with("\r\nGrbl"));
        assert_eq!(h.state(), MachineState::Idle);
    }

    #[test]
    fn sim_homes_only_when_enabled() {
        let (mut t, h) = quiet();
        t.write_all(b"$H\n").unwrap();
        assert_eq!(read_text(&mut t, 1), "error:5\r\n");

        let (mut t, h2) = sim(SimConfig {
            homing: true,
            ..SimConfig::default()
        });
        assert!(read_text(&mut t, 1).contains("'$H'|'$X' to unlock"));
        assert_eq!(h2.state(), MachineState::Alarm);
        t.write_all(b"$H\n").unwrap();
        // One read starts the cycle, then it takes `HOMING_READS`.
        assert_eq!(read_text(&mut t, HOMING_READS), "");
        assert_eq!(h2.state(), MachineState::Home);
        assert_eq!(read_text(&mut t, 1), "ok\r\n");
        assert_eq!(h2.state(), MachineState::Idle);
        assert_eq!(h.state(), MachineState::Idle);
    }

    #[test]
    fn sim_jogs_and_cancels() {
        let (mut t, h) = quiet();
        t.write_all(b"$J=G91 X10 Y-2 F1000\n").unwrap();
        assert_eq!(read_text(&mut t, 1), "ok\r\n");
        assert_eq!(h.state(), MachineState::Jog);
        assert_eq!(h.position(), [10.0, -2.0]);
        t.write_all(&[0x85]).unwrap();
        assert_eq!(h.state(), MachineState::Idle);
        t.write_all(b"$J=G91 X1\n").unwrap();
        assert_eq!(read_text(&mut t, 1), "error:16\r\n");
    }

    #[test]
    fn sim_injects_errors_alarms_and_unplugging() {
        let (mut t, h) = quiet();
        h.fail_gcode_line(1, 20);
        t.write_all(b"G0 X1\nG5 X2\nG0 X3\n").unwrap();
        assert_eq!(read_text(&mut t, 3), "ok\r\nerror:20\r\nok\r\n");

        h.raise_alarm(1);
        assert_eq!(read_text(&mut t, 1), "ALARM:1\r\n");
        assert_eq!(h.state(), MachineState::Alarm);

        h.unplug();
        assert!(t.read(&mut [0u8; 8]).is_err());
        assert!(t.write_all(b"?").is_err());
        drop(t);
        assert!(h.is_closed());
    }
}
