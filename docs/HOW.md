# How

Architecture and design decisions. This doc is written for the kind of reader who opens `DESIGN.md` before `README.md`.

---

## System overview

```
┌────────────────────────────┐
│   Claude Code / Agent SDK  │
└──────────────┬─────────────┘
               │ (hooks, pty, stdio)
               ▼
┌────────────────────────────┐
│        Recorder            │
│  - pty intercept           │
│  - tool-call capture       │
│  - model-stream capture    │
│  - fs-op capture           │
│  - env/clock capture       │
└──────────────┬─────────────┘
               │ frames
               ▼
┌────────────────────────────┐
│      Framed Log Store      │   ◄──── seek index
│  - zstd-trained dictionary │
│  - blake3 frame hashes     │
└──────────────┬─────────────┘
               │
               ▼
┌────────────────────────────┐
│       CoW Object Store     │
│  - blake3 content-addr     │
│  - Merkle tree per frame   │
└──────────────┬─────────────┘
               │
               ▼
┌────────────────────────────┐
│        Replay Engine       │
│  - frame seek              │
│  - fs restore at frame N   │
│  - model-output lookup     │
│  - fork-from-frame         │
└──────────────┬─────────────┘
               │
               ▼
┌────────────────────────────┐
│         TUI / Viewer       │
│  - timeline scrubber       │
│  - diff two runs           │
│  - query DSL over frames   │
└────────────────────────────┘
```

---

## Design decisions

### 1. Record model outputs, never re-infer

The non-determinism of the LLM is the reason replay is hard. The move is to push the non-determinism outside your recording boundary — record what came back from the model, hash it, and on replay return the recorded response as a lookup.

**Consequence:** replay is deterministic *with respect to the agent's hook-visible event stream* — same hook events in the same order, same captured stdout for `Bash`, same blake3-pinned file content for `Edit`/`Write`/`MultiEdit`. Syscall-level determinism (clock, randomness, network) lands in v0.3; v0.0.1–v0.2 do not claim it.

**Tradeoff:** you cannot "replay with a different prompt." The product scope is *"rewind this run as it happened"*, not *"what would have happened if I had asked differently."* The latter is much harder (Pernosco-level) and is explicitly out of v1.

### 2. Framed binary log with a seek index

**Format:** fixed-size header per frame (type, length, monotonic index, blake3 hash), variable-size payload, trailing checksum.

**Inspirations:** ARINC 717 (aviation flight-data recording), Cap'n Proto (zero-copy reads), Git packfile format (hash-addressed content).

**Why not JSON:** ~8× the size, not seekable without full parse, no natural content-addressing, no deterministic byte ordering.

**Compression:** zstd with a dictionary trained on 100+ real Claude Code traces. Real-world ratio ~12× on agent traces (very repetitive — tool schemas, file paths, prompt templates all repeat).

**Seek index:** monotonic frame index → byte offset. Rebuildable from a scan. In-memory footprint: ~16 bytes per frame. A one-hour session ≈ 100k frames ≈ 1.6 MB index. Negligible.

### 3. Content-addressed filesystem snapshots (userspace CoW)

Every file read or write at the instrumented boundary produces a blake3-hashed blob written to the object store. A "filesystem state at frame N" is a Merkle tree of (path → blob hash) pairs.

**Why not OverlayFS:** Linux-only, requires kernel cooperation, opaque to userspace queries, heavy.

**Why not `git add -A` snapshots:** Git's object store is beautiful but the staging dance is wrong for this use case (we want immutable per-frame snapshots, not an index).

**Why blake3:** faster than SHA-256, faster than blake2, tree-hashable natively (parallel hashing of large files), same security margin we need.

**Dedup-for-free:** two users with the same `package.json` share the blob. A 50-frame trace of edits to one file stores 50 small deltas, not 50 full copies.

**Cross-platform:** pure userspace. Works on Linux, macOS, Windows without per-OS branches.

### 4. Clock, randomness, and env virtualization

Non-determinism that isn't the LLM:

| Source | Capture | Replay |
|---|---|---|
| `clock_gettime`, `Date.now()` | record wall/monotonic at each call | return recorded value |
| `/dev/urandom`, `Math.random` seeding | capture consumed bytes/seed | return same bytes |
| `env` reads | capture key+value on first read | return recorded value |
| `uname`, `hostname` | capture once | return recorded value |
| `process.argv`, cwd | capture at session start | restore at replay start |

