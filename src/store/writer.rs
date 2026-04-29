use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use zstd::stream::write::Encoder;

use crate::format::{Event, SessionHeader};

/// Flush after this many events.
pub const DEFAULT_FLUSH_EVENTS: usize = 100;

/// Flush after this much wall-clock time has elapsed since the last flush.
/// This is a max-latency bound, not a wall-clock tick: the timer resets on
/// every write, so a quiet session does not flush.
pub const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_millis(250);

/// zstd compression level. Level 3 is the standard fast/decent-ratio knob.
pub const ZSTD_LEVEL: i32 = 3;

/// Streams events to a compressed NDJSON file. Each flush ends a zstd frame
/// so that a partial trailing frame from a crash is recoverable by the reader.
pub struct SessionWriter {
    encoder: Option<Encoder<'static, BufWriter<File>>>,
    events_since_flush: usize,
    last_flush: Instant,
    flush_events: usize,
    flush_interval: Duration,
    finished: bool,
}

impl SessionWriter {
    /// Open `path` for writing, emit the session header as the first record.
    pub fn create(path: impl AsRef<Path>, header: &SessionHeader) -> Result<Self> {
        let path = path.as_ref();
        let file = File::create(path).with_context(|| format!("create log {}", path.display()))?;
        let buf = BufWriter::new(file);
        let mut encoder = Encoder::new(buf, ZSTD_LEVEL).context("init zstd encoder")?;
        // Don't add a content checksum -- it would force buffering an entire
        // session before flushing. We rely on per-frame structure for recovery.
        encoder
            .include_checksum(false)
            .context("configure zstd encoder")?;

        let mut writer = Self {
            encoder: Some(encoder),
            events_since_flush: 0,
            last_flush: Instant::now(),
            flush_events: DEFAULT_FLUSH_EVENTS,
            flush_interval: DEFAULT_FLUSH_INTERVAL,
            finished: false,
        };
        writer.write_header(header)?;
        Ok(writer)
    }

    fn write_header(&mut self, header: &SessionHeader) -> Result<()> {
        let line = serde_json::to_string(header).context("serialize header")?;
        self.write_line(&line)?;
        // Header gets its own frame so a reader sees it immediately even if a
        // crash happens before the first event batch is flushed.
        self.flush_frame()?;
        Ok(())
    }

    /// Append one event. May trigger a flush + frame boundary.
    pub fn write_event(&mut self, event: &Event) -> Result<()> {
        let line = serde_json::to_string(event).context("serialize event")?;
        self.write_line(&line)?;
        self.events_since_flush += 1;

        let needs_flush = self.events_since_flush >= self.flush_events
            || self.last_flush.elapsed() >= self.flush_interval;
        if needs_flush {
            self.flush_frame()?;
        }
        Ok(())
    }

    /// Force a flush + frame boundary now. Used for signal-generated events
    /// (Resize, Marker) that should be durable immediately.
    pub fn flush_frame(&mut self) -> Result<()> {
        let enc = self.encoder.as_mut().context("writer already finished")?;
        // Flush the zstd block stream and end the current frame. The next
        // write will start a brand-new frame.
        enc.flush().context("flush zstd")?;
        enc.do_finish().context("end zstd frame")?;
        // do_finish() leaves the encoder in a state where it must be replaced
        // before further writes; rebuild it on top of the same underlying
        // BufWriter so subsequent events form their own frame.
        let buf = self
            .encoder
            .take()
            .unwrap()
            .finish()
            .context("finish frame")?;
        let mut next = Encoder::new(buf, ZSTD_LEVEL).context("re-init zstd encoder")?;
        next.include_checksum(false)
            .context("configure zstd encoder")?;
        self.encoder = Some(next);

        self.events_since_flush = 0;
        self.last_flush = Instant::now();
        Ok(())
    }

    fn write_line(&mut self, line: &str) -> Result<()> {
        let enc = self.encoder.as_mut().context("writer already finished")?;
        enc.write_all(line.as_bytes()).context("write log line")?;
        enc.write_all(b"\n").context("write newline")?;
        Ok(())
    }

    /// Close the writer cleanly: flush, end the trailing frame, finish the
    /// encoder. Idempotent.
    pub fn finish(mut self) -> Result<()> {
        self.finish_inner()
    }

    fn finish_inner(&mut self) -> Result<()> {
        if self.finished {
            return Ok(());
        }
        if let Some(enc) = self.encoder.take() {
            let buf = enc.finish().context("finalise zstd encoder")?;
            // Drain BufWriter and fsync the underlying file.
            let file = buf.into_inner().context("flush buffered writer")?;
            file.sync_all().ok();
        }
        self.finished = true;
        Ok(())
    }
}

impl Drop for SessionWriter {
    fn drop(&mut self) {
        // Best-effort close on drop. Errors here are unrecoverable -- we're
        // already on the way out.
        let _ = self.finish_inner();
    }
}
