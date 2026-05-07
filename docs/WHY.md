# Why

## The origin moment

40 minutes into a Claude Code session, the agent deleted a file that mattered. There was no way to rewind. No way to see the state it was looking at three steps earlier. No way to ask *"what did you think was true when you decided to do that?"*

Every agent tool on the market ships tracing. None ship replay.

Tracing tells you what happened. Replay lets you *go back and look*. For a system that is non-deterministic by construction and that does destructive things to your repository, replay is the missing primitive.

## The insight

> *"Can you make a deterministic record/replay debugger for a system whose central component (the LLM) is fundamentally non-deterministic?"*

Yes. The move is:

1. **Record model outputs, don't re-infer them.** The non-determinism is external to your trace boundary. Freeze it at capture time.
2. **Virtualize the agent's environment** the way `rr` virtualizes syscalls — filesystem, tool calls, clock, randomness, environment variables.
3. **Commit to a semantic frame model.** Every state transition is a frame with a monotonic index. Everything else — scrub, fork, diff, align — is index math.

Once you have that, the non-deterministic LLM becomes the most boring part of the system. The hard parts are all classical: framed log formats, Merkle trees, clock virtualization, sequence alignment. The LLM is just the subject you're debugging, not a component you're integrating with.

## Prior art the design stands on

- **Mozilla `rr` (2014)** — deterministic record/replay for native binaries on Linux. The granddaddy. The thing I steal the most from.
- **`pernosco`** — hosted time-travel debugger built on top of `rr`. Shows what a polished commercial version looks like.
- **Git object store** — content-addressed Merkle tree of blobs and trees. The filesystem-snapshot layer borrows directly.
- **ARINC 717** — aviation flight-data recording frame format. Framed binary telemetry with a monotonic index. The log-format inspiration.
- **TAS (Tool-Assisted Speedruns) in games** — record inputs, replay determinism. Shows that non-determinism is about where you draw the recording boundary.
- **BLAST / Smith-Waterman** — biological sequence alignment. Later maps trace tuples to a substitution matrix for corpus-wide failure diagnosis.
- **OpenTelemetry spans** — the thing this project deliberately is *not* built on. Spans discard state; you can't retrofit replay onto them.

## The market premise (supporting, not leading)

The commercial angle matters less than the technical bet, but for completeness:

- Teams running agents in production (agents in CI, agents on prod data, agents merging code) carry high cost-of-failure and will pay for debugging.
- Tracing incumbents (Langfuse, Braintrust, Arize) built on OTel and cannot retrofit replay. That asymmetry is structural, not a matter of effort.
- The corpus-alignment layer compounds with users — each failing run recorded sharpens the next user's diagnosis.

<!-- adversarial premise review is tracked internally, not published -->

## Why not the alternatives

- **Just better tracing.** OTel spans can't capture enough state to replay. Every "richer span" effort runs into cardinality and retention walls.
- **Just a transcript viewer.** What Langfuse and Braintrust already ship. Doesn't let you fork, diff, or restore filesystem state.
- **Let Anthropic ship it.** They might, for the simple replay layer. The corpus-alignment layer is harder — a data-acquisition problem, not a coding problem.
- **Wait for rr2 or LLDB-for-agents.** The prior art for time-travel debugging is a decade old and still hasn't made it to the agent domain. Someone has to port it. That someone can be you.

## Why not Claude Code's `/rewind`

Claude Code ships a `/rewind` slash command that reverts the conversation and (optionally) restores file state to a prior turn-level checkpoint. It's the right tool for "undo the last few steps and try again." It is **not** the same shape as redo:

| Axis | `/rewind` | redo |
|---|---|---|
| Scope | In-process, current session only | Out-of-process recorder; standalone artifact on disk |
| Action | Mutates state back (destructive) | Read-only inspectable log (additive) |
| Granularity | Conversation-turn checkpoints | Per-frame: model token, tool call, fs op |
| Diff two runs | No | `redo diff`, side-by-side |
| Cross-session search | No | Index over all sessions on a root |
| Fork-from-frame | Implicit (it's the new head) | Explicit, named, replayable independently |
| Survives session end | Best-effort, in Claude's storage | Persistent log forever, content-addressed |
| Shareable | No | Yes — log is a self-contained artifact |
| Audience | User mid-task | Debugger, post-mortem, sharing, training data |

Different shape, different audience. `/rewind` ≈ `git reset --hard`. redo ≈ `rr` + `git log -p` for agents.

**Honest overlap and where redo still has room:**
- The "I want to back out and try again" use case is fully covered by `/rewind`. redo doesn't compete there and shouldn't pretend to.
- The CoW filesystem-snapshot layer overlaps with Claude's checkpoint store. redo's value-add is that the snapshot is a portable, content-addressed, dedup-across-sessions, queryable artifact — not a private checkpoint tied to one session's state.
- Forensics ("why did this run blow up two days ago"), comparison ("did this same failure happen yesterday"), reproducibility ("send me your trace"), and corpus alignment ("is this run drifting from a known-good trajectory") are not addressable by `/rewind` and likely never will be.

**What stops Anthropic from shipping this?** Nothing, for the basic replay layer. The longer-term value is in the corpus-alignment layer — comparing runs against a growing body of recorded traces. That's a data-acquisition problem, not a coding problem.
