# PhotonForge

PhotonForge is free and open-source laser engraving and cutting software, an
alternative to LightBurn. It targets GRBL-based laser controllers first.

**Status:** early development (Phase 0, workspace bootstrap). See
[`ROADMAP.md`](ROADMAP.md) for the plan and [`docs/specs/`](docs/specs/) for
the per-phase specs.

## Building

Requires Rust stable (1.99 or newer).

```sh
cargo run -p pf-app                                    # run the app
cargo test --workspace                                 # run the tests
cargo clippy --workspace --all-targets -- -D warnings  # lint
cargo fmt --all                                        # format
```

On Linux, building the GUI needs the X11/Wayland and fontconfig development
headers. On Debian/Ubuntu:

```sh
sudo apt-get install libxkbcommon-dev libxcb-render0-dev libxcb-shape0-dev \
  libxcb-xfixes0-dev libwayland-dev libfontconfig1-dev
```

## Workspace

| Crate       | Purpose                                                  |
|-------------|----------------------------------------------------------|
| `pf-core`   | Geometry, shapes, layers, units (mm). No I/O, no GUI.    |
| `pf-io`     | File import/export (SVG, images, project files).         |
| `pf-gcode`  | Toolpath to G-code generation. Pure functions.           |
| `pf-device` | Device communication (GRBL serial, status, streaming).   |
| `pf-app`    | eframe GUI binary, a thin layer over the other crates.   |

## License

PhotonForge is licensed under the GNU General Public License v3.0 or later. See
[`LICENSE`](LICENSE).
