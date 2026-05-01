# Changelog

All notable changes to this project are documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it reaches `0.1.0`.

## [Unreleased]

### Added
- `Event::Output` projection from `PostToolUse[Bash]`: stdout (and stderr when stdout is empty) is captured, base64-inlined up to `MAX_INLINE_PAYLOAD`, tagged with the new optional `stream` field (`stdout` / `stderr`) and `extras.source = "bash"`. The catch-all `Marker` still ships, so the tool-call span boundary is preserved.
- `Event::FileWrite` (new fifth variant, internally tagged on `kind = "file_write"`): projected from `PostToolUse[Edit|Write|MultiEdit]`. Re-reads the file at `tool_input.file_path`, blake3-hashes the bytes, inlines them when `size <= MAX_INLINE_PAYLOAD`, sets `truncated` + `truncated_original_size` otherwise. Race window between hook and re-read is documented as v0.2 work.
- `FORMAT_VERSION` bumped from `1` to `2`. Readers accept v1 and v2 logs interchangeably (new fields are `#[serde(default)]`).
- Hook-payload schema check on every ingest: `crate::hook::schema::validate` checks the envelope payload against per-kind required fields (e.g. `PostToolUse` requires `tool_name`, `tool_input`, `tool_response`). Drift fires `tracing::warn!` and bumps the new `Meta.schema_drift_events` counter; the recorder keeps running.
- `tests/fixtures/hooks/claude_code_v1.json` pins the Claude Code hook payload shape redo depends on, sourced from the public hook docs and observed PostToolUse output. When Anthropic ships a payload change, bump the fixture and the contract in `src/hook/schema.rs`.
- TUI: `FileWrite` is its own span kind (singleton, red on the scrub bar) with a `file_write <path> <size>B (<hash[..8]>)` summary.
- `Meta.frame_count: u64` cached on every meta tick and on finalize. `redo list` now reads `meta.json` only and falls back to a streaming log scan only for legacy sessions whose meta predates the field.
- `SessionReader::EventStream` streaming variant of `read` that yields one event at a time without buffering the full log.
- Hook bridge enforces `MAX_INLINE_PAYLOAD = 256 KiB` end-to-end: oversize stdin is dropped, the envelope is flagged with `truncated: true` and `truncated_original_size`, and the projected `Marker.extras` carries the same flags through to the canonical-line summary.
- CI matrix gains a `macos-latest` job (fmt + clippy + tests). Release matrix gains `aarch64-apple-darwin` and `x86_64-apple-darwin`.

### Changed
- `Cargo.toml` `version` bumped to `0.1.0`. Recorded session headers (`redo_version`) now carry the accurate release string instead of `0.0.0`.
- Doc scope tightened: the determinism claim is now scoped to "the hook-visible event stream" with syscall-level determinism explicitly deferred to v0.3. Killer-feature framing for the v0.7 alignment layer demoted to "the durability bet". README adds a "Status of replay fidelity" subsection enumerating what each event variant captures and what is still out-of-scope (model reasoning tokens, network state, subprocess trees of `Bash`).

### Fixed
- Recorder no longer dies on a transient ingest failure (a single unreadable dropbox file or a writer hiccup): the watcher closure logs, increments a new `Meta.ingest_errors` counter, and continues. Finalize always runs, so `meta.json` reliably ends in `Complete` rather than stuck in `Recording`.

## [0.1.0]

Timeline scrubbing, fork-from-frame, and text-level diff between two recorded sessions. macOS only.

