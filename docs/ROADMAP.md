# Roadmap

Milestone ladder with honest time estimates. Solo, 80% of time on core, 20% on upstream churn + distraction. Add ~30% reality buffer on top of these.

---

## The "just enough for resume credit" tier

**Ship v0.0.1 → v0.2.** That's ~5 months part-time or ~2 months full-time. Already a complete resume story. Everything past v0.2 compounds — more depth, more moat, more revenue — but the portfolio piece is earned by v0.2.

---

## Milestone table

| Version | What ships | Part-time (15 hr/wk) | Full-time (40 hr/wk) |
|---|---|---|---|
| **v0.0.1** | Hooks Agent SDK + Claude Code, records tool calls + model streams + file-writes to a framed binary log. TUI replays as scrubbable transcript. Linux only. | **3–4 weeks** | **1.5 weeks** |
| **v0.1** | Timeline scrubbing, step-through, fork-and-diff-two-runs (text-level only). Ship on GitHub. First HN post. | **+6–8 weeks** | **+2–3 weeks** |
| **v0.2** | Content-addressed filesystem snapshots (userspace CoW via blake3 Merkle). Replay actually restores file state at any frame. | **+2–3 months** | **+4–5 weeks** |
| **v0.3** | PTY interception + syscall capture. Clock virtualization. `/dev/urandom` interception. The "real determinism" layer. | **+3–4 months** | **+6–8 weeks** |
| **v0.4** | Fork-from-frame with external-API mocking (return recorded responses). Query DSL for trace search. | **+1.5 months** | **+3 weeks** |
| **v0.5** | macOS port (`DYLD_INSERT_LIBRARIES` + interposing + Endpoint Security API where applicable). | **+3 months** | **+6 weeks** |
| **v0.6** | Hosted ingestion + multi-tenancy + PII boundaries for corpus contributions. Paid tier. | **+2 months** | **+4 weeks** |
| **v0.7** | BLAST / Smith-Waterman alignment engine + substitution matrix learned from seed corpus. **The killer feature.** | **+2–3 months** | **+4–6 weeks** |
| **v1.0** | Polish, onboarding flow, docs site, auth, billing, telemetry. | **+1.5–2 months** | **+3–4 weeks** |

---

## Totals

- **Part-time:** ~16–22 months raw → **20–28 months with reality buffer**
- **Full-time:** ~7–10 months raw → **9–13 months with reality buffer**

---

## Resume-payoff checkpoints

You accrue portfolio value long before v1.0:

- **Month 1 (v0.0.1)** — working demo, GitHub stars start, HN top-20 is plausible
- **Month 4 (v0.2)** — *"I wrote a CoW filesystem snapshotter for agent traces"* is a real talk at a local meetup
- **Month 8 (v0.3)** — *"I ported rr-style determinism to the agent domain"* is a strong conference submission (LangChain conf, AI Engineer Summit, Papers We Love, Systems Distributed)
- **Month 12 (v0.7)** — *"I ported BLAST to debug agent failures"* is a blog post that gets cited in 2027

---

## Motivation checkpoints

If you are part-time, pre-commit to these review points:

- **End of month 1:** did I ship v0.0.1? If no, the project is not happening.
- **End of month 4:** did I hit v0.2 and get at least 10 real users touching it? If no, pivot to #2 (Capability Kernel for MCP).
- **End of month 8:** is v0.3 at 90%+ replay fidelity? If no, decide whether to keep pushing or ship what I have and start writing.

Pre-commit to these checkpoints when motivation is high, so future-you can't renegotiate when it's low.

---

## Parallel tracks (don't wait)

Some work happens in parallel with coding. Budget time for:

- **Blog post draft** — started at v0.0.1, polished at v0.2, published with v0.1
- **Conference talk proposal** — submit at v0.3 window
- **Design doc (`DESIGN.md`)** — written *before* v0.0.1, updated per milestone
- **Demo GIFs / video** — one per milestone, embedded in README
- **Twitter/X or Bluesky presence** — build-in-public cadence from week 1

Total parallel-track time: ~10% of build time. Non-negotiable — without these, the code is invisible.
