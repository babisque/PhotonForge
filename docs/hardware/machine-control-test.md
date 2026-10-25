# Hardware test: machine control and the M3 cut (Phase 6.8)

This checklist runs PhotonForge's machine control on a real GRBL laser, ending
with the **M3 milestone**: a simple SVG cut end to end. It takes about 45
minutes. Record the results at the end of this file.

Everything here moves the machine, and steps 5–9 fire the laser. Every step was
tested against a simulated GRBL first, but a real machine can differ. Go
slowly, and **keep a hand near the machine's own emergency stop or power
switch**, not only the Stop button on screen.

## Before you start

- [ ] **Eye protection** rated for your laser's wavelength, on everyone in the
      room. Enclosure closed where the machine has one.
- [ ] A fire extinguisher in reach. Never leave the machine while it runs.
- [ ] Air assist and extraction on, if fitted.
- [ ] A piece of **scrap card** (about 60 × 60 mm) on the bed at the origin
      corner, for steps 5–8. The real material for step 9.
- [ ] Focus set for the material.
- [ ] No other program has the serial port open.
- [ ] Build: `cargo run -p pf-app --release`.
- [ ] Open **View → Console** and untick **Hide status reports** for the first
      connection, so every byte sent is visible.

## 1. Profile and connection

1. [ ] **Machine** tab: work area matches the bed, **Max S value** matches the
       controller's `$30`, **Max power** at 100 % (or lower if you want a cap),
       **Frame power** at 0. **Apply**.
2. [ ] **Device** tab → connect as in `grbl-readonly-smoke.md`.
3. [ ] No amber profile hints. In particular `$32=1` (laser mode). If `$32=0`,
       stop here: PhotonForge does not write settings; set it with your usual
       tool and reconnect.
4. [ ] The red **■ Stop** button shows in the Device tab and in the bottom
       status bar.

## 2. Stop when nothing runs

1. [ ] Press **■ Stop** while idle. The console shows `! (feed hold)`, then
       `Ctrl-X (soft reset)` about a second later, the welcome message, and
       `M5`. **Job: idle** again. Nothing moved.

## 3. Homing and unlock (only if `$22=1`)

Skip to step 4 if homing is disabled. The **Home ($H)** button then says why.

1. [ ] After connecting the machine is usually in **Alarm**. **Unlock ($X)…**
       opens a dialog with the alarm text and a homing hint. Press **Cancel**:
       the console shows no `$X`.
2. [ ] **Home ($H)**. The machine homes. "Not responding" does **not** appear
       while it homes. The status bar shows "Home done" and the state is
       **Idle**.
3. [ ] Home again and press **■ Stop** mid-cycle. Motion stops, GRBL reports an
       alarm, and **Job: stopping (AwaitingUnlock)** appears. The console shows
       "Home failed: interrupted by a reset".
4. [ ] **Unlock ($X)…** → **Unlock**. The console shows `$X` and then `M5`;
       **Job: idle**. Home once more so the position is known.

## 4. Jogging

1. [ ] **Jog** pad, **Step** 10 mm, **Feed** 1000 mm/min. Click each arrow. The
       head moves 10 mm the right way (X+ to the right, Y+ away from you on a
       bottom-left origin) and the position updates.
2. [ ] Move near an edge and click toward it with step 50 mm. The head stops
       **at** the work-area edge, not past it.
3. [ ] Tick **Continuous (moves while held)**. Hold an arrow: the head moves;
       release: it stops at once (console `0x85 (jog cancel)`). Hold, then
       switch to another window: it stops too.
4. [ ] The laser never lit during jogging.

## 5. Framing

1. [ ] **File → Import SVG…** →
       `crates/pf-io/tests/fixtures/m3_sample.svg`: a circle (blue) inside a
       30 mm square (red).
2. [ ] Move the design onto the card if needed. **Frame**. The head traces the
       square's outline at 3000 mm/min (capped at the profile's max feed),
       **laser off**, and returns to the start corner. **Job: done** →
       **Clear**.
3. [ ] **Frame** again and press **■ Stop** halfway. The head stops, the stop
       sequence runs, and after any unlock the state is idle.
4. [ ] The **Laser on at 0 %** box is greyed out (frame power is 0).

## 6. A low-power job with pause and resume

1. [ ] **Layers** tab: both layers at **3 %** power, 1000 mm/min, 1 pass. On
       card this should only mark lightly.
2. [ ] **▶ Start**. The confirmation lists the line count, an estimated time
       and the highest power (about S30 for `$30=1000`). **Cancel**: nothing is
       sent. **▶ Start** → **Start**.
3. [ ] The circle is marked first, then the square. The progress bar and
       elapsed time advance.
4. [ ] During the square press **⏸ Pause**. Motion stops and the **beam goes
       off** (look at it through your eye protection). The badge reads
       **Paused**.
5. [ ] **⏵ Resume**. The job continues from where it stopped and ends.
       **Done** → **Clear**.

## 7. Stop mid-job

1. [ ] **▶ Start** the same job and press **■ Stop** during the circle.
       Motion stops at once and **the laser is off**.
2. [ ] Expect either **Job: idle** (GRBL came back idle; the console ends with
       `M5`) or **Job: stopping (AwaitingUnlock)** (GRBL raised an alarm). In
       the second case **Unlock ($X)…** → **Unlock** (or **Home ($H)**) ends
       with `M5`, then idle.
3. [ ] Nothing resumed on its own.

## 8. Losing the connection mid-job

1. [ ] **▶ Start** the job and **pull the USB cable** during the circle. The
       machine stops when GRBL runs out of buffered lines (a few moves at
       most). The Job section shows **Failed** with "The connection was lost".
2. [ ] If the laser keeps firing after the motion stops, switch the machine off
       and note it in the results; this must not happen.
3. [ ] Plug in, **⟳**, **Connect**. Nothing resumes. **Acknowledge** the
       failure.

## 9. The M3 cut

1. [ ] Real material on the bed, focused. **Layers**: the blue layer at a
       marking setting for that material, the red layer at a cutting setting
       (from your usual settings or a material test).
2. [ ] **Frame** to check placement.
3. [ ] **▶ Start** → **Start**. Stay with the machine until it finishes.
4. [ ] The square drops out (or nearly) with the circle marked inside it, and
       the job ends **Done**.

## Results

| Item | Value |
|------|-------|
| Date | |
| Board / controller | e.g. Arduino Uno + CNC shield, MKS DLC32, … |
| Firmware (`$I`) | e.g. `[VER:1.1h.20190825:]` `[OPT:V,15,128]` |
| Laser | e.g. 5 W diode, 445 nm |
| OS | |
| `$22` homing / `$32` laser mode | |
| Material and settings for step 9 | e.g. 3 mm plywood, cut 100 % 300 mm/min × 2 |

| Step | Pass / fail | Notes |
|------|-------------|-------|
| 1. Profile and connection | | |
| 2. Stop when idle | | |
| 3. Homing and unlock | | |
| 4. Jogging | | |
| 5. Framing | | |
| 6. Low-power job, pause, resume | | Beam off while paused? |
| 7. Stop mid-job | | Idle or AwaitingUnlock? |
| 8. Connection lost | | |
| 9. M3 cut | | |
