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
#[derive(Debug)]
pub struct WorkspaceLock {
    _file: File,
    path: PathBuf,
}

impl WorkspaceLock {
    /// Lock all mutable state under one gateway work directory.
    pub fn for_workspace(workdir: &Path) -> io::Result<Self> {
        let bridge_dir = workdir.join(".bridge");
        fs::create_dir_all(&bridge_dir)?;
        // Canonicalize after creation so relative/absolute paths and symlink
        // aliases for the same workspace share one lock identity.
        let canonical_workdir = fs::canonicalize(workdir)?;
        Self::acquire(&canonical_workdir.join(".bridge").join("workspace.lock"))
    }

    /// Lock a bare Git mirror against concurrent fetches and reads by the CLI.
    ///
    /// The lock lives beside the mirror, not inside the bare repository.
    pub fn for_mirror(mirror: &Path) -> io::Result<Self> {
        let requested_parent = mirror
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(requested_parent)?;

        // If the mirror already exists, canonicalize the mirror itself so a
        // symlink alias resolves to the same lock as the target. For a mirror
        // that has not been created yet, canonicalize its parent and append the
        // requested final component. Broken symlinks and other metadata errors
        // fail closed rather than creating a second lock identity.
        let normalized_mirror = match fs::symlink_metadata(mirror) {
            Ok(_) => fs::canonicalize(mirror)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let parent = fs::canonicalize(requested_parent)?;
                let name = mirror
                    .file_name()
                    .ok_or_else(|| io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Git mirror path must have a final path component",
                    ))?;
                parent.join(name)
            }
            Err(error) => return Err(error),
        };
        let parent = normalized_mirror
            .parent()
            .ok_or_else(|| io::Error::new(
                io::ErrorKind::InvalidInput,
                "normalized Git mirror path has no parent",
            ))?;
        let name = normalized_mirror
            .file_name()
            .ok_or_else(|| io::Error::new(
                io::ErrorKind::InvalidInput,
                "normalized Git mirror path has no final component",
            ))?
            .to_string_lossy();
        Self::acquire(&parent.join(format!("{name}.lock")))
    }

    fn acquire(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
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

    #[cfg(unix)]
    #[test]
    fn workspace_lock_normalizes_symlink_aliases() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let real = dir.path().join("real-workspace");
        let alias = dir.path().join("workspace-alias");
        std::fs::create_dir(&real).unwrap();
        symlink(&real, &alias).unwrap();

        let first = WorkspaceLock::for_workspace(&real).unwrap();
        let error = WorkspaceLock::for_workspace(&alias).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        drop(first);
    }

    #[cfg(unix)]
    #[test]
    fn mirror_lock_normalizes_symlink_aliases() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let mirror = dir.path().join("mirror.git");
        let alias = dir.path().join("mirror-alias.git");
        std::fs::create_dir(&mirror).unwrap();
        symlink(&mirror, &alias).unwrap();

        let first = WorkspaceLock::for_mirror(&mirror).unwrap();
        let error = WorkspaceLock::for_mirror(&alias).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        drop(first);
    }

    #[test]
    fn mirror_lock_normalizes_relative_and_absolute_paths() {
        let dir = tempdir().unwrap();
        let mirror = dir.path().join("mirror.git");
        std::fs::create_dir(&mirror).unwrap();
        let absolute = WorkspaceLock::for_mirror(&mirror).unwrap();
        let relative_path = std::env::current_dir().unwrap().join(&mirror);
        let error = WorkspaceLock::for_mirror(&relative_path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        drop(absolute);
    }
}
