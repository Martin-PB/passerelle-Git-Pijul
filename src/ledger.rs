//! Ledger: the correspondence map between Git SHAs and Pijul change hashes.
//!
//! The ledger is the gateway's source of truth for idempotence, audit and
//! force-push detection. v0 stores it as a JSON file next to the working
//! repository; the schema is deliberately flat so a SQLite backend can
//! replace it without touching the callers.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
        let ledger: Self = serde_json::from_str(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        ledger.validate()?;
        Ok(ledger)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        self.validate()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let raw = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, raw)
    }

    /// Record an immutable Git-SHA mapping. Returns false for invalid entries
    /// or when a replay changes the Pijul hash, PR attribution, or direction.
    /// The timestamp is intentionally ignored for idempotent re-records.
    pub fn record(&mut self, entry: Entry) -> bool {
        if entry.git_sha.trim().is_empty() || entry.pijul_change.trim().is_empty() {
            return false;
        }
        match self.by_sha.get(&entry.git_sha) {
            Some(existing)
                if existing.pijul_change != entry.pijul_change
                    || existing.pr != entry.pr
                    || existing.direction != entry.direction =>
            {
                false
            }
            Some(_) => true,
            None => {
                self.by_sha.insert(entry.git_sha.clone(), entry);
                true
            }
        }
    }

    /// Validate the serialized key-to-entry identity and required mapping
    /// fields before the ledger is consumed or persisted.
    pub fn validate(&self) -> std::io::Result<()> {
        for (key, entry) in &self.by_sha {
            if key.trim().is_empty()
                || entry.git_sha.trim().is_empty()
                || key != &entry.git_sha
                || entry.pijul_change.trim().is_empty()
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "invalid ledger entry at key {key:?}: key must match a non-empty git_sha and pijul_change must be non-empty"
                    ),
                ));
            }
        }
        Ok(())
    }

    pub fn get(&self, git_sha: &str) -> Option<&Entry> {
        self.by_sha.get(git_sha)
    }
}

/// Default ledger path for a working repository.
pub fn ledger_path(workdir: &Path) -> PathBuf {
    workdir.join("gateway-ledger.json")
}
