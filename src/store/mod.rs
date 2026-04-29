//! On-disk session store.
//!
//! A session lives in `<root>/sessions/<uuidv7>/` and contains:
//!
//! * `log.ndjson.zst` — compressed NDJSON. The first line is a
//!   `SessionHeader`; the rest are `Event` records, one per line.
//! * `meta.json` — small JSON document tracking the session's lifecycle
//!   state, owner pid, and counters.
//! * `dropbox/` — drop directory for hook-event files (used by later layers).
//!
//! The writer emits a fresh zstd frame on every flush, so a partial trailing
//! frame from a crash is silently skipped by the reader rather than fataling.

pub mod meta;
pub mod reader;
pub mod session;
pub mod writer;

pub use meta::{Meta, SessionState};
pub use reader::{ReadResult, SessionReader};
pub use session::SessionStore;
pub use writer::SessionWriter;
