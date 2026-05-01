# redo

> Time-travel debugger for LLM agent sessions.

**Working name** — verify namespace availability on GitHub, npm / PyPI / cargo, and a `.dev` domain before committing. Alternates: `recall`, `rerun`, `tide`, `rewind`.

---

## What it is

`redo` is a forensic time-travel debugger for LLM agent sessions. Every state transition inside a Claude Code / Agent SDK run — model tokens, tool calls, file writes, env reads, clock reads — is recorded into a framed, seekable, content-addressed binary log on disk. The log is portable, shareable, and queryable across sessions. From it you can:

- Scrub back to any frame and inspect what the agent thought was true.
- Fork from any frame into a new replayable session.
- Diff two runs at the structural level, with a coming sequence-alignment layer that flags drift against a corpus of known-bad traces (*"your run diverged at step 34, matches failure class #142 with E-value 1e-12"*).

Closest analogy: Mozilla's `rr`, but for non-deterministic agentic systems instead of native binaries.

`redo` is **not** an undo button — Claude Code's own `/rewind` already covers that case. `redo` is for the questions `/rewind` cannot answer: *why* a run failed two days ago, *whether* the same failure shape happened to someone else, and *how* this run drifts from a reference trajectory. See [`docs/WHY.md`](./docs/WHY.md#why-not-claude-codes-rewind) for the head-on comparison.

## Why it exists

Every agent tool ships tracing. None ship replay. Tracing tells you *what happened*. Replay lets you *go back and look*, share the trace with someone else, and align it to a corpus of prior runs. For a system that's non-deterministic by construction and does destructive things to your repository, replay is the missing primitive.

See [`docs/WHY.md`](./docs/WHY.md) for the origin story.

## How it works

Three design decisions carry most of the weight:

1. **Record model outputs, never re-infer.** The non-determinism of the LLM is the reason replay is hard, so on replay the "model call" is a lookup, not an API call.
2. **Framed binary log with a seek index.** Inspired by ARINC 717 flight-data recording. Zstd with a trained dictionary gets ~12× compression on real traces.
3. **Content-addressed filesystem snapshots.** Userspace CoW via blake3 Merkle trees — cross-platform, dedup-for-free, no kernel module, no OverlayFS dependency.

See [`docs/HOW.md`](./docs/HOW.md) for the full design walk-through.

## Quick start

`redo` ships four subcommands and one hook bridge. Storage defaults to `$XDG_DATA_HOME/redo` (fallback `~/.local/share/redo`); pass `--root` to override.

### 0. Get the binary

Either install:

```bash
cargo install --path .
```

…or run from a release build inside the repo (use `cargo run --release -- <args>` everywhere this README writes `redo <args>`).

### 1. Start a recorder

```bash
redo record
```

Sample output:

```
session_id=018f2a5b-...-...
session_dir=/Users/you/Library/Application Support/redo/sessions/018f2a5b-...
dropbox=/Users/you/Library/Application Support/redo/sessions/018f2a5b-.../dropbox
env REDO_SESSION_DIR=/Users/you/Library/Application Support/redo/sessions/018f2a5b-...
```

The recorder prints that banner to stdout. It then runs until you send `SIGINT` / `SIGTERM`, at which point it drains pending hook events and finalises `meta.json`.

### 2. Wire Claude Code hooks to the bridge

Export the session dir from the banner above:

```bash
export REDO_SESSION_DIR=/path/from/banner
```

Then point each Claude Code hook at `redo hook` in your hook config (e.g. `~/.claude/settings.json`):

```
command: redo hook PreToolUse
```

(Similarly for `PostToolUse`, `UserPromptSubmit`, `Stop`, `Notification`.)

Each invocation reads the hook's stdin JSON and atomically stages a single file in the session's `dropbox/`. The recorder watches that directory and ingests every file as one frame in the log. Hook payloads above 256 KiB are truncated and flagged on the resulting frame (the `truncated` and `truncated_original_size` fields in the marker's extras).

To smoke-test the bridge by hand, use `printf '%s'` rather than `echo`. Some shells (notably zsh) expand `\n` inside `echo` arguments to a literal newline, which produces invalid JSON:

```bash
printf '%s' '{"tool_name":"Bash","output":"foo\nbar\n"}' | redo hook PostToolUse
```

### 3. List, inspect, and replay

```bash
redo list                       # table of sessions
redo inspect <SESSION_ID>       # frames as NDJSON for scripting
redo replay  <SESSION_ID>       # scrubbable TUI
```

TUI keys: `j`/`k` step one frame, `J`/`K` jump to next/prev span boundary, `g`/`G` (or `Home`/`End`) first/last, `0`-`9` jump to that decile of the session, `f` fork at the current frame, `d` open a side-by-side diff against a peer session, `/` filter by event-kind substring, `q` quit.

### 4. Fork & diff

`redo fork` branches a session at any frame. The resulting session is a complete recording on its own — copy of the parent's `[0..=FRAME]` prefix plus a closing `fork-from <parent>:<frame>` marker — and its `meta.json` records the parent id and the fork frame.

```bash
redo fork <SESSION_ID> --at 42 --label experiment
# prints the new session id on stdout
```

You can also press `f` at any frame inside `redo replay` to fork at the cursor. The TUI exits cleanly and prints the new session id.

`redo diff` compares two sessions at the structural level: each session is projected to a sequence of canonical lines `#<seq> <kind> <summary>` (which elides payload bytes — see `src/format/canonical.rs`) and the projections are diffed via Myers. The output is unified-diff style with optional ANSI colour. `--no-color` and the `NO_COLOR` environment variable both suppress escapes.

```bash
redo diff <SESSION_A> <SESSION_B> --context 5
NO_COLOR=1 redo diff <SESSION_A> <SESSION_B>
```

Inside `redo replay`, press `d` and enter a peer session id to open a two-column side-by-side view of the same diff with `j`/`k` scrolling and `q` to return.

### Platform note

v0.0.1 targets macOS. The recorder watches the dropbox via a 100 ms polling loop on macOS (and any non-Linux Unix). On Linux, an `inotify` fast path is compiled in but is not the primary supported target for this release.

## Status

v0.1 ships span grouping + scrub bar in the replay TUI, fork-from-frame, and text-level diff between two recorded sessions on macOS. v0.0.1 is the prior milestone (hook recording + replay TUI). Content-addressed filesystem snapshots land in v0.2.

- [`CHANGELOG.md`](./CHANGELOG.md) — what shipped, per release
- [`docs/ROADMAP.md`](./docs/ROADMAP.md) — milestone ladder ahead

## Docs

- [`docs/WHY.md`](./docs/WHY.md) — origin story and thesis
- [`docs/HOW.md`](./docs/HOW.md) — architecture and design decisions
- [`docs/ROADMAP.md`](./docs/ROADMAP.md) — milestones and time estimates
