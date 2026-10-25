//! The state of a job on the machine, as a pure transition function.
//!
//! ```text
//! Idle ──Start──► Running ──HoldConfirmed──► Held ──Resumed──► Running
//!   ▲               │  │                        │
//!   └── Finished ───┘  └── error/alarm/reset/disconnect ──► Faulted
//! ```
//!
//! Stop is accepted in every state and runs the stop sequence (`Stopping`).
//! A fault can only be left by acknowledging it, which leads to `Idle`:
//! there is no way back to `Running`, so nothing ever resumes on its own.

use thiserror::Error;

/// What is being streamed. Both go through the same pre-flight checks;
/// a frame has a lower power limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Job,
    Frame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JobProgress {
    pub lines_sent: usize,
    pub lines_acked: usize,
    pub total: usize,
}

/// Steps of the stop sequence, in order: `!`, `0x18`, `$X` if the user
/// confirms, `M5`. `AwaitingUnlock` is skipped when GRBL comes back idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StopStage {
    Holding,
    Resetting,
    AwaitingUnlock,
    LaserOff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaultReason {
    /// GRBL answered `error:code` to the line at this index.
    CommandError {
        line: usize,
        code: u8,
    },
    Alarm(u8),
    /// GRBL printed its welcome message without being asked to reset.
    ControllerReset,
    Disconnected(String),
    /// The stop sequence could not finish, e.g. no banner after `0x18`.
    StopFailed(String),
    /// GRBL reported the laser output on while paused.
    LaserOnDuringHold,
    /// A pause could not be confirmed with the laser off in time.
    HoldNotConfirmed,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum JobState {
    #[default]
    Idle,
    Running,
    Held,
    /// `fault` is set when the stop was caused by a fault; the sequence
    /// then ends in `Faulted` instead of `Idle`.
    Stopping {
        stage: StopStage,
        fault: Option<FaultReason>,
    },
    Faulted(FaultReason),
    Done,
}

impl JobState {
    /// A job owns the connection: no other line command may be sent.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            JobState::Running | JobState::Held | JobState::Stopping { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    Start,
    /// GRBL reports `Hold:0` after a feed hold.
    HoldConfirmed,
    /// GRBL left the hold after a cycle resume.
    Resumed,
    /// Every line was acknowledged and GRBL is idle.
    Finished,
    Error {
        line: usize,
        code: u8,
    },
    Alarm(u8),
    ControllerReset,
    Disconnected(String),
    StopRequested,
    StopAdvanced(StopStage),
    StopFinished,
    /// The stop sequence gave up; the reason becomes the fault.
    StopFailed(String),
    /// Something unsafe was detected: stop the job and fault with `reason`.
    Abort(FaultReason),
    Acknowledge,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{event:?} is not allowed while the job is {state:?}")]
pub struct InvalidTransition {
    pub state: JobState,
    pub event: JobEvent,
}

/// The state after `event`. Controller events (errors, alarms, resets)
/// outside a job leave the state unchanged; requests that make no sense in
/// the current state are rejected.
pub fn transition(state: &JobState, event: JobEvent) -> Result<JobState, InvalidTransition> {
    use JobState as S;
    let next = match (state, &event) {
        (S::Idle | S::Done, JobEvent::Start) => S::Running,
        (S::Running, JobEvent::HoldConfirmed) => S::Held,
        (S::Held, JobEvent::Resumed) => S::Running,
        (S::Running, JobEvent::Finished) => S::Done,

        // An error mid-job may have skipped a move, so the job is stopped.
        (S::Running | S::Held, JobEvent::Error { line, code }) => S::Stopping {
            stage: StopStage::Holding,
            fault: Some(FaultReason::CommandError {
                line: *line,
                code: *code,
            }),
        },
        (S::Running | S::Held, JobEvent::Abort(reason)) => S::Stopping {
            stage: StopStage::Holding,
            fault: Some(reason.clone()),
        },
        // GRBL resets itself on an alarm, which stops motion and the laser.
        (S::Running | S::Held, JobEvent::Alarm(code)) => S::Faulted(FaultReason::Alarm(*code)),
        (S::Running | S::Held, JobEvent::ControllerReset) => {
            S::Faulted(FaultReason::ControllerReset)
        }
        // The stop sequence resets GRBL itself, so the banner is expected there.
        (S::Stopping { stage, .. }, JobEvent::ControllerReset) if *stage < StopStage::Resetting => {
            S::Faulted(FaultReason::ControllerReset)
        }
        (S::Running | S::Held | S::Stopping { .. }, JobEvent::Disconnected(reason)) => {
            S::Faulted(FaultReason::Disconnected(reason.clone()))
        }

        (S::Faulted(reason), JobEvent::StopRequested) => S::Stopping {
            stage: StopStage::Holding,
            fault: Some(reason.clone()),
        },
        (S::Stopping { .. }, JobEvent::StopRequested) => state.clone(),
        (_, JobEvent::StopRequested) => S::Stopping {
            stage: StopStage::Holding,
            fault: None,
        },
        (S::Stopping { stage, fault }, JobEvent::StopAdvanced(next)) if next > stage => {
            S::Stopping {
                stage: *next,
                fault: fault.clone(),
            }
        }
        (S::Stopping { fault, .. }, JobEvent::StopFinished) => match fault {
            Some(reason) => S::Faulted(reason.clone()),
            None => S::Idle,
        },
        (S::Stopping { .. }, JobEvent::StopFailed(reason)) => {
            S::Faulted(FaultReason::StopFailed(reason.clone()))
        }

        (S::Faulted(_) | S::Done, JobEvent::Acknowledge) => S::Idle,

        // Not part of a running job: the state stays as it is.
        (
            S::Idle | S::Done | S::Faulted(_) | S::Stopping { .. },
            JobEvent::Error { .. }
            | JobEvent::Alarm(_)
            | JobEvent::ControllerReset
            | JobEvent::Disconnected(_),
        ) => state.clone(),

        _ => {
            return Err(InvalidTransition {
                state: state.clone(),
                event,
            });
        }
    };
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stopping(stage: StopStage, fault: Option<FaultReason>) -> JobState {
        JobState::Stopping { stage, fault }
    }

    fn all_states() -> Vec<JobState> {
        vec![
            JobState::Idle,
            JobState::Running,
            JobState::Held,
            stopping(StopStage::Holding, None),
            stopping(StopStage::Resetting, None),
            stopping(StopStage::AwaitingUnlock, Some(FaultReason::Alarm(3))),
            stopping(StopStage::LaserOff, None),
            JobState::Faulted(FaultReason::ControllerReset),
            JobState::Done,
        ]
    }

    fn all_events() -> Vec<JobEvent> {
        vec![
            JobEvent::Start,
            JobEvent::HoldConfirmed,
            JobEvent::Resumed,
            JobEvent::Finished,
            JobEvent::Error { line: 7, code: 20 },
            JobEvent::Alarm(1),
            JobEvent::ControllerReset,
            JobEvent::Disconnected("unplugged".into()),
            JobEvent::StopRequested,
            JobEvent::StopAdvanced(StopStage::Resetting),
            JobEvent::StopFinished,
            JobEvent::StopFailed("no banner".into()),
            JobEvent::Abort(FaultReason::LaserOnDuringHold),
            JobEvent::Acknowledge,
        ]
    }

    #[test]
    fn a_job_runs_pauses_resumes_and_finishes() {
        let mut s = JobState::Idle;
        for event in [
            JobEvent::Start,
            JobEvent::HoldConfirmed,
            JobEvent::Resumed,
            JobEvent::Finished,
            JobEvent::Acknowledge,
        ] {
            s = transition(&s, event).unwrap();
        }
        assert_eq!(s, JobState::Idle);
        assert_eq!(
            transition(&JobState::Done, JobEvent::Start),
            Ok(JobState::Running)
        );
    }

    #[test]
    fn stop_walks_the_stages_in_order_and_ends_idle() {
        let mut s = transition(&JobState::Running, JobEvent::StopRequested).unwrap();
        assert_eq!(s, stopping(StopStage::Holding, None));
        for stage in [
            StopStage::Resetting,
            StopStage::AwaitingUnlock,
            StopStage::LaserOff,
        ] {
            s = transition(&s, JobEvent::StopAdvanced(stage)).unwrap();
        }
        // No going back to an earlier stage.
        assert!(transition(&s, JobEvent::StopAdvanced(StopStage::Resetting)).is_err());
        assert_eq!(transition(&s, JobEvent::StopFinished), Ok(JobState::Idle));
        // Unlock is skipped when GRBL comes back idle.
        let s = stopping(StopStage::Resetting, None);
        assert!(transition(&s, JobEvent::StopAdvanced(StopStage::LaserOff)).is_ok());
    }

    #[test]
    fn an_error_mid_job_stops_and_ends_faulted() {
        let s = transition(&JobState::Running, JobEvent::Error { line: 3, code: 22 }).unwrap();
        let fault = FaultReason::CommandError { line: 3, code: 22 };
        assert_eq!(s, stopping(StopStage::Holding, Some(fault.clone())));
        let s = transition(&s, JobEvent::StopAdvanced(StopStage::LaserOff)).unwrap();
        assert_eq!(
            transition(&s, JobEvent::StopFinished),
            Ok(JobState::Faulted(fault))
        );
    }

    #[test]
    fn an_abort_stops_the_job_with_its_reason() {
        for from in [JobState::Running, JobState::Held] {
            assert_eq!(
                transition(&from, JobEvent::Abort(FaultReason::HoldNotConfirmed)),
                Ok(stopping(
                    StopStage::Holding,
                    Some(FaultReason::HoldNotConfirmed)
                ))
            );
        }
        assert!(
            transition(
                &JobState::Idle,
                JobEvent::Abort(FaultReason::HoldNotConfirmed)
            )
            .is_err()
        );
    }

    #[test]
    fn alarms_resets_and_disconnects_fault_the_job() {
        for from in [JobState::Running, JobState::Held] {
            assert_eq!(
                transition(&from, JobEvent::Alarm(1)),
                Ok(JobState::Faulted(FaultReason::Alarm(1)))
            );
            assert_eq!(
                transition(&from, JobEvent::ControllerReset),
                Ok(JobState::Faulted(FaultReason::ControllerReset))
            );
            assert_eq!(
                transition(&from, JobEvent::Disconnected("gone".into())),
                Ok(JobState::Faulted(FaultReason::Disconnected("gone".into())))
            );
        }
        // The banner the stop sequence asked for is not a fault.
        let s = stopping(StopStage::Resetting, None);
        assert_eq!(transition(&s, JobEvent::ControllerReset), Ok(s.clone()));
        let s = stopping(StopStage::Holding, None);
        assert_eq!(
            transition(&s, JobEvent::ControllerReset),
            Ok(JobState::Faulted(FaultReason::ControllerReset))
        );
        assert_eq!(
            transition(&s, JobEvent::StopFailed("no banner".into())),
            Ok(JobState::Faulted(FaultReason::StopFailed(
                "no banner".into()
            )))
        );
    }

    #[test]
    fn stop_is_accepted_in_every_state() {
        for state in all_states() {
            let next = transition(&state, JobEvent::StopRequested).unwrap();
            assert!(
                matches!(next, JobState::Stopping { .. }),
                "{state:?} -> {next:?}"
            );
        }
        // Stopping a faulted job keeps the fault.
        let faulted = JobState::Faulted(FaultReason::Alarm(2));
        assert_eq!(
            transition(&faulted, JobEvent::StopRequested),
            Ok(stopping(StopStage::Holding, Some(FaultReason::Alarm(2))))
        );
    }

    /// Every (state, event) pair: a fault never leads anywhere but `Idle`
    /// (by acknowledging) or `Stopping` (by stopping), so a faulted job can
    /// never be resumed or restarted directly.
    #[test]
    fn nothing_leaves_a_fault_except_acknowledge_or_stop() {
        for state in all_states() {
            for event in all_events() {
                let Ok(next) = transition(&state, event.clone()) else {
                    continue;
                };
                if let JobState::Faulted(_) = state {
                    let allowed = match event {
                        JobEvent::Acknowledge => next == JobState::Idle,
                        JobEvent::StopRequested => matches!(next, JobState::Stopping { .. }),
                        _ => next == state,
                    };
                    assert!(allowed, "{state:?} + {event:?} -> {next:?}");
                }
                if next == JobState::Running {
                    assert!(
                        matches!(
                            (&state, &event),
                            (JobState::Idle | JobState::Done, JobEvent::Start)
                                | (JobState::Held, JobEvent::Resumed)
                        ),
                        "{state:?} + {event:?} -> Running"
                    );
                }
            }
        }
    }

    #[test]
    fn requests_out_of_place_are_rejected() {
        let rejected = [
            (JobState::Running, JobEvent::Start),
            (JobState::Held, JobEvent::Start),
            (
                JobState::Faulted(FaultReason::ControllerReset),
                JobEvent::Start,
            ),
            (JobState::Idle, JobEvent::HoldConfirmed),
            (JobState::Running, JobEvent::Resumed),
            (JobState::Held, JobEvent::Finished),
            (JobState::Idle, JobEvent::StopFinished),
            (JobState::Running, JobEvent::Acknowledge),
            (JobState::Idle, JobEvent::StopAdvanced(StopStage::Resetting)),
        ];
        for (state, event) in rejected {
            assert!(
                transition(&state, event.clone()).is_err(),
                "{state:?} + {event:?}"
            );
        }
    }

    #[test]
    fn controller_events_outside_a_job_change_nothing() {
        for state in [
            JobState::Idle,
            JobState::Done,
            JobState::Faulted(FaultReason::Alarm(9)),
        ] {
            for event in [
                JobEvent::Error { line: 0, code: 9 },
                JobEvent::Alarm(2),
                JobEvent::ControllerReset,
                JobEvent::Disconnected("x".into()),
            ] {
                assert_eq!(transition(&state, event), Ok(state.clone()));
            }
        }
        assert!(!JobState::Idle.is_active());
        assert!(JobState::Held.is_active());
        assert!(stopping(StopStage::LaserOff, None).is_active());
    }
}
