use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use uuid::Uuid;

use super::meta::Meta;

/// Filename for the compressed NDJSON event log within a session directory.
pub const LOG_FILENAME: &str = "log.ndjson.zst";

/// Filename for the per-session metadata document.
pub const META_FILENAME: &str = "meta.json";

/// Sub-directory where hook processes drop event files for the recorder.
pub const DROPBOX_DIRNAME: &str = "dropbox";

/// Filesystem layout for a single session.
///
/// `SessionStore` is purely about *paths*: it doesn't open files. Use
/// `SessionWriter` / `SessionReader` for IO.
#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
    session_id: Uuid,
}

impl SessionStore {
    /// Build a `SessionStore` for an existing or to-be-created session
    /// directory at `<root>/sessions/<session_id>/`.
    pub fn new(root: impl Into<PathBuf>, session_id: Uuid) -> Self {
        Self {
            root: root.into(),
            session_id,
        }
    }

    /// Create the session directory skeleton on disk:
    /// `<root>/sessions/<id>/dropbox/`. Permissions are 0700 on Unix.
    pub fn create(&self) -> Result<()> {
        let dir = self.session_dir();
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        fs::create_dir_all(self.dropbox_dir())
            .with_context(|| format!("create {}", self.dropbox_dir().display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("chmod 0700 {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    pub fn session_dir(&self) -> PathBuf {
        self.root.join("sessions").join(self.session_id.to_string())
    }

    pub fn log_path(&self) -> PathBuf {
        self.session_dir().join(LOG_FILENAME)
    }

    pub fn meta_path(&self) -> PathBuf {
        self.session_dir().join(META_FILENAME)
    }

    pub fn dropbox_dir(&self) -> PathBuf {
        self.session_dir().join(DROPBOX_DIRNAME)
    }

    /// Write `meta.json` atomically: write to a sibling temp file, then rename.
    pub fn write_meta(&self, meta: &Meta) -> Result<()> {
        let target = self.meta_path();
        let tmp = self.session_dir().join(".meta.json.tmp");
        let bytes = serde_json::to_vec_pretty(meta).context("serialize meta")?;
        let mut f = fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(&bytes)
            .with_context(|| format!("write {}", tmp.display()))?;
        f.sync_data().ok();
        fs::rename(&tmp, &target)
            .with_context(|| format!("rename {} -> {}", tmp.display(), target.display()))?;
        Ok(())
    }

    /// Read `meta.json`. Returns an error if missing or malformed.
    pub fn read_meta(&self) -> Result<Meta> {
        let p = self.meta_path();
        let bytes = fs::read(&p).with_context(|| format!("read {}", p.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", p.display()))
    }

    /// Test helper: list session directories under `<root>/sessions/`.
    pub fn list_session_dirs(root: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
        let dir = root.as_ref().join("sessions");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in fs::read_dir(&dir).with_context(|| format!("readdir {}", dir.display()))? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                out.push(entry.path());
            }
        }
        Ok(out)
    }
}
