# Changelog

All notable changes to this project are documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it reaches `0.1.0`.

## [Unreleased]

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

[Unreleased]: https://github.com/aman751997/redo/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/aman751997/redo/releases/tag/v0.0.1
