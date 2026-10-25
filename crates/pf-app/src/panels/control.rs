use std::time::{Duration, Instant};

use pf_device::{ControlRequest, FaultReason, JobKind, JobState, alarm_info, error_info};

use crate::actions;
use crate::panels::console::describe_alarm;
use crate::panels::preview::format_duration;
use crate::state::AppState;

/// Requested while drawing, run afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobAction {
    AskStart,
    ConfirmStart,
    CancelStart,
    Pause,
    Resume,
    Acknowledge,
}

/// The red Stop button. It is always enabled: stopping is accepted in every
/// state.
pub fn stop_button() -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new("■ Stop")
            .strong()
            .color(egui::Color32::WHITE),
    )
    .fill(egui::Color32::from_rgb(190, 30, 30))
}

/// Start, pause, resume and progress of the job, with a confirmation before
/// anything is sent. Stop is next to it and in the status bar.
pub fn show_job_controls(ui: &mut egui::Ui, state: &mut AppState) {
    actions::refresh_job(state);
    actions::refresh_preview(state);
    let plan = actions::job_plan(state);
    let job = state.device.snapshot.job.clone();
    let mut wanted = Vec::new();

    ui.horizontal(|ui| {
        ui.strong("Job");
        let (name, color) = job_badge(&job, ui.visuals());
        ui.label(egui::RichText::new(name).strong().color(color));
    });
    ui.horizontal(|ui| {
        let reasons = plan
            .as_ref()
            .err()
            .map(|r| r.join("\n"))
            .unwrap_or_default();
        if ui
            .add_enabled(plan.is_ok(), egui::Button::new("▶ Start"))
            .on_disabled_hover_text(reasons)
            .clicked()
        {
            wanted.push(JobAction::AskStart);
        }
        if ui
            .add_enabled(job == JobState::Running, egui::Button::new("⏸ Pause"))
            .clicked()
        {
            wanted.push(JobAction::Pause);
        }
        if ui
            .add_enabled(job == JobState::Held, egui::Button::new("⏵ Resume"))
            .clicked()
        {
            wanted.push(JobAction::Resume);
        }
    });

    if let Some(progress) = state.device.snapshot.job_progress
        && job != JobState::Idle
        && progress.total > 0
    {
        let fraction = progress.lines_acked as f32 / progress.total as f32;
        ui.add(egui::ProgressBar::new(fraction).text(format!(
            "{} / {} lines",
            progress.lines_acked, progress.total
        )));
        let elapsed = elapsed(
            state.device.job_started,
            state.device.job_ended,
            Instant::now(),
        );
        let mut times = format!("Elapsed {}", format_duration(elapsed.as_secs_f64()));
        let estimate = state
            .preview
            .result()
            .and_then(|r| r.as_ref().ok())
            .map(|p| p.estimate.seconds);
        if job.is_active()
            && state.device.running_kind == Some(JobKind::Job)
            && let Some(seconds) = estimate
        {
            let left = seconds * (1.0 - f64::from(fraction));
            times.push_str(&format!(" · about {} left", format_duration(left)));
        }
        ui.weak(times);
    }

    match &job {
        JobState::Faulted(reason) => {
            ui.colored_label(ui.visuals().error_fg_color, describe_fault(reason));
            if ui.button("Acknowledge").clicked() {
                wanted.push(JobAction::Acknowledge);
            }
        }
        JobState::Done if ui.button("Clear").clicked() => {
            wanted.push(JobAction::Acknowledge);
        }
        _ => {}
    }

    if state.device.confirm_start {
        confirm_dialog(ui, state, plan.as_ref().ok(), &mut wanted);
    }

    for action in wanted {
        match action {
            JobAction::AskStart => state.device.confirm_start = true,
            JobAction::ConfirmStart => {
                state.device.confirm_start = false;
                actions::start_job(state);
            }
            JobAction::CancelStart => state.device.confirm_start = false,
            JobAction::Pause => actions::device_control(state, ControlRequest::Hold),
            JobAction::Resume => actions::device_control(state, ControlRequest::Resume),
            JobAction::Acknowledge => {
                actions::device_control(state, ControlRequest::AcknowledgeFault);
            }
        }
    }
}

/// Starting fires the laser, so it is confirmed with what the job will do.
fn confirm_dialog(
    ui: &mut egui::Ui,
    state: &AppState,
    lines: Option<&Vec<String>>,
    wanted: &mut Vec<JobAction>,
) {
    let modal = egui::Modal::new(egui::Id::unique("confirm_start")).show(ui.ctx(), |ui| {
        ui.set_max_width(420.0);
        ui.heading("Start the job?");
        let Some(lines) = lines else {
            ui.label("The job can no longer start; close this and check the reasons.");
            if ui.button("Close").clicked() {
                wanted.push(JobAction::CancelStart);
            }
            return;
        };
        ui.label(format!("{} lines of G-code.", lines.len()));
        if let Some(Ok(preview)) = state.preview.result() {
            ui.label(format!(
                "Estimated time: {} (real jobs take longer).",
                format_duration(preview.estimate.seconds)
            ));
        }
        if let Ok(summary) = pf_device::scan_job(lines) {
            let max_s = state.profile.max_s_value();
            let percent = f64::from(summary.max_s) / f64::from(max_s) * 100.0;
            ui.label(format!(
                "Highest power: S{} ({percent:.0} % of S{max_s}).",
                summary.max_s
            ));
        }
        ui.label("Wear eye protection, close the enclosure and keep the Stop button in reach.");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Start").clicked() {
                wanted.push(JobAction::ConfirmStart);
            }
            if ui.button("Cancel").clicked() {
                wanted.push(JobAction::CancelStart);
            }
        });
    });
    if modal.should_close() {
        wanted.push(JobAction::CancelStart);
    }
}

