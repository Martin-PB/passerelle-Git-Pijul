//! Advisory single-writer locks for shared gateway workspaces and Git mirrors.
//!
//! The CLI holds these locks for the full command, not just for individual
//! file writes. OS advisory locks are released automatically when a process
//! exits or crashes; the lock files themselves may remain on disk.

use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// An advisory exclusive lock held for as long as this value is alive.
pub struct WorkspaceLock {
    _file: File,
    path: PathBuf,
}

impl WorkspaceLock {
    /// Lock all mutable state under one gateway work directory.
    pub fn for_workspace(workdir: &Path) -> io::Result<Self> {
        let bridge_dir = workdir.join(".bridge");
        fs::create_dir_all(&bridge_dir)?;
        Self::acquire(&bridge_dir.join("workspace.lock"))
    }

    /// Lock a bare Git mirror against concurrent fetches and reads by the CLI.
    ///
    /// The lock lives beside the mirror, not inside the bare repository.
    pub fn for_mirror(mirror: &Path) -> io::Result<Self> {
        let parent = mirror
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let name = mirror
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("git-mirror"))
            .to_string_lossy();
        fs::create_dir_all(parent)?;
        Self::acquire(&parent.join(format!(".{name}.lock")))
    }

    fn acquire(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;
        file.try_lock_exclusive().map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!(
                        "another passerelle process holds lock {}; retry after it finishes",
                        path.display()
                    ),
                )
            } else {
                error
            }
        })?;
        Ok(Self {
            _file: file,
            path: path.to_path_buf(),
        })
    }

    /// Return the lock file path, primarily for diagnostics.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::WorkspaceLock;
    use std::io;
    use tempfile::tempdir;

    #[test]
    fn workspace_lock_rejects_a_second_writer_and_releases_on_drop() {
        let dir = tempdir().unwrap();
        let first = WorkspaceLock::for_workspace(dir.path()).unwrap();
        let error = WorkspaceLock::for_workspace(dir.path()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        drop(first);
        WorkspaceLock::for_workspace(dir.path()).unwrap();
    }

    #[test]
    fn mirror_lock_is_outside_the_bare_repository() {
        let dir = tempdir().unwrap();
        let mirror = dir.path().join("mirror.git");
        std::fs::create_dir(&mirror).unwrap();
        let lock = WorkspaceLock::for_mirror(&mirror).unwrap();
        assert_eq!(lock.path().parent(), Some(dir.path()));
        assert_ne!(lock.path().parent(), Some(mirror.as_path()));
    }
}
