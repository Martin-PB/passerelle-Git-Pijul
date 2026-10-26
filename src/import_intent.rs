//! Durable intent records for Git-to-Pijul imports.
//!
//! A process can die after `pijul git` mutates the Pijul repository but before
//! the bridge records the resulting change hash. An intent makes that window
//! visible on the next run. It is deliberately fail-closed: if an intent has
//! no matching ledger entry, the bridge stops and asks for explicit recovery
//! rather than replaying an import whose effects may already exist.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

fn is_full_git_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    OpenOptions::new().read(true).open(path)?.sync_all()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportIntent {
    pub git_sha: String,
    pub parent_sha: String,
    pub base_sha: String,
    pub pr: u64,
    /// Filled after Pijul's before/after observation identifies the change.
    #[serde(default)]
    pub pijul_change: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImportIntentStore {
    root: PathBuf,
}

impl ImportIntentStore {
    pub fn new(workdir: &Path) -> Self {
        Self {
            root: workdir.join(".bridge").join("import-intents"),
        }
    }

    fn path_for(&self, git_sha: &str) -> io::Result<PathBuf> {
        // Git commit OIDs are hexadecimal. Validate before using an identifier
        // as a path component, including in recovery and cleanup operations.
        if !is_full_git_oid(git_sha) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "import intent requires a full hexadecimal Git commit OID",
            ));
        }
        Ok(self.root.join(format!("{git_sha}.json")))
    }

    /// Read every pending intent in the workspace. A pending intent for a
    /// different SHA is still relevant: it may describe an earlier mutation
    /// whose result was never committed to the ledger.
    pub fn pending_intents(&self) -> io::Result<Vec<ImportIntent>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut intents = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let git_sha = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| io::Error::new(
                    io::ErrorKind::InvalidData,
                    "import intent filename is not valid UTF-8",
                ))?;
            let intent = self.load(git_sha)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("import intent {git_sha:?} disappeared while being read"),
                )
            })?;
            intents.push(intent);
        }
        intents.sort_by(|left, right| left.git_sha.cmp(&right.git_sha));
        Ok(intents)
    }

    pub fn load(&self, git_sha: &str) -> io::Result<Option<ImportIntent>> {
        let path = self.path_for(git_sha)?;
        let mut file = match OpenOptions::new().read(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut raw = String::new();
        file.read_to_string(&mut raw)?;
        let intent: ImportIntent = serde_json::from_str(&raw)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if intent.git_sha != git_sha {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "import intent filename {git_sha:?} does not match its recorded SHA {:?}",
                    intent.git_sha
                ),
            ));
        }
        if !is_full_git_oid(&intent.parent_sha) || !is_full_git_oid(&intent.base_sha) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("import intent for {git_sha} contains an invalid parent or base OID"),
            ));
        }
        if matches!(intent.pijul_change.as_deref(), Some(change) if change.trim().is_empty()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("import intent for {git_sha} contains an empty Pijul change hash"),
            ));
        }
        Ok(Some(intent))
    }

    /// Refuse to retry a SHA while an earlier attempt has no committed
    /// ledger mapping. The caller may only clear that intent after an explicit
    /// recovery has established the exact mapping.
    pub fn require_no_uncommitted_intent(&self, git_sha: &str) -> io::Result<()> {
        if self.load(git_sha)?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                format!(
                    "Git SHA {git_sha} has a pending import intent but no ledger mapping; inspect Pijul state and recover explicitly before retrying"
                ),
            ));
        }
        Ok(())
    }

    /// Persist the observed change identity before committing its ledger
    /// mapping. A crash after this step can be reconciled by comparing the
    /// exact change hash rather than trusting only SHA/PR provenance.
    pub fn mark_mapped(&self, intent: &ImportIntent) -> io::Result<()> {
        let Some(pijul_change) = intent.pijul_change.as_deref() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "mapped import intent requires a Pijul change hash",
            ));
        };
        if pijul_change.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Pijul change hash in import intent must not be empty",
            ));
        }
        let path = self.path_for(&intent.git_sha)?;
        let Some(persisted) = self.load(&intent.git_sha)? else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("cannot update missing import intent for {}", intent.git_sha),
            ));
        };
        if persisted.git_sha != intent.git_sha
            || persisted.parent_sha != intent.parent_sha
            || persisted.base_sha != intent.base_sha
            || persisted.pr != intent.pr
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("import intent identity changed while mapping {}", intent.git_sha),
            ));
        }
        if let Some(existing) = persisted.pijul_change.as_deref() {
            if existing == pijul_change && &persisted == intent {
                return Ok(());
            }
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("import intent for {} already records a different mapping", intent.git_sha),
            ));
        }

        let temp = self.root.join(format!(
            ".{}.{}.{}.tmp",
            intent.git_sha,
            std::process::id(),
            TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let raw = serde_json::to_vec_pretty(intent)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temp)?;
        file.write_all(&raw)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, &path)?;
        sync_directory(&self.root)?;
        Ok(())
    }

    /// Persist the intent before mutating the Pijul repository. create_new
    /// prevents two bridge processes from silently claiming the same SHA.
    pub fn begin(&self, intent: &ImportIntent) -> io::Result<()> {
        let path = self.path_for(&intent.git_sha)?;
        if !is_full_git_oid(&intent.parent_sha) || !is_full_git_oid(&intent.base_sha) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "import intent requires full hexadecimal parent and base commit OIDs",
            ));
        }
        if intent.pijul_change.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "new import intent must not contain a Pijul change before import",
            ));
        }
        fs::create_dir_all(&self.root)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let raw = serde_json::to_vec_pretty(intent)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        file.write_all(&raw)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        // Persist the directory entry before allowing the caller to mutate Pijul.
        sync_directory(&self.root)?;
        Ok(())
    }

    /// Remove an intent only when it matches the mapping that was persisted.
    /// If the ledger was saved but the process died before cleanup, this is
    /// safe to call on restart. A mismatch is never silently discarded.
    pub fn complete(&self, intent: &ImportIntent) -> io::Result<()> {
        if !matches!(intent.pijul_change.as_deref(), Some(change) if !change.trim().is_empty()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot complete an import intent before recording its Pijul change hash",
            ));
        }
        let Some(persisted) = self.load(&intent.git_sha)? else {
            return Ok(());
        };
        if &persisted != intent {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("persisted import intent for {} does not match the current import", intent.git_sha),
            ));
        }
        fs::remove_file(self.path_for(&intent.git_sha)?)?;
        sync_directory(&self.root)?;
        Ok(())
    }

    /// Used when the ledger already contains the mapping. Only the exact
    /// matching intent may be cleared; otherwise recovery must stop.
    pub fn reconcile_committed(
        &self,
        intent: &ImportIntent,
        ledger_sha: &str,
        ledger_pr: Option<u64>,
        ledger_change: &str,
    ) -> io::Result<()> {
        if ledger_sha != intent.git_sha
            || ledger_pr != Some(intent.pr)
            || intent.pijul_change.as_deref() != Some(ledger_change)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("ledger entry for {} does not match the SHA, PR or Pijul change recorded in its pending import intent", intent.git_sha),
            ));
        }
        self.complete(intent)
    }
}
