# Hardware smoke test: GRBL read-only connection (Phase 5.6)

This checklist confirms PhotonForge's read-only GRBL connection on a real
controller. It needs about 15 minutes. Record the results in the PR.

Phase 5 can only send four commands: `?`, `$$`, `$I` and `$G`. None of them
moves the machine or fires the laser. The checklist still treats the machine
as live, as every laser test should.

## Before you start

- [ ] **Laser disabled:** the key switch is off, or the laser module/PSU is
      unplugged. The software cannot fire it, but test with the hardware off.
- [ ] Nothing is on the bed, and the gantry can move freely (a GRBL reset
      while moving can stop it abruptly; nothing here should move it).
- [ ] No other program has the serial port open (LightBurn, a terminal, a
      slicer). Serial ports open in only one program at a time.
- [ ] Build: `cargo run -p pf-app --release`.
- [ ] Open **View → Console** and untick **Hide status reports** for the
      first connection, so every byte sent is visible.

## 1. Connect with DTR off (default)

1. [ ] **Device** tab → **⟳**. The board's port is listed (Windows: `COMx`;
       Linux: `/dev/ttyUSB*` or `/dev/ttyACM*`; macOS: `/dev/cu.*`).
2. [ ] Baud 115200, status rate 5 Hz, **DTR unticked** → **Connect**.
3. [ ] Within 5 s the panel shows **Connected (Grbl 1.1x)**.
       - Linux and some Windows drivers reset the board on open: the console
         then shows `< Grbl 1.1h ['$' for help]`. That is expected.
       - Without a reset, the console shows `> $I` after about 2 s and the
         `[VER:…]` answer. That is the no-banner identification.
4. [ ] The console's `>` lines contain **only** `?`, `$$`, `$I` and `$G`.
5. [ ] Status shows a state (usually **Idle**, or **Alarm** when homing is
       enabled and the machine was not homed) and machine position X/Y/Z.
6. [ ] The magenta crosshair on the canvas sits at the head's machine position.
7. [ ] The settings table lists `$0`…`$132` with names. Compare a few values
       with what LightBurn or a terminal shows for `$$`.
8. [ ] Build info shows the version, build date, options and RX buffer size.
9. [ ] Any amber profile hints make sense (e.g. `$30` vs. the profile's max S,
       `$130`/`$131` vs. the work area, laser mode `$32`).

## 2. Requests in different states

1. [ ] Click **Read settings ($$)**, **Build info ($I)**, **Parser state ($G)**:
       each answers, and the console shows the request and an `ok`.
2. [ ] If the machine is in **Alarm**: `$$`, `$I` and `$G` still answer (GRBL
       allows them in Alarm). The alarm is explained in red.
3. [ ] Tick **Hide status reports**: the `?` and `<…>` lines disappear, the rest
       stays.

## 3. Board reset and disconnects

1. [ ] Press the controller's **reset button** (if it has one). The console
       notes "GRBL was reset.", the welcome message appears and `$I`, `$$`,
       `$G` are read again. Nothing moves.
2. [ ] **Unplug the USB cable** while connected. Within a few seconds the panel
       shows Disconnected with a "Connection lost" reason. The app keeps
       running.
3. [ ] Plug it back in, **⟳**, **Connect**: it connects again.
4. [ ] **Disconnect**, then close the app while connected on a second run: the
       app exits cleanly and the port is free for other programs.

## 4. Connect with DTR on

1. [ ] Disconnect, tick **DTR**, **Connect**.
2. [ ] On Arduino-based boards expect a reset (welcome message). The connection
       still identifies and reads as in step 1. Note whether your board
       resets with DTR off, on, or both.

## Results (copy into the PR)

| Item | Value |
|------|-------|
| Board / controller | e.g. Arduino Uno + CNC shield, MKS DLC32, … |
| USB-serial chip | e.g. CH340 (1A86:7523), from the port list |
| Firmware (`$I`) | e.g. `[VER:1.1h.20190825:]` `[OPT:V,15,128]` |
| OS | e.g. Windows 11 23H2 |
| Port and baud | e.g. `COM3` at 115200 |
| Reset on connect, DTR off | yes / no |
| Reset on connect, DTR on | yes / no |
| Identified via | welcome message / `$I` |
| Sections 1–4 | pass / fail (with notes) |
| Problems found | |

## Verifying read-only (for reviewers)

- **Types:** `Connection::request` takes a `ReadOnlyRequest`, an enum with four
  variants. Its bytes are defined in exactly one place,
  `ReadOnlyRequest::bytes` (`crates/pf-device/src/protocol/readonly.rs`):
  `?`, `$$\n`, `$I\n`, `$G\n`.
- **No raw writes:** the byte-writing `Transport` trait and `SerialTransport`
  are `pub(crate)`. The only write outside tests is in
  `crates/pf-device/src/connection.rs` and writes `request.bytes()`.
- **Tests:**
  - `connection::tests::a_full_session_only_writes_read_only_requests` splits
    everything written in a simulated session into the four requests and
    fails on any other byte.
  - `session::tests::under_random_input_only_read_only_requests_are_sent`
    drives the protocol with random real and garbage input.
  - `readonly::tests::acceptance_follows_grbl_system_c` checks when each
    request is sent against GRBL v1.1h `system.c`.
- **Console:** every byte sent appears as a `>` line, so this checklist's step
  1.4 checks it on real hardware too.
- **DTR:** see [`dtr.md`](dtr.md). PhotonForge never asserts DTR unless the
  box is ticked; the OS may still pulse it on open (Linux always does).
