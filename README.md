# redo

[![CI](https://github.com/aman751997/redo/actions/workflows/ci.yml/badge.svg)](https://github.com/aman751997/redo/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

> Time-travel debugger for LLM agent sessions.

`redo` records every state transition inside a Claude Code session — tool calls, model outputs, file writes — into a seekable, content-addressed binary log. From that log you can scrub to any frame, fork into a new session, and diff two runs at the structural level.

Closest analogy: Mozilla's [`rr`](https://rr-project.org/), but for non-deterministic agentic systems instead of native binaries.

<!-- TODO: add demo GIF here once TUI recording is ready -->

## Why

Every agent tool ships tracing. None ship replay. Tracing tells you *what happened*. Replay lets you *go back and look*.

`redo` is **not** an undo button — Claude Code's own `/rewind` covers that. `redo` is for the questions `/rewind` cannot answer: *why* a run failed two days ago, *whether* the same failure shape happened to someone else, and *how* this run drifts from a reference trajectory.

See [`docs/WHY.md`](./docs/WHY.md) for the full thesis.

## How it works

Three design decisions carry most of the weight:

1. **Record model outputs, never re-infer.** On replay the "model call" is a lookup, not an API call.
2. **Framed binary log with a seek index.** Zstd with a trained dictionary gets ~12× compression on real traces.
3. **Content-addressed filesystem snapshots.** Userspace CoW via blake3 Merkle trees — cross-platform, dedup-for-free.

See [`docs/HOW.md`](./docs/HOW.md) for the full architecture walk-through.

## Quick start

### Install

```bash
cargo install --path .
```

Or run from the repo: `cargo run --release -- <args>` everywhere below says `redo <args>`.

### 1. Record a session

```bash
redo record
```

Prints a session banner with `REDO_SESSION_DIR`. Runs until `SIGINT`/`SIGTERM`.

### 2. Wire Claude Code hooks

Export the session dir from the banner, then point each hook at the bridge:

```bash
export REDO_SESSION_DIR=/path/from/banner
```

In your hook config (`~/.claude/settings.json`):

```
command: redo hook PreToolUse
```

Repeat for `PostToolUse`, `UserPromptSubmit`, `Stop`, `Notification`.

Smoke test:
```bash
printf '%s' '{"tool_name":"Bash","output":"hello\n"}' | redo hook PostToolUse
```

### 3. Replay, inspect, list

```bash
redo list                       # table of sessions
redo inspect <SESSION_ID>       # frames as NDJSON
redo replay  <SESSION_ID>       # scrubbable TUI
```

**TUI keys:** `j`/`k` step, `J`/`K` jump spans, `g`/`G` first/last, `0`–`9` decile jump, `f` fork, `d` diff, `/` filter, `q` quit.

### 4. Fork and diff

```bash
redo fork <SESSION_ID> --at 42 --label experiment
redo diff <SESSION_A> <SESSION_B> --context 5
```

Fork branches a session at any frame. Diff compares two sessions via Myers diff over canonical projections.

Inside `redo replay`: `f` forks at cursor, `d` opens side-by-side diff view.

## What v0.1 captures

| Event | Source | What's recorded |
|---|---|---|
| `Marker` | Every Claude Code hook | Verbatim payload in `extras` |
| `Output` | `PostToolUse[Bash]` | stdout (or stderr fallback), tagged `extras.source = "bash"` |
| `FileWrite` | `PostToolUse[Edit\|Write\|MultiEdit]` | blake3 hash + size + inline bytes (≤ 256 KiB) |

**Not yet captured:** model reasoning tokens, network state from `Bash` subprocesses, syscall-level clock/randomness (v0.3).

When Claude Code changes a hook payload shape, the recorder logs a warning and bumps `Meta.schema_drift_events`.

## Platform

v0.1 targets macOS. Linux `inotify` path compiles but is not the primary target yet. Full Linux port is v0.5.

## Status

v0.1 is current — span grouping, scrub bar, fork-from-frame, text-level diff. v0.2 adds content-addressed filesystem snapshots.

- [`CHANGELOG.md`](./CHANGELOG.md) — what shipped
- [`docs/ROADMAP.md`](./docs/ROADMAP.md) — what's next

## Docs

- [`docs/WHY.md`](./docs/WHY.md) — origin story and thesis
- [`docs/HOW.md`](./docs/HOW.md) — architecture and design decisions
- [`docs/ROADMAP.md`](./docs/ROADMAP.md) — milestone ladder

## Contributing

Issues and PRs welcome. The project is early — if something is broken or unclear, open an issue.

## License

[MIT](LICENSE)
