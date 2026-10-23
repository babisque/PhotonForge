# Project Spec: Phase 5: GRBL Connection, Read-Only (`pf-device`)

## 1. Overview and Goal
* **Purpose:** Connect to a GRBL controller over serial, show live status, and read settings,
  **without sending any command that moves the machine or turns on the laser**.
* **Success Criteria:**
  * Lists serial ports, connects at a selected baud rate (default 115200), and detects the `Grbl 1.1x` banner.
  * Parses status reports, `ok`, `error:N`, `ALARM:N`, `$$` settings, `[MSG:…]`, and `$I` build info. Each has unit tests.
  * The GUI shows the connection state, machine state, MPos/WPos, and feed/speed, plus a console log.
  * The only commands the code can send are `?`, `$$`, `$I`, and `$G`. Other bytes are impossible by construction (see section 5).

## 2. Tech Stack and Architecture
* **Crate:** `pf-device`. Dependencies: `pf-core`, `thiserror = "2.0"`, `serialport = "4.10"` (⚠️ approve).
* **Reference:** GRBL v1.1 `doc/markdown/interface.md` (status report format, error/alarm tables).
* **Threading:** one I/O thread per connection owns the port. It talks to the GUI through `std::sync::mpsc` channels (`DeviceEvent` out, `ReadOnlyRequest` in).
* **Transport boundary:** a small `Transport` trait (`read`, `write`) with `SerialTransport` and `MockTransport`. It is the one allowed abstraction, and exists for testability.
* **Status report example (GRBL 1.1):** `<Idle|MPos:0.000,0.000,0.000|FS:0,0|WCO:0.000,0.000,0.000>`

## 3. Executable Commands
* **Test:** `cargo test -p pf-device`
* **Parser tests only:** `cargo test -p pf-device parse`
* **Manual hardware check:** `cargo run -p pf-app`, then Connect. Connecting is read-only, so it is safe.

## 4. Project Structure
* `src/lib.rs`: public API.
* `src/transport.rs`: the `Transport` trait, `SerialTransport`, `MockTransport`, and `list_ports()`.
* `src/protocol/parse.rs`: line classifier `parse_line(&str) -> Response`.
* `src/protocol/status.rs`: `StatusReport { state, mpos, wpos, wco, feed, spindle, … }`.
* `src/protocol/settings.rs`: `$$` → `GrblSettings` (typed `$30`, `$31`, `$32`, `$130`, `$131`, …).
* `src/protocol/codes.rs`: error and alarm code → message tables, cited from the GRBL docs.
* `src/protocol/readonly.rs`: the `ReadOnlyRequest` enum and its byte encoding.
* `src/connection.rs`: the I/O thread, line reassembly, banner detection, status polling.
* `crates/pf-app/src/panels/connection.rs`, `panels/console.rs`: GUI panels.

## 5. Code Style and Conventions
* Parsers are pure `&str → Result<T, ParseError>` functions with table-driven tests that use real GRBL output samples.
* Read-only by construction: the connection's public API only accepts `ReadOnlyRequest`, and there is no raw `send(&str)` in this phase.
  ```rust
  /// The only commands Phase 5 can send. None of them move the machine or fire the laser.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum ReadOnlyRequest {
      StatusQuery,   // `?` (realtime, no newline)
      ViewSettings,  // `$$\n`
      BuildInfo,     // `$I\n`
      ParserState,   // `$G\n`
  }
  ```
* Status polling runs at 5 Hz while connected (configurable from 1 to 10 Hz).

## 6. Boundaries and Guardrails
* **Always do:** Cite the GRBL doc section for each parsed field and code table. Test parsers against captured real output. Handle disconnects as events, not panics.
* **Ask first:** Before adding any request variant, before supporting GRBL forks (grblHAL, FluidNC) beyond what parses for free, and before auto-connect behavior.
* **Never do:** Never send `$X`, `$H`, `$J=`, G-code, `M3`/`M4`, `~`, or any write to `$` settings in this phase. Never expose a raw-write API. Never auto-reset the controller on connect (DTR handling must be documented).

## 7. Implementation Phases
* **5.1 Parsers:** `parse_line`, status reports, `ok`/`error`/`ALARM`, `[MSG]`, with code tables. Pure, with tests.
* **5.2 Settings and build info:** `$$` → `GrblSettings`, `$I` parsing, with tests.
* **5.3 Transport:** the `Transport` trait, `MockTransport`, `SerialTransport`, `list_ports`.
* **5.4 Connection thread:** line reassembly, banner detection, polling, events. Tested with `MockTransport` scripts.
* **5.5 GUI:** connection panel (port, baud, connect), status display, console log.
* **5.6 Hardware smoke test:** connect to a real board, then verify the status and `$$` display. Record the board and firmware version in the PR.