### Added
- Span grouping in the TUI: consecutive related frames coalesce into spans (tool-call, model-stream, input-stream, resize, marker singletons). Defined in `src/tui/spans.rs`.
- One-line scrub bar at the bottom of the replay TUI showing the current frame's relative position with span boundaries marked and the cursor cell highlighted.
- New TUI keybinds: `shift-J` / `shift-K` jump to next / previous span boundary; `0`-`9` jump to that decile of the session; `Home` / `End` alias `g` / `G`.
- `redo fork <SESSION_ID> --at <FRAME> [--label LABEL]`: copies frames `[0..=FRAME]` from a parent session into a fresh session whose `meta.json` is annotated with `parent_session_id` and `forked_at_frame`. A closing `Marker` with label `fork-from <parent>:<frame>` is appended.
- `Meta` gains optional `parent_session_id: Uuid` and `forked_at_frame: u64`. Both `#[serde(default)]` so older session meta files still deserialise unchanged.
- TUI `f` keybind: fork at the current cursor frame, exit, and print the new session id on stdout (with a human-friendly note on stderr).
- `redo diff <SESSION_A> <SESSION_B> [--context N] [--no-color]`: text-level Myers diff (via the `similar` crate) over each session's canonical-line projection (`#<seq> <kind> <summary>`). Honours `--no-color` and `NO_COLOR=1`.
- `crate::format::canonical` module exporting `CanonicalLine` / `canonicalize` / `canonicalize_all`, shared between the diff CLI and the in-TUI diff view.
- TUI `d` keybind: prompts for a peer session id and opens a side-by-side diff view (two columns of canonical-tuple lines, aligned and highlighted) with `j` / `k` scrolling and `q` to return.
- `similar = "2.6"` added as a dependency for line-level Myers diff.
- Golden-fixture coverage for diff output: `tests/diff_fixture.rs` against `tests/fixtures/diff/expected.txt`.

### Changed
- TUI status bar advertises the new keybinds.
- `redo replay` is now action-bearing on exit: a fork pressed inside the TUI prints the new session id once the terminal is restored.

## [0.0.1] - 2026-05-01

First end-to-end slice. Records Claude Code hook events into a framed, seekable log on disk and replays them in a scrubbable TUI. macOS only.

### Added
- Framed log format: `SessionHeader` + `Event` enum (`Output` / `Input` / `Resize` / `Marker`), versioned at `FORMAT_VERSION = 1`, payloads inlined up to `MAX_INLINE_PAYLOAD = 256 KiB`.
- Session store on disk under `<root>/sessions/<uuid>/{log.ndjson.zst, meta.json, dropbox/}`. NDJSON-on-zstd with per-flush boundaries; reader tolerates a truncated tail.
- `Meta` document with atomic write via temp + rename.
- CLI surface (clap-derive): `redo {record, list, replay, inspect, hook}`. `--root` defaults to `$XDG_DATA_HOME/redo` (fallback `~/.local/share/redo`). Logs honour `REDO_LOG`.
- Hook bridge: `redo hook <kind>` reads a Claude Code hook JSON envelope from stdin and atomically writes it to `<session>/dropbox/<received_t_ns>-<uuid>.json`. Resolves the target session via `REDO_SESSION_DIR`.
- Recorder daemon: creates the session, watches the dropbox (100 ms polling on macOS, `inotify` fast path compiled on Linux), projects envelopes onto frames, periodically updates `meta.json`, drains and finalises on `SIGINT` / `SIGTERM`.
- TUI replay (ratatui): three panes (timeline / detail / fs-placeholder) with `j` `k` `J` `K` `g` `G` `/` `q` keybindings. Restores the terminal on panic via `panic_hook::install`.
- Tests: format roundtrip (12), store crash safety (6), end-to-end recorder (2). Total: 20 passing.
- `README.md` quick start covering record / hook wiring / list / inspect / replay.
- Design docs: `docs/WHY.md`, `docs/HOW.md`, `docs/ROADMAP.md`.

### Known limitations
- macOS only for v0.0.1. The `inotify` Linux branch compiles but has not been smoke-tested on Linux.
- Filesystem snapshots are out of scope until v0.2; the right TUI pane is a placeholder.
- Replay is read-only; fork-from-frame and diff-two-runs are v0.1.

[Unreleased]: https://github.com/aman751997/redo/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/aman751997/redo/compare/v0.0.1...v0.1.0
[0.0.1]: https://github.com/aman751997/redo/releases/tag/v0.0.1
