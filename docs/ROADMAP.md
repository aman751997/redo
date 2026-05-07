# Roadmap

What's shipped, what's next, and where this is headed.

---

## Shipped

- **Hook bridge + recorder** — records Claude Code tool calls, model streams, and file-writes to a framed binary log. macOS.
- **TUI replay** — scrubbable transcript with span grouping and a scrub bar.
- **Fork-from-frame** — branch a session at any frame into an independent recording.
- **Text-level diff** — Myers diff over canonical projections of two sessions.

## Next

- [ ] **Filesystem snapshots** — content-addressed CoW via blake3 Merkle trees. Replay restores file state at any frame.
- [ ] **Linux port** — `LD_PRELOAD` shim, inotify fast path (already compiles, needs testing).

## Future ideas

- PTY interception + syscall capture (clock virtualization, `/dev/urandom` interposition) for deeper replay fidelity
- External-API mocking on fork (return recorded responses)
- Query DSL for searching over frames
- Sequence alignment across sessions (Smith-Waterman over canonical trace tuples) for failure-pattern detection
- Hosted corpus with multi-tenancy and PII boundaries
