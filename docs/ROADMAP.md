# Roadmap

Milestone ladder with time estimates. Solo development, ~30% reality buffer on top of raw estimates.

---

## Near-term (v0.0.1 → v0.2)

**v0.0.1 → v0.2** covers the core loop: record, replay, fork, diff, and filesystem snapshots. Everything past v0.2 compounds the corpus and alignment layers.

---

## Milestone table

| Version | What ships | Estimated time |
|---|---|---|
| **v0.0.1** | [shipped] Hook bridge for Claude Code, records tool calls + model streams + file-writes to a framed binary log. TUI replays as scrubbable transcript. macOS only. | — |
| **v0.1** | [shipped] Timeline scrubbing, step-through, fork-and-diff-two-runs (text-level). | — |
| **v0.2** | Content-addressed filesystem snapshots (userspace CoW via blake3 Merkle). Replay restores file state at any frame. | ~4–5 weeks |
| **v0.3** | PTY interception + syscall capture. Clock virtualization. `/dev/urandom` interception. The "real determinism" layer. | +6–8 weeks |
| **v0.4** | Fork-from-frame with external-API mocking (return recorded responses). Query DSL for trace search. | +3 weeks |
| **v0.5** | Linux port (`LD_PRELOAD` shim + inotify fast path already compiled in). | +6 weeks |
| **v0.6** | Hosted ingestion + multi-tenancy + PII boundaries for corpus contributions. Paid tier. | +4 weeks |
| **v0.7** | BLAST / Smith-Waterman alignment engine + substitution matrix learned from seed corpus. The durability bet. | +4–6 weeks |
| **v1.0** | Polish, onboarding flow, docs site, auth, billing, telemetry. | +3–4 weeks |

> **v0.4 stretch — synthetic corpus generator.** Mutation-style perturbations of known-good traces (drop a tool call, swap two, mutate args, inject an error result). Calibrates the v0.7 substitution matrix without waiting on real-user adoption. ~2–3 weeks on top of base v0.4.

---

## Totals

- **Estimated:** ~7–10 months → **~9–13 months with reality buffer**

---

## Technical milestones worth noting

- **v0.2** — CoW filesystem snapshotter for agent traces
- **v0.3** — `rr`-style determinism ported to the agent domain
- **v0.7** — BLAST-style sequence alignment for agent failure diagnosis
