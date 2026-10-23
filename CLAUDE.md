# PhotonForge
Free and open-source (GPL-3.0) laser engraving/cutting software — an alternative to LightBurn.

## Stack
- Rust stable (edition 2024), Cargo workspace
- GUI: egui/eframe
- First supported controller: GRBL (G-code over serial, via `serialport`)
- SVG import via `usvg`; raster images via `image`

## Workspace layout
- `crates/pf-core` — geometry, shapes, layers, units (internal unit: mm). No I/O, no GUI.
- `crates/pf-io` — file import/export (SVG, images, project files).
- `crates/pf-gcode` — toolpath → G-code generation. Pure functions.
- `crates/pf-device` — device communication (GRBL serial, status, streaming).
- `crates/pf-app` — eframe GUI binary. Thin layer over the other crates.

## Run commands
- dev: cargo run -p pf-app
- test: cargo test --workspace
- lint: cargo clippy --workspace --all-targets -- -D warnings
- format: cargo fmt --all
- check: cargo check --workspace

## Conventions
- Prefer small, pure functions and plain data structs over trait hierarchies.
- No complex abstractions — prioritize clarity over brevity.
- Every new feature requires tests (unit tests in the same file, integration tests in `tests/`).
- G-code generation must have snapshot/golden tests for its output.
- Business logic never lives in the GUI crate; the app only calls into the other crates.
- No `unwrap()`/`expect()` outside tests — use `Result` with `thiserror` in libraries and `anyhow` in the app.
- Code, comments, commit messages and docs in English.
- Run `cargo fmt` and `cargo clippy` before considering a task done.

## Off-limits (never touch without explicit instruction)
- Any code path that sends motion or laser-on commands to real hardware — safety first; propose the change and wait for approval.
- Laser safety limits (max power, work area bounds, frame/ping power) — never relax defaults.
- Project file format (serialization) — propose a migration plan first.
- Adding new dependencies — propose the crate and why before adding it.
- `LICENSE`.

## Fallbacks (when in doubt)
- Propose 2 options and ask me to choose.
- Never rename public APIs of a crate without asking.
- If a GRBL behavior is unclear, cite the GRBL docs/source instead of guessing.

<!-- BEGIN OPENLORE (managed — edits inside this block will be overwritten) -->
<!-- openlore-fingerprint: f5e6039ff6662e0c -->
This project uses OpenLore for persistent architectural memory.

Call `orient "<task description>"` (via the openlore MCP server, or
`npx openlore orient --json`) **before touching a module you have not yet read in
this session**, and when a task spans several modules. It returns the relevant
functions, callers, spec sections, and insertion points in one structural lookup
instead of file-by-file rediscovery.

Skip it for work you are already inside: repeated edits to a file you have read
this session do not need re-orientation. Reach for it again when the task moves
to unfamiliar code.

OpenLore prefixes tool responses with a brief, factual freshness note (the
Epistemic Lease) once your cached context has aged or the repo has moved since
your last `orient()`. It is informational — re-`orient()` if you are relying on
cached cross-module structure; otherwise carry on.

For the MCP setup, ensure `openlore mcp` is configured as an MCP server.
See https://github.com/clay-good/OpenLore for details.

Wired MCP surface: `substrate`. This guidance is written for that surface; re-run `openlore install --preset <name>` to change it and regenerate these instructions.
<!-- END OPENLORE -->

<!-- openlore-decisions-instructions -->
## Architectural decisions

When making a significant design choice, call `record_decision` **before** writing the code.

Significant choices: data structure, library/dependency, API contract, auth strategy,
module boundary, database schema, caching approach, error handling pattern.

```
record_decision({
  title: "Use JWTs for stateless auth",
  rationale: "Avoids session store in infra",
  consequences: "Tokens can't be revoked early",
  affectedFiles: ["src/auth/middleware.ts"],
  supersedes: "a1b2c3d4"  // 8-char ID of prior decision being reversed
})
```

Decisions are consolidated in the background immediately after `record_decision` is called — the pre-commit gate reads the already-consolidated store and adds no LLM latency.

**Performance note**: if you skip `record_decision`, the gate detects unrecorded source changes at commit time and triggers a slow LLM extraction on the *next* commit (~10-30s). Calling `record_decision` proactively keeps every commit instant.

## When git commit is blocked by the decisions gate

If `git commit` fails and the output is JSON with `"gated": true`, do NOT retry silently.
Check the `reason` field and act accordingly:

**`reason: "verified"` — decisions await review:**
Present each decision to the user:
> "The commit is blocked — I found N architectural decision(s) to validate:
> 1. **[id]** Title — rationale
Do you approve? (yes/no)"
For each approval call `approve_decision`, for rejections call `reject_decision`.
Then run `openlore decisions --sync` and retry `git commit`.

**`reason: "approved_not_synced"` — decisions approved but not written to specs:**
Run `openlore decisions --sync` then retry `git commit`. Do not skip this step.

**`reason: "drafts_pending_consolidation"` — drafts were recorded but not yet consolidated:**
Present to the user:
> "N decision draft(s) were recorded but never consolidated. Run consolidation now? (~10-30s)"
If yes: run `openlore decisions --consolidate --gate` and handle the result.
If no: retry with `git commit --no-verify` to skip the gate.

**`reason: "no_decisions_recorded"` — source files staged but nothing recorded:**
Present to the user:
> "Source files are staged but no architectural decisions were recorded. Run fallback extraction to check for undocumented decisions? (~10-30s)"
If yes: run `openlore decisions --consolidate --gate` and handle the result.
If no: retry with `git commit --no-verify` to skip the gate.
<!-- end-openlore-decisions-instructions -->
