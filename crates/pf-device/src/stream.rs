//! Bookkeeping for streaming G-code to GRBL, without I/O.
//!
//! Character counting (GRBL `doc/markdown/interface.md`, "Streaming
//! Protocol: Character-Counting", reference script `doc/script/stream.py`):
//! GRBL answers every line with `ok` or `error:N`, in order. Track the bytes
//! of lines sent but not answered, and send the next line only if it still
//! fits in GRBL's serial receive buffer. Send-response keeps one line in
//! flight and is the slow, simple fallback.

use std::collections::VecDeque;

use thiserror::Error;

use crate::job::JobProgress;
use crate::protocol::build_info::BuildOptions;

/// GRBL's default serial receive buffer (`RX_BUFFER_SIZE` in `serial.h`).
pub const DEFAULT_RX_BUFFER_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamMode {
    #[default]
    CharacterCounting,
    SendResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StreamError {
    #[error("line {index} is {len} bytes with its newline; GRBL's buffer holds {max}")]
    LineTooLong {
        index: usize,
        len: usize,
        max: usize,
    },
    #[error("line {index} contains {ch:?}, which GRBL would act on as a real-time command")]
    UnsafeChar { index: usize, ch: char },
    #[error("an answer arrived with no line waiting for one")]
    UnexpectedAck,
}

/// Usable bytes of GRBL's receive buffer: one less than its size, because
/// the ring buffer in `serial.c` always keeps one slot empty. Uses the size
/// `$I` reported (`[OPT:…,blocks,rx]`), else GRBL's default.
pub fn rx_capacity(options: Option<&BuildOptions>) -> usize {
    let size = options.map_or(DEFAULT_RX_BUFFER_BYTES, |o| usize::from(o.rx_buffer_bytes));
    size.saturating_sub(1)
}

#[derive(Debug, Clone)]
pub struct Streamer {
    lines: Vec<String>,
    mode: StreamMode,
    next: usize,
    in_flight: VecDeque<usize>, // byte lengths awaiting an answer, newline included
    in_flight_bytes: usize,
    acked: usize,
    rx_capacity: usize,
}

impl Streamer {
    /// Blank and comment-only lines are dropped, as they only take buffer
    /// space; every other line is sent exactly as given, trimmed.
    pub fn new(
        lines: Vec<String>,
        mode: StreamMode,
        rx_capacity: usize,
    ) -> Result<Streamer, StreamError> {
        let lines: Vec<String> = lines
            .iter()
            .map(|l| l.trim())
            .filter(|l| !is_blank_or_comment(l))
            .map(str::to_owned)
            .collect();
        for (index, line) in lines.iter().enumerate() {
            if let Some(ch) = line.chars().find(|&c| !is_streamable_char(c)) {
                return Err(StreamError::UnsafeChar { index, ch });
            }
            let len = line.len() + 1;
            if len > rx_capacity {
                return Err(StreamError::LineTooLong {
                    index,
                    len,
                    max: rx_capacity,
                });
            }
        }
        Ok(Streamer {
            lines,
            mode,
            next: 0,
            in_flight: VecDeque::new(),
            in_flight_bytes: 0,
            acked: 0,
            rx_capacity,
        })
    }

    /// Lines that can be sent now without overflowing GRBL's receive buffer,
    /// in order. They count as sent: write each one followed by `\n`.
    pub fn ready_lines(&mut self) -> Vec<&str> {
        let start = self.next;
        while let Some(line) = self.lines.get(self.next) {
            let len = line.len() + 1;
            let fits = match self.mode {
                StreamMode::CharacterCounting => self.in_flight_bytes + len <= self.rx_capacity,
                StreamMode::SendResponse => self.in_flight.is_empty(),
            };
            if !fits {
                break;
            }
            self.in_flight.push_back(len);
            self.in_flight_bytes += len;
            self.next += 1;
        }
        self.lines[start..self.next]
            .iter()
            .map(String::as_str)
            .collect()
    }

    /// `ok` for the oldest line in flight. Returns that line's index.
    pub fn on_ack(&mut self) -> Result<usize, StreamError> {
        self.pop()
    }

    /// `error:N` for the oldest line in flight: GRBL dropped that line and
    /// carries on with the next. Returns the failed line's index; stopping
    /// the job is up to the caller.
    pub fn on_error(&mut self) -> Result<usize, StreamError> {
        self.pop()
    }

    fn pop(&mut self) -> Result<usize, StreamError> {
        let len = self
            .in_flight
            .pop_front()
            .ok_or(StreamError::UnexpectedAck)?;
        self.in_flight_bytes -= len;
        let index = self.acked;
        self.acked += 1;
        Ok(index)
    }

    /// `ready_lines` as values the connection may write.
    pub(crate) fn take_ready(&mut self) -> Vec<StreamedLine> {
        self.ready_lines()
            .into_iter()
            .map(|l| StreamedLine(l.to_owned()))
            .collect()
    }

    pub fn line(&self, index: usize) -> Option<&str> {
        self.lines.get(index).map(String::as_str)
    }

    pub fn in_flight_bytes(&self) -> usize {
        self.in_flight_bytes
    }

    pub fn progress(&self) -> JobProgress {
        JobProgress {
            lines_sent: self.next,
            lines_acked: self.acked,
            total: self.lines.len(),
        }
    }

    /// Every line was sent and answered. GRBL may still be moving.
    pub fn is_finished(&self) -> bool {
        self.acked == self.lines.len()
    }
}

/// A job line that passed the `Streamer`'s checks. Only the streamer can
/// make one, so only checked lines can be written as job lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamedLine(String);

impl StreamedLine {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_blank_or_comment(line: &str) -> bool {
    line.is_empty() || line.starts_with(';') || (line.starts_with('(') && line.ends_with(')'))
}

/// GRBL acts on `?`, `!`, `~`, 0x18 and bytes above 0x7F the moment they
/// arrive, even inside comments (grbl `serial.c`), and a newline would split
/// the line, so only printable ASCII without those may be streamed.
fn is_streamable_char(c: char) -> bool {
    c == ' ' || (c.is_ascii_graphic() && !matches!(c, '?' | '!' | '~'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAP: usize = 127;

    fn streamer(lines: &[&str], mode: StreamMode) -> Streamer {
        let lines = lines.iter().map(|l| l.to_string()).collect();
        Streamer::new(lines, mode, CAP).unwrap()
    }

    fn line_of(len_with_newline: usize) -> String {
        "G".repeat(len_with_newline - 1)
    }

    #[test]
    fn stream_capacity_comes_from_the_build_options() {
        assert_eq!(rx_capacity(None), 127);
        let options = BuildOptions {
            codes: "V".into(),
            planner_blocks: 15,
            rx_buffer_bytes: 256,
        };
        assert_eq!(rx_capacity(Some(&options)), 255);
    }

    #[test]
    fn stream_fills_the_buffer_exactly_and_no_further() {
        // 60 + 67 = 127 bytes fit; one more byte does not.
        let lines = [line_of(60), line_of(67), line_of(1 + 1)];
        let mut s = Streamer::new(lines.to_vec(), StreamMode::CharacterCounting, CAP).unwrap();
        assert_eq!(s.ready_lines().len(), 2);
        assert_eq!(s.in_flight_bytes(), 127);
        assert!(s.ready_lines().is_empty());
        assert_eq!(s.on_ack(), Ok(0));
        assert_eq!(s.ready_lines(), vec!["G"]);
        assert_eq!(s.in_flight_bytes(), 69);

        let lines = [line_of(60), line_of(68)];
        let mut s = Streamer::new(lines.to_vec(), StreamMode::CharacterCounting, CAP).unwrap();
        assert_eq!(s.ready_lines().len(), 1);
    }

    #[test]
    fn stream_line_length_limit_counts_the_newline() {
        let ok = Streamer::new(vec![line_of(127)], StreamMode::CharacterCounting, CAP);
        assert!(ok.is_ok());
        let too_long = Streamer::new(
            vec!["G0".into(), line_of(128)],
            StreamMode::CharacterCounting,
            CAP,
        );
        assert_eq!(
            too_long.unwrap_err(),
            StreamError::LineTooLong {
                index: 1,
                len: 128,
                max: 127
            }
        );
    }

    #[test]
    fn stream_rejects_realtime_and_non_ascii_characters() {
        for (line, ch) in [
            ("G1 X1 ; why?", '?'),
            ("M3 S1000 !", '!'),
            ("G0 ~", '~'),
            ("G0 X1\u{18}", '\u{18}'),
            ("G0 X1\tY2", '\t'),
            ("(café)G0", 'é'),
        ] {
            let result = Streamer::new(vec![line.into()], StreamMode::CharacterCounting, CAP);
            assert_eq!(
                result.unwrap_err(),
                StreamError::UnsafeChar { index: 0, ch },
                "{line:?}"
            );
        }
    }

    #[test]
    fn stream_drops_blank_and_comment_lines_and_trims() {
        let mut s = streamer(
            &[
                "; PhotonForge job",
                "",
                "  G21 G90  ",
                "(header)",
                "G0 X1 (inline) Y2",
            ],
            StreamMode::CharacterCounting,
        );
        assert_eq!(s.progress().total, 2);
        assert_eq!(s.ready_lines(), vec!["G21 G90", "G0 X1 (inline) Y2"]);
    }

    #[test]
    fn stream_send_response_keeps_one_line_in_flight() {
        let mut s = streamer(&["G0 X1", "G1 X2", "M5"], StreamMode::SendResponse);
        assert_eq!(s.ready_lines(), vec!["G0 X1"]);
        assert!(s.ready_lines().is_empty());
        s.on_ack().unwrap();
        assert_eq!(s.ready_lines(), vec!["G1 X2"]);
    }

    #[test]
    fn stream_errors_free_their_line_and_report_its_index() {
        let mut s = streamer(
            &["G0 X1", "G1 X2 F100", "M5"],
            StreamMode::CharacterCounting,
        );
        assert_eq!(s.ready_lines().len(), 3);
        assert_eq!(s.on_ack(), Ok(0));
        assert_eq!(s.on_error(), Ok(1));
        assert_eq!(s.line(1), Some("G1 X2 F100"));
        assert_eq!(s.on_ack(), Ok(2));
        assert!(s.is_finished());
        assert_eq!(s.on_ack(), Err(StreamError::UnexpectedAck));
        assert_eq!(s.on_error(), Err(StreamError::UnexpectedAck));
    }

    #[test]
    fn stream_progress_and_end() {
        let mut s = streamer(&["G0 X1", "M5"], StreamMode::CharacterCounting);
        assert!(!s.is_finished());
        s.ready_lines();
        s.on_ack().unwrap();
        assert_eq!(
            s.progress(),
            JobProgress {
                lines_sent: 2,
                lines_acked: 1,
                total: 2
            }
        );
        s.on_ack().unwrap();
        assert!(s.is_finished());

        let mut empty = streamer(&["; nothing"], StreamMode::CharacterCounting);
        assert!(empty.is_finished());
        assert!(empty.ready_lines().is_empty());
    }

    /// Deterministic pseudo-random numbers (xorshift64), so failures repeat.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// Random line lengths and random answer timing: the bytes in flight
    /// never exceed the capacity, lines go out in order, every line is
    /// answered exactly once, and the stream always finishes.
    #[test]
    fn stream_never_overfills_under_random_timing() {
        for seed in 1..=50u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let lines: Vec<String> = (0..300).map(|_| line_of(2 + rng.below(126))).collect();
            let mut s = Streamer::new(lines.clone(), StreamMode::CharacterCounting, CAP).unwrap();
            let mut sent = Vec::new();
            let mut answered = 0;
            while !s.is_finished() {
                for line in s.ready_lines() {
                    sent.push(line.to_owned());
                }
                assert!(s.in_flight_bytes() <= CAP, "seed {seed}");
                let waiting = s.progress().lines_sent - s.progress().lines_acked;
                for _ in 0..rng.below(waiting + 1) {
                    let index = if rng.below(10) == 0 {
                        s.on_error()
                    } else {
                        s.on_ack()
                    };
                    assert_eq!(index, Ok(answered), "seed {seed}");
                    answered += 1;
                }
                // Any line fits an empty buffer, so nothing in flight
                // means everything was sent.
                if waiting == 0 {
                    assert_eq!(s.progress().lines_sent, lines.len(), "seed {seed}");
                }
            }
            assert_eq!(sent, lines, "seed {seed}");
            assert_eq!(answered, lines.len());
        }
    }
}
