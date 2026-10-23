# DTR and controller resets on connect

PhotonForge never resets a GRBL controller on purpose when it connects. This
page explains the one way a reset can still happen, and what PhotonForge does
about it.

## Why DTR matters

Most GRBL boards are Arduino-based (Uno/Nano with a CH340 or ATmega16U2 USB
chip). On these boards the serial DTR line is wired to the microcontroller's
reset pin through a capacitor, so **any DTR transition resets GRBL**. That is
how the Arduino bootloader gets started for uploads.

A GRBL reset is not dangerous: it stops all motion, turns the spindle/laser
off, and prints the welcome message `Grbl 1.1h ['$' for help]`. But it does
abort anything running and can drop the machine into an alarm state (for
example `ALARM:3` "Abort during cycle" if it was moving), so it must never
happen silently.

Some boards (several 32-bit and native-USB designs) only send data when DTR is
asserted. For those, DTR has to be turned on.

## What PhotonForge does

* **DTR is off by default** (`SerialSettings::dtr = false`). PhotonForge never
  asserts DTR unless you tick the **DTR** checkbox in the connection panel.
* The port is opened 8N1, no flow control, and PhotonForge then drives DTR to
  the chosen level (`serialport`'s `dtr_on_open`).
* PhotonForge never sends a soft reset (Ctrl-X) in the read-only phase.
* Because no reset is forced, GRBL may not print its welcome message on
  connect. PhotonForge then identifies the controller with `$I` (build info),
  which is read-only.

## What the operating system does anyway

Choosing "DTR off" means PhotonForge does not assert it. It cannot always stop
the OS or USB driver from pulsing it while the port opens:

| OS | Behavior (serialport 4.10) | Effect on Arduino-based GRBL |
|----|----------------------------|------------------------------|
| Linux | The kernel asserts DTR briefly on every open, whatever is requested. This cannot be prevented without kernel changes. | Usually resets on connect. |
| Windows | The port is configured with DTR disabled, then set to the requested level. Some USB-serial drivers still pulse DTR during open. | Often no reset with DTR off; depends on the driver. |
| macOS | DTR is set to the requested level after opening; the driver may pulse it on open. | Depends on the driver. |

If the board resets on connect, you will see the welcome message in the
console. That is expected and safe.

## If the connection shows no data

1. Check the baud rate (GRBL 1.1 default: 115200).
2. Tick **DTR** and connect again. Expect a reset on Arduino-based boards.
