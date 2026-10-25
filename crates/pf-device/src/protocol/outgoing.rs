//! Everything this crate can write to the controller, as a closed set. The
//! bytes of each command are defined here and nowhere else.

use super::readonly::ReadOnlyRequest;
use crate::jog::JogLine;
use crate::stream::StreamedLine;

/// Real-time commands: GRBL acts on them the moment they arrive, without an
/// `ok`, and they take no space in its receive buffer (GRBL v1.1
/// `interface.md`, "Real-time Commands").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Realtime {
    FeedHold,   // `!`
    CycleStart, // `~`
    SoftReset,  // 0x18 (Ctrl-X)
    JogCancel,  // 0x85
}

impl Realtime {
    pub fn byte(self) -> u8 {
        match self {
            Realtime::FeedHold => b'!',
            Realtime::CycleStart => b'~',
            Realtime::SoftReset => 0x18,
            Realtime::JogCancel => 0x85,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Realtime::FeedHold => "! (feed hold)",
            Realtime::CycleStart => "~ (cycle start)",
            Realtime::SoftReset => "Ctrl-X (soft reset)",
            Realtime::JogCancel => "0x85 (jog cancel)",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outgoing {
    ReadOnly(ReadOnlyRequest),
    Realtime(Realtime),
    /// `M5`: laser off, the last step of the stop sequence.
    LaserOff,
    /// `$X`: clears an alarm lock. Only on the user's confirmation.
    Unlock,
    /// `$H`: runs the homing cycle.
    Home,
    /// `$J=…`, clamped to the work area by `plan_jog`.
    Jog(JogLine),
    /// A job line, checked by the `Streamer`.
    JobLine(StreamedLine),
}

impl Outgoing {
    pub(crate) fn bytes(&self) -> Vec<u8> {
        match self {
            Outgoing::ReadOnly(request) => request.bytes().to_vec(),
            Outgoing::Realtime(command) => vec![command.byte()],
            Outgoing::LaserOff => b"M5\n".to_vec(),
            Outgoing::Unlock => b"$X\n".to_vec(),
            Outgoing::Home => b"$H\n".to_vec(),
            Outgoing::Jog(line) => {
                let mut bytes = line.as_str().as_bytes().to_vec();
                bytes.push(b'\n');
                bytes
            }
            Outgoing::JobLine(line) => {
                let mut bytes = line.as_str().as_bytes().to_vec();
                bytes.push(b'\n');
                bytes
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outgoing_bytes_are_fixed() {
        assert_eq!(Outgoing::Realtime(Realtime::FeedHold).bytes(), b"!");
        assert_eq!(Outgoing::Realtime(Realtime::CycleStart).bytes(), b"~");
        assert_eq!(Outgoing::Realtime(Realtime::SoftReset).bytes(), [0x18]);
        assert_eq!(Outgoing::Realtime(Realtime::JogCancel).bytes(), [0x85]);
        assert_eq!(Outgoing::LaserOff.bytes(), b"M5\n");
        assert_eq!(Outgoing::Unlock.bytes(), b"$X\n");
        assert_eq!(Outgoing::Home.bytes(), b"$H\n");
        assert_eq!(
            Outgoing::ReadOnly(ReadOnlyRequest::ViewSettings).bytes(),
            b"$$\n"
        );
    }
}
