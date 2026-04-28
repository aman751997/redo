pub mod event;
pub mod header;

pub use event::Event;
pub use header::{SessionHeader, TermSize};

/// Log format version. Bumped on incompatible schema changes.
pub const FORMAT_VERSION: u32 = 1;

/// Magic string identifying the file format.
pub const FORMAT_NAME: &str = "redo";

/// Maximum bytes inlined into Output / Input event payloads.
/// Larger payloads are truncated; the original size is recorded.
pub const MAX_INLINE_PAYLOAD: usize = 256 * 1024;
