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

## Status

Pre-v0.0.1. Design docs only; implementation in progress.

See [`docs/ROADMAP.md`](./docs/ROADMAP.md) for the milestone ladder.

## Docs

- [`docs/WHY.md`](./docs/WHY.md) — origin story and thesis
- [`docs/HOW.md`](./docs/HOW.md) — architecture and design decisions
- [`docs/ROADMAP.md`](./docs/ROADMAP.md) — milestones and time estimates
