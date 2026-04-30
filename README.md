# redo

> Time-travel debugger for LLM agent sessions.

**Working name** — verify namespace availability on GitHub, npm / PyPI / cargo, and a `.dev` domain before committing. Alternates: `recall`, `rerun`, `tide`, `rewind`.

---

## What it is

`redo` records every state transition inside a Claude Code / Agent SDK session — model tokens, tool calls, file writes, env reads, clock reads — into a framed, seekable binary log. You can then scrub back to any frame, fork from there, and diff two runs side by side. Later versions add a crowd-sourced corpus of known-bad traces and Smith-Waterman-style alignment so the debugger can tell you *"your run diverged at step 34, matches failure class #142 with E-value 1e-12."*

Closest analogy: Mozilla's `rr`, but for non-deterministic agentic systems instead of native binaries.

## Why it exists

Every agent tool ships tracing. None ship replay. Tracing tells you *what happened*. Replay lets you *go back and look*. For a system that's non-deterministic by construction and that does destructive things to your repo, replay is the missing primitive.

See [`docs/WHY.md`](./docs/WHY.md) for the origin story.

## How it works

Three design decisions carry most of the weight:

1. **Record model outputs, never re-infer.** The non-determinism of the LLM is the reason replay is hard, so on replay the "model call" is a lookup, not an API call.
2. **Framed binary log with a seek index.** Inspired by ARINC 717 flight-data recording. Zstd with a trained dictionary gets ~12× compression on real traces.
3. **Content-addressed filesystem snapshots.** Userspace CoW via blake3 Merkle trees — cross-platform, dedup-for-free, no kernel module, no OverlayFS dependency.

See [`docs/HOW.md`](./docs/HOW.md) for the full design walk-through.

## Quick start

`redo` ships four subcommands and one hook bridge. Storage defaults to `$XDG_DATA_HOME/redo` (fallback `~/.local/share/redo`); pass `--root` to override.

### 1. Start a recorder

```bash
redo record
# session_id=018f2a5b-...-...
# session_dir=/home/you/.local/share/redo/sessions/018f2a5b-...
# dropbox=/home/you/.local/share/redo/sessions/018f2a5b-.../dropbox
# env REDO_SESSION_DIR=/home/you/.local/share/redo/sessions/018f2a5b-...
```

The recorder prints a small banner you can `eval` or parse from a wrapper. It runs until you send `SIGINT` / `SIGTERM`, at which point it drains pending hook events and finalises `meta.json`.

### 2. Wire Claude Code hooks to the bridge

Export the session dir and point each Claude Code hook at `redo hook`:

```bash
export REDO_SESSION_DIR=/path/from/banner
# in your Claude Code hook config:
#   command: redo hook PreToolUse
# (similarly for PostToolUse, UserPromptSubmit, Stop, ...)
```

Each invocation reads the hook's stdin JSON and atomically stages a single file in the session's `dropbox/`. The recorder watches that directory and ingests every file as one frame in the log.

### 3. List, inspect, and replay

```bash
redo list                       # table of sessions
redo inspect <SESSION_ID>       # frames as NDJSON for scripting
redo replay  <SESSION_ID>       # scrubbable TUI
```

TUI keys: `j`/`k` step one frame, `J`/`K` jump 10, `g`/`G` first/last, `/` filter by event-kind substring, `q` quit.

### Platform note

v0.0.1 targets macOS. The recorder watches the dropbox via a 100 ms polling loop on macOS (and any non-Linux Unix). On Linux, an `inotify` fast path is compiled in but is not the primary supported target for this release.

## Status

v0.0.1 shipped on `main`: hook recording + replay TUI working end-to-end on macOS. Content-addressed filesystem snapshots land in v0.2.

- [`CHANGELOG.md`](./CHANGELOG.md) — what shipped, per release
- [`docs/ROADMAP.md`](./docs/ROADMAP.md) — milestone ladder ahead

## Docs

- [`docs/WHY.md`](./docs/WHY.md) — origin story and thesis
- [`docs/HOW.md`](./docs/HOW.md) — architecture and design decisions
- [`docs/ROADMAP.md`](./docs/ROADMAP.md) — milestones and time estimates
