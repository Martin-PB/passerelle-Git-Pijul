//! Durable marker for the initial Git-base import into Pijul.
//!
//! A base import is not an ordinary PR delta and has no normal Git-SHA ->
//! Pijul-change ledger entry. It therefore uses a separate journal namespace.
//! Any marker left by an interrupted base import quarantines the workspace;
//! the importer must not retry the mutation blindly.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

fn is_full_git_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    OpenOptions::new().read(true).open(path)?.sync_all()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseImportIntent {
    pub base_sha: String,
}

#[derive(Debug, Clone)]
pub struct BaseImportIntentStore {
    root: PathBuf,
}

impl BaseImportIntentStore {
    pub fn new(workdir: &Path) -> Self {
        Self {
            root: workdir.join(".bridge").join("base-import-intents"),
        }
    }

    fn path_for(&self, base_sha: &str) -> io::Result<PathBuf> {
        if !is_full_git_oid(base_sha) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "base import intent requires a full hexadecimal Git commit OID",
            ));
        }
        Ok(self.root.join(format!("{base_sha}.json")))
    }

    fn load(&self, base_sha: &str) -> io::Result<Option<BaseImportIntent>> {
        let path = self.path_for(base_sha)?;
        let mut file = match OpenOptions::new().read(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut raw = String::new();
        file.read_to_string(&mut raw)?;
        let intent: BaseImportIntent = serde_json::from_str(&raw)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if intent.base_sha != base_sha || !is_full_git_oid(&intent.base_sha) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("base import intent filename does not match its recorded OID {base_sha:?}"),
            ));
        }
        Ok(Some(intent))
    }

    /// Enumerate every unfinished base import. Any malformed marker is an
    /// error, not something the importer may skip.
    pub fn pending(&self) -> io::Result<Vec<BaseImportIntent>> {
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
            let base_sha = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "base import intent filename is not valid UTF-8",
                    )
                })?;
            let intent = self.load(base_sha)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("base import intent {base_sha:?} disappeared while being read"),
                )
            })?;
            intents.push(intent);
        }
        intents.sort_by(|left, right| left.base_sha.cmp(&right.base_sha));
        Ok(intents)
    }

    /// Write and sync the marker before the first mutating Pijul command.
    pub fn begin(&self, base_sha: &str) -> io::Result<()> {
        let path = self.path_for(base_sha)?;
        fs::create_dir_all(&self.root)?;
        let intent = BaseImportIntent {
            base_sha: base_sha.to_owned(),
        };
        let raw = serde_json::to_vec_pretty(&intent)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(&raw)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        sync_directory(&self.root)
    }

    /// Clear the marker only after the stable base channel has been created.
    /// A missing marker is harmless when the base channel was already present.
    pub fn complete(&self, base_sha: &str) -> io::Result<()> {
        let Some(intent) = self.load(base_sha)? else {
            return Ok(());
        };
        if intent.base_sha != base_sha {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "base import intent does not match the completed base",
            ));
        }
        fs::remove_file(self.path_for(base_sha)?)?;
        sync_directory(&self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::BaseImportIntentStore;
    use std::io::ErrorKind;

    const BASE: &str = "fedcba9876543210fedcba9876543210fedcba98";

    #[test]
    fn base_intent_is_durable_until_stable_base_is_completed() {
        let dir = tempfile::tempdir().unwrap();
        let store = BaseImportIntentStore::new(dir.path());

        store.begin(BASE).unwrap();
        assert_eq!(store.pending().unwrap().len(), 1);
        assert_eq!(store.pending().unwrap()[0].base_sha, BASE);

        let duplicate = store.begin(BASE).unwrap_err();
        assert_eq!(duplicate.kind(), ErrorKind::AlreadyExists);

        store.complete(BASE).unwrap();
        assert!(store.pending().unwrap().is_empty());
    }

    #[test]
    fn malformed_base_intent_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let store = BaseImportIntentStore::new(dir.path());
        let root = dir.path().join(".bridge").join("base-import-intents");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(format!("{BASE}.json")), "{not json").unwrap();

        assert_eq!(store.pending().unwrap_err().kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_non_oid_base_paths() {
        let dir = tempfile::tempdir().unwrap();
        let store = BaseImportIntentStore::new(dir.path());
        assert_eq!(store.begin("../outside").unwrap_err().kind(), ErrorKind::InvalidInput);
    }
}