fn elapsed(started: Option<Instant>, ended: Option<Instant>, now: Instant) -> Duration {
    match started {
        Some(start) => ended.unwrap_or(now).saturating_duration_since(start),
        None => Duration::ZERO,
    }
}

fn job_badge(job: &JobState, visuals: &egui::Visuals) -> (&'static str, egui::Color32) {
    let green = egui::Color32::from_rgb(40, 160, 70);
    let blue = egui::Color32::from_rgb(40, 110, 220);
    match job {
        JobState::Idle => ("Idle", visuals.weak_text_color()),
        JobState::Running => ("Running", blue),
        JobState::Held => ("Paused", visuals.warn_fg_color),
        JobState::Stopping { .. } => ("Stopping", visuals.warn_fg_color),
        JobState::Faulted(_) => ("Failed", visuals.error_fg_color),
        JobState::Done => ("Done", green),
    }
}

/// What went wrong, in words, with what to do next where it matters.
pub fn describe_fault(reason: &FaultReason) -> String {
    match reason {
        FaultReason::CommandError { line, code } => {
            let what = error_info(*code).map_or("unknown error", |i| i.description);
            format!(
                "GRBL refused line {} with error:{code} ({what}); the job was stopped.",
                line + 1
            )
        }
        FaultReason::Alarm(code) => match alarm_info(*code) {
            Some(_) => format!("{}. The job was stopped.", describe_alarm(*code)),
            None => format!("ALARM:{code}. The job was stopped."),
        },
        FaultReason::ControllerReset => {
            String::from("GRBL reset itself during the job (power or USB glitch?).")
        }
        FaultReason::Disconnected(reason) => format!("The connection was lost: {reason}"),
        FaultReason::StopFailed(reason) => format!("The stop did not complete: {reason}"),
        FaultReason::LaserOnDuringHold => String::from(
            "GRBL reported the laser still on while paused, so the job was stopped. \
             This GRBL build may lack DISABLE_LASER_DURING_HOLD.",
        ),
        FaultReason::HoldNotConfirmed => String::from(
            "The pause could not be confirmed with the laser off in time, so the job was stopped.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pf_device::{ConnectionState, DeviceEvent, JobProgress, StopStage};

    fn render(state: &mut AppState) {
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                show_job_controls(ui, state);
            });
            output.textures_delta.clear();
        }
    }

    #[test]
    fn renders_headless_in_every_job_state() {
        for job in [
            JobState::Idle,
            JobState::Running,
            JobState::Held,
            JobState::Stopping {
                stage: StopStage::Resetting,
                fault: None,
            },
            JobState::Faulted(FaultReason::Alarm(1)),
            JobState::Done,
        ] {
            let mut state = AppState::new();
            for event in [
                DeviceEvent::ConnectionChanged(ConnectionState::Opening),
                DeviceEvent::JobStateChanged(job.clone()),
                DeviceEvent::JobProgress(JobProgress {
                    lines_sent: 10,
                    lines_acked: 5,
                    total: 20,
                }),
            ] {
                state.device.snapshot.apply(&event);
            }
            render(&mut state);
            assert!(!state.device.confirm_start, "{job:?}");
            assert_eq!(state.status, None, "{job:?}");
        }
    }

    #[test]
    fn the_confirmation_sends_nothing_by_itself() {
        let mut state = AppState::new();
        state.device.confirm_start = true;
        render(&mut state);
        assert!(state.device.confirm_start);
        assert_eq!(state.device.running_kind, None);
        assert_eq!(state.status, None);
    }

    #[test]
    fn faults_are_described_in_words() {
        let text = describe_fault(&FaultReason::CommandError { line: 9, code: 20 });
        assert!(
            text.starts_with("GRBL refused line 10 with error:20 ("),
            "{text}"
        );
        assert!(text.ends_with("the job was stopped."), "{text}");
        assert!(describe_fault(&FaultReason::Alarm(1)).starts_with("ALARM:1 Hard limit"));
        assert!(describe_fault(&FaultReason::Alarm(99)).starts_with("ALARM:99"));
        assert!(
            describe_fault(&FaultReason::StopFailed("switch the machine off".into()))
                .contains("switch the machine off")
        );
        assert!(
            describe_fault(&FaultReason::LaserOnDuringHold).contains("DISABLE_LASER_DURING_HOLD")
        );
    }

    #[test]
    fn elapsed_time_stops_with_the_job() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(90);
        let t2 = t0 + Duration::from_secs(200);
        assert_eq!(elapsed(None, None, t1), Duration::ZERO);
        assert_eq!(elapsed(Some(t0), None, t1), Duration::from_secs(90));
        assert_eq!(elapsed(Some(t0), Some(t1), t2), Duration::from_secs(90));
    }
}
