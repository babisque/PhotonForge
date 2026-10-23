//! The only commands Phase 5 can send. None of them move the machine or
//! fire the laser: `?` asks for a status report and the others print
//! settings, build info and parser state.

use super::status::MachineState;

/// The only commands Phase 5 can send. None of them move the machine or fire the laser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOnlyRequest {
    StatusQuery,  // `?` (realtime, no newline)
    ViewSettings, // `$$\n`
    BuildInfo,    // `$I\n`
    ParserState,  // `$G\n`
}

impl ReadOnlyRequest {
    pub const ALL: [ReadOnlyRequest; 4] = [
        ReadOnlyRequest::StatusQuery,
        ReadOnlyRequest::ViewSettings,
        ReadOnlyRequest::BuildInfo,
        ReadOnlyRequest::ParserState,
    ];

    /// The exact bytes written to the controller. This is the only place
    /// outgoing bytes are defined.
    pub fn bytes(self) -> &'static [u8] {
        match self {
            ReadOnlyRequest::StatusQuery => b"?",
            ReadOnlyRequest::ViewSettings => b"$$\n",
            ReadOnlyRequest::BuildInfo => b"$I\n",
            ReadOnlyRequest::ParserState => b"$G\n",
        }
    }

    /// `?` is a realtime command: GRBL handles it on arrival, with no `ok`.
    /// The others are line commands, answered with `ok` or `error:N`.
    pub fn is_realtime(self) -> bool {
        self == ReadOnlyRequest::StatusQuery
    }

    pub fn label(self) -> &'static str {
        match self {
            ReadOnlyRequest::StatusQuery => "?",
            ReadOnlyRequest::ViewSettings => "$$",
            ReadOnlyRequest::BuildInfo => "$I",
            ReadOnlyRequest::ParserState => "$G",
        }
    }

    /// Whether GRBL v1.1h executes this request in `state` (`None`: not
    /// known yet). From `grbl/system.c`, `system_execute_line`: `$$` is
    /// refused with `error:8` during a cycle or hold, `$I` is accepted only
    /// in IDLE or ALARM, and `$G` always. No line is processed while homing.
    pub fn accepted_in(self, state: Option<MachineState>) -> bool {
        let Some(state) = state else {
            return true;
        };
        match self {
            ReadOnlyRequest::StatusQuery => true,
            ReadOnlyRequest::ParserState => state != MachineState::Home,
            ReadOnlyRequest::ViewSettings => !matches!(
                state,
                MachineState::Run | MachineState::Hold(_) | MachineState::Home
            ),
            ReadOnlyRequest::BuildInfo => {
                matches!(state, MachineState::Idle | MachineState::Alarm)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_requests_encode_to_fixed_bytes() {
        let encoded: Vec<&[u8]> = ReadOnlyRequest::ALL.iter().map(|r| r.bytes()).collect();
        assert_eq!(encoded, vec![b"?".as_slice(), b"$$\n", b"$I\n", b"$G\n"]);
        let realtime: Vec<bool> = ReadOnlyRequest::ALL
            .iter()
            .map(|r| r.is_realtime())
            .collect();
        assert_eq!(realtime, vec![true, false, false, false]);
    }

    #[test]
    fn acceptance_follows_grbl_system_c() {
        use MachineState::*;
        use ReadOnlyRequest::*;
        let states = [
            Idle,
            Run,
            Hold(Some(0)),
            Jog,
            Alarm,
            Door(Some(1)),
            Check,
            Home,
            Sleep,
        ];
        let accepted = |request: ReadOnlyRequest| -> Vec<bool> {
            states
                .iter()
                .map(|s| request.accepted_in(Some(*s)))
                .collect()
        };
        //                       Idle   Run    Hold   Jog    Alarm  Door   Check  Home   Sleep
        assert_eq!(accepted(StatusQuery), [true; 9]);
        assert_eq!(
            accepted(ParserState),
            [true, true, true, true, true, true, true, false, true]
        );
        assert_eq!(
            accepted(ViewSettings),
            [true, false, false, true, true, true, true, false, true]
        );
        assert_eq!(
            accepted(BuildInfo),
            [true, false, false, false, true, false, false, false, false]
        );
        for request in ReadOnlyRequest::ALL {
            assert!(request.accepted_in(None));
        }
    }
}
