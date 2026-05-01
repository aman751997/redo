pub mod canonical;
pub mod event;
pub mod header;

pub use canonical::{canonicalize, canonicalize_all, CanonicalLine};
pub use event::{Event, OutputStream};
pub use header::{SessionHeader, TermSize};

/// Log format version.
///
/// * `1` — `Output` / `Input` / `Resize` / `Marker` only.
/// * `2` — adds `FileWrite` and an optional `stream` field on `Output`.
///   Readers accept v1 and v2 logs interchangeably (the new fields are
///   `#[serde(default)]`); writers emit v2 going forward.
pub const FORMAT_VERSION: u32 = 2;

/// Magic string identifying the file format.
pub const FORMAT_NAME: &str = "redo";

/// Maximum bytes inlined into Output / Input / FileWrite payloads.
/// Larger payloads are truncated; the original size is recorded.
pub const MAX_INLINE_PAYLOAD: usize = 256 * 1024;