Mechanism differs per platform:
- **Linux:** `LD_PRELOAD` shim on the Claude Code process (or SDK bindings if recording at library level). `ptrace` considered; rejected for v1 — performance cost too high.
- **macOS:** `DYLD_INSERT_LIBRARIES` + interposing. No SIP issues in user-installed binaries.
- **Windows:** Detours-style IAT hooking. Out of scope for v0.x.

**Tradeoff:** `LD_PRELOAD`/`DYLD` can't catch a statically-linked binary. Claude Code is Node/JS so we have it easy — hook at the JS layer.

### 5. TUI / Viewer

v0.1 ships a Ratatui-style TUI:

- Left pane: timeline with frames grouped into tool-call spans
- Middle pane: frame details at cursor
- Right pane: filesystem view of state at frame N
- Keybindings: `j/k` step, `shift-J/K` jump spans, `f` fork, `d` diff mode, `/` query

v0.2+ adds a web viewer for the hosted tier. TUI stays for local.

### 6. Query DSL (v0.4)

Small language for searching over frames:

```
# find runs where a tool retry followed a 401
span:tool[name=*] after span:tool where exit_code=401

# all frames where file `src/auth.ts` was written
frame:fs-write where path = "src/auth.ts"

# find the first frame where the model output contained "```sql"
frame:model-token where text matches /```sql/
```

Roughly: Datalog shape, but closed over frame-stream primitives. Compiles to an index lookup plan.

### 7. Sequence alignment (v0.7)

For each frame, extract a canonical tuple:

```
(kind, name, args_hash, exit_hash)
```

where:
- `kind` ∈ { `model-token`, `tool-call`, `tool-result`, `fs-write`, `fs-read`, `env-read`, `clock-read` }
- `args_hash` is blake3 of a canonicalized arg shape (not values — values destroy the alignment)

Given two trace tuple sequences A and B, compute a Smith-Waterman local alignment with:
- **Substitution matrix:** learned from seed traces. `(tool-call, grep) ↔ (tool-call, rg)` is cheap. `(fs-write, *.py) ↔ (tool-call, curl)` is expensive.
- **Affine gap penalties:** opens expensive, extensions cheap.
- **E-value:** BLAST's formulation, calibrated against random trace pairs.

Output: *"your run aligned with failure corpus entry #142 with score 412, E-value 1e-12, divergence at frame 34."*

---

## Non-goals (explicit)

These are deliberately excluded from v1:

- **Replay with a different prompt** — too hard; scope creep.
- **Distributed / multi-agent replay** — single-process v1 only.
- **Network-level record/replay** — HTTPS record/replay is a separate product.
- **Fine-grained CPU determinism** — we are above the syscall boundary, not at instruction level.
- **Windows support** — macOS + Linux first. Windows in 2027 if demand.
- **Real-time streaming to cloud** — v1 is file-on-disk.

---

## Language choice

**Recorder + store + replay engine:** Rust. Reasons:
- FFI for the OS-level intercepts.
- No GC pauses during hot-path capture.
- Easy cross-compilation.
- Memory safety in the recording boundary is load-bearing.

**TUI:** Rust (Ratatui) or Go (bubbletea). Leaning Rust to keep one language.

**Hosted viewer (later):** TypeScript + React. No reason to be clever.

**Frame schema:** Protocol Buffers or Cap'n Proto. Cap'n Proto leaning for zero-copy reads.

---

## Open questions (rolling — versioned by milestone)

Resolved in v0.1:

- **Hook point — at the Agent SDK boundary, the Claude Code CLI boundary, or the pty level?** Hook bridge wins for v0.0.1–v0.2 (the `redo hook <kind>` subcommand consumes the Claude Code hook payload from stdin). Syscall-level capture (`DYLD_INSERT_LIBRARIES`) is the v0.3 boundary; v0.x is *not* deterministic at the syscall layer.
- **Frame schema versioning.** `FORMAT_VERSION = 2` as of this milestone. Readers accept v1 and v2 logs interchangeably (new fields are `#[serde(default)]`); writers emit v2 going forward. Future bumps follow the same forward-compat rule.

Open for v0.2+:

- [ ] How to handle tool calls that hit MCP servers that aren't locally run? (Record the JSON-RPC roundtrip; replay the response.)
- [ ] How to handle subprocess trees spawned by tools? (Extend recording boundary to subprocess stdio. Today `Bash` `tool_response.stdout` is captured but the subprocess tree is not.)
- [ ] Race window between `PostToolUse[Edit|Write]` firing and the recorder's re-read. v0.2 closes this by reconstructing content from `tool_input` over the previous CoW snapshot. v0.0.1–v0.1 explicitly accept the race.
- [ ] Should the hosted corpus accept traces from private repos? (Probably: PII-scrub at upload boundary, opt-in only.)
