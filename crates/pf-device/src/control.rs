//! Requests that act on the machine, sent with `Connection::control`.

use pf_core::MachineProfile;

use crate::job::JobKind;
use crate::jog::JogMove;
use crate::stream::StreamMode;

/// A job to stream. The connection scans the lines and runs the pre-flight
/// checks again against the latest controller state before sending any.
#[derive(Debug, Clone, PartialEq)]
pub struct JobRequest {
    pub kind: JobKind,
    pub lines: Vec<String>,
    pub profile: MachineProfile,
    pub mode: StreamMode,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ControlRequest {
    Start(JobRequest),
    /// Pause: feed hold. The job counts as held once GRBL reports the hold
    /// complete with the laser output off.
    Hold,
    /// Cycle start, only from a completed hold.
    Resume,
    /// Feed hold, soft reset, then `M5`. Accepted in every state.
    Stop,
    /// Clears a fault or a finished job back to idle.
    AcknowledgeFault,
    /// `$X`. Send it only after the user confirmed: GRBL then allows motion
    /// without knowing where the head is. Ends a stop sequence that waits
    /// for an unlock.
    Unlock,
    /// `$H`, when homing is enabled (`$22=1`). Also clears an alarm.
    Home,
    /// A relative move, clamped to the profile's work area. One at a time.
    Jog {
        step: JogMove,
        profile: MachineProfile,
    },
    /// Stops a jog at once and drops queued jog moves.
    JogCancel,
}

impl ControlRequest {
    pub fn label(&self) -> &'static str {
        match self {
            ControlRequest::Start(job) => match job.kind {
                JobKind::Job => "Start",
                JobKind::Frame => "Frame",
            },
            ControlRequest::Hold => "Pause",
            ControlRequest::Resume => "Resume",
            ControlRequest::Stop => "Stop",
            ControlRequest::AcknowledgeFault => "Acknowledge",
            ControlRequest::Unlock => "Unlock",
            ControlRequest::Home => "Home",
            ControlRequest::Jog { .. } => "Jog",
            ControlRequest::JogCancel => "Jog cancel",
        }
    }
}
