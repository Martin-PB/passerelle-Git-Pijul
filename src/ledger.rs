//! Ledger: the correspondence map between Git SHAs and Pijul change hashes.
//!
//! The ledger is the gateway's source of truth for idempotence, audit and
//! force-push detection. v0 stores it as a JSON file next to the working
//! repository; the schema is deliberately flat so a SQLite backend can
//! replace it without touching the callers.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub git_sha: String,
    pub pijul_change: String,
    /// Which PR this came from, if any.
    pub pr: Option<u64>,
    /// How the change entered Pijul (import) or returned to Git (export).
    pub direction: Direction,
    /// Version-0 timestamp marker (`unix:<seconds>`).
    pub recorded_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    GitToPijul,
    PijulToGit,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(flatten)]
    pub by_sha: BTreeMap<String, Entry>,
}

impl Ledger {
    pub fn load(path: &Path) -> std::io::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)?;
        serde_json::from_str(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Persist the ledger with an atomic same-directory replacement.
    ///
    /// A process interruption during serialization or writing must not leave
    /// the canonical ledger truncated: write and sync a unique temporary file,
    /// then rename it over the destination.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;

        let raw = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let filename = path.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "ledger path must name a file",
            )
        })?;

        let (temporary_path, mut temporary_file) = loop {
            let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let temporary_path = parent.join(format!(
                ".{}.{}.{}.tmp",
                filename.to_string_lossy(),
                std::process::id(),
                id
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary_path)
            {
                Ok(file) => break (temporary_path, file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        };

        let result = (|| {
            temporary_file.write_all(raw.as_bytes())?;
            temporary_file.sync_all()?;
            drop(temporary_file);
            std::fs::rename(&temporary_path, path)?;
            // Make the rename durable across a host crash on filesystems that
            // support syncing directory metadata (the intended deployment is Linux).
            std::fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary_path);
        }
        result
    }

    /// Record a translation. Returns false if this SHA was already ledged
    /// with a DIFFERENT change hash, i.e. the Git side moved under us
    /// (force-push detection).
    pub fn record(&mut self, entry: Entry) -> bool {
        match self.by_sha.get(&entry.git_sha) {
            Some(existing) if existing.pijul_change != entry.pijul_change => false,
            Some(_) => true, // idempotent re-record
            None => {
                self.by_sha.insert(entry.git_sha.clone(), entry);
                true
            }
        }
    }

    pub fn get(&self, git_sha: &str) -> Option<&Entry> {
        self.by_sha.get(git_sha)
    }
}

/// Default ledger path for a working repository.
pub fn ledger_path(workdir: &Path) -> PathBuf {
    workdir.join("gateway-ledger.json")
}
