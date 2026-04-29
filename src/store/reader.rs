use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::format::{Event, SessionHeader};

/// Outcome of reading a log file.
///
/// `is_partial == true` means the trailing zstd frame was truncated (typically
/// because the recorder crashed) and we returned the events from complete
/// frames. The data we *do* return is structurally intact.
#[derive(Debug, Clone)]
pub struct ReadResult {
    pub header: SessionHeader,
    pub events: Vec<Event>,
    pub is_partial: bool,
}

/// Tolerant reader for `log.ndjson.zst`.
///
/// Walks the file frame by frame; if a final frame is truncated mid-block,
/// stops there and returns what survived complete frames. Never panics on
/// truncation.
pub struct SessionReader;

impl SessionReader {
    pub fn read(path: impl AsRef<Path>) -> Result<ReadResult> {
        let path = path.as_ref();
        let file = File::open(path).with_context(|| format!("open log {}", path.display()))?;
        let buf = BufReader::new(file);
        // The zstd Decoder transparently concatenates concatenated frames.
        // On a truncated trailing frame it surfaces an io::Error which we
        // catch below to salvage everything that decoded cleanly so far.
        let decoder = zstd::stream::read::Decoder::new(buf).context("init zstd decoder")?;
        let mut reader = BufReader::new(decoder);
        let mut header: Option<SessionHeader> = None;
        let mut events: Vec<Event> = Vec::new();
        let mut is_partial = false;

        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break, // clean EOF
                Ok(_) => {
                    let trimmed = line.trim_end_matches(['\n', '\r']);
                    if trimmed.is_empty() {
                        continue;
                    }
                    if header.is_none() {
                        header = Some(
                            serde_json::from_str(trimmed)
                                .with_context(|| format!("parse header: {trimmed}"))?,
                        );
                    } else {
                        match serde_json::from_str::<Event>(trimmed) {
                            Ok(e) => events.push(e),
                            Err(_) => {
                                // A line we can't parse means either a partial
                                // last line from a truncated frame or an
                                // unknown event kind. Treat as partial and stop.
                                is_partial = true;
                                break;
                            }
                        }
                    }
                }
                Err(_) => {
                    // zstd will surface a truncation here. Salvage what we have.
                    is_partial = true;
                    break;
                }
            }
        }

        let header = header.ok_or_else(|| anyhow!("log file is empty -- no header"))?;
        Ok(ReadResult {
            header,
            events,
            is_partial,
        })
    }
}
