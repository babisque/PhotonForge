# Project Spec: Phase 6: Machine Control and Job Streaming ⚠️ Hardware Gate

> **Every sub-phase in this spec changes code paths that move the machine or fire the laser.
> Each one is proposed (with a diff summary) and needs explicit approval before it is implemented.**

## 1. Overview and Goal
* **Purpose:** Run jobs on real hardware safely: streaming, pause/resume/stop, homing, unlock,
  jogging, and framing.
* **Success Criteria:**
  * **M3:** a simple SVG is cut on a real GRBL machine end to end.
  * Streaming never overflows GRBL's RX buffer. This is verified against a mock that enforces 128 bytes.
  * Stop always leaves the laser off (soft reset `0x18`, and `M5` after recovery).
  * A job cannot start unless every pre-flight check passes.
  * A lost connection mid-job shows an error state, and nothing resumes automatically.

## 2. Tech Stack and Architecture
* **Crate:** `pf-device` (extends Phase 5). No new dependencies.
* **References:** GRBL v1.1 `interface.md` (streaming protocols, realtime commands), `jogging.md`, `laser_mode.md`.
  The reference streaming algorithm is GRBL's `doc/script/stream.py` (character counting).
* **Streaming:** character counting. Track the bytes sent but not yet acknowledged, and send the next line only if
  `in_flight + line.len() + 1 <= RX_BUFFER_SIZE` (127 usable on the default 128-byte buffer). Each `ok`/`error` pops one line.
  Send-response (one line at a time) is a user-selectable fallback.
* **Realtime commands:** `?` status, `!` feed hold, `~` cycle resume, `0x18` soft reset, `0x85` jog cancel.
* **Job state machine:**
  ```
  Idle ──start(preflight ok)──► Running ──!──► Held ──~──► Running
    ▲                             │  │                        │
    └──── done / stopped ◄────────┘  └──error/alarm──► Faulted ◄┘
  ```

## 3. Executable Commands
* **Test:** `cargo test -p pf-device`
* **Streaming tests:** `cargo test -p pf-device stream`
* **Hardware test:** documented checklist in `docs/hardware-test.md`. Always run it with the laser at low power, eye protection on, and the enclosure closed.

## 4. Project Structure
* `src/stream.rs`: `Streamer` (pure, with character-counting bookkeeping) that takes a `Vec<String>` and emits sends and acks.
* `src/job.rs`: `JobState` enum, transitions, `JobProgress { lines_sent, lines_acked, total }`.
* `src/control.rs`: the `ControlRequest` enum: `Start`, `Hold`, `Resume`, `Stop`, `Unlock`, `Home`, `Jog`, `JogCancel`, `Frame`.
* `src/preflight.rs`: `preflight(status, settings, gcode_bounds, profile) -> Result<(), PreflightError>`.
* `src/frame.rs`: generates the frame toolpath (bounding box) through `pf-gcode` with the laser off by default.
* `crates/pf-app/src/panels/control.rs`, `panels/jog.rs`: GUI controls.
* `docs/hardware-test.md`: manual test checklist.

## 5. Code Style and Conventions
* The `Streamer` is pure and has no I/O, so every buffer edge case is unit-testable.
  ```rust
  pub struct Streamer {
      lines: Vec<String>,
      next: usize,
      in_flight: VecDeque<usize>, // byte lengths awaiting `ok`
      rx_capacity: usize,         // 127 usable bytes by default
  }

  impl Streamer {
      /// Lines that can be sent now without overflowing GRBL's RX buffer.
      pub fn ready_lines(&mut self) -> Vec<&str> { /* … */ }
      pub fn on_ack(&mut self) -> Result<(), StreamError> { /* pops in_flight */ }
  }
  ```
* **Pre-flight checks:** connected; state is `Idle`; no active alarm; `$32=1` (laser mode); the job bounds are inside the work area; the job's max `S` ≤ `$30` and ≤ the profile max.
* **Stop sequence:** `!` (hold) → `0x18` (soft reset) → wait for the banner → `$X` only if the user confirms → `M5`.
* **Frame:** laser off (`S0` / `M5`) by default. Low-power framing is only allowed up to `profile.frame_power_percent`, which defaults to 0.

## 6. Boundaries and Guardrails
* **Always do:** Propose each sub-phase and wait for approval. Test against `MockTransport` before any hardware test. Keep the Stop button visible and enabled whenever connected.
* **Ask first:** Every change in this phase. Also any change to the stop sequence, the pre-flight checks, frame power, or jog limits.
* **Never do:** Never auto-resume after disconnect, alarm, or error. Never skip pre-flight. Never send `$X` without user confirmation. Never raise frame power above the profile limit. Never write `$` settings to the controller in v1.

## 7. Implementation Phases (each one needs ⚠️ approval)
* **6.1 Streamer (pure):** character-counting and send-response bookkeeping, with exhaustive unit tests (buffer full, error mid-job, long lines).
* **6.2 Job state machine and pre-flight (pure):** transitions and checks, with tests.
* **6.3 Realtime control:** hold, resume, soft reset, and the stop sequence on the connection thread. Mock tests.
* **6.4 Unlock and homing:** `$X` (with confirmation) and `$H`, with GUI buttons.
* **6.5 Jogging:** `$J=G91 …` from the jog panel, with jog cancel on release. Distances are clamped to the work area.
* **6.6 Framing:** bounding-box trace with the laser off.
* **6.7 GUI job controls:** Start/Pause/Resume/Stop, progress, state display.
* **6.8 M3 hardware run:** follow `docs/hardware-test.md`, cut the sample, and record the results.
