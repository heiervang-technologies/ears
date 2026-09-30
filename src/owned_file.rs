//! Best-effort cleanup of a file owned by this operation, not a replacement path.
use anyhow::{Context, Result};
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(crate) struct CleanupFile {
    path: PathBuf,
    identity: Option<(u64, u64)>,
    // Pin the inode until cleanup finishes so it cannot be recycled.
    _file: Option<std::fs::File>,
}

impl CleanupFile {
    pub(crate) fn new(path: &Path) -> Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_PATH | libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(file) => Some(file),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Cannot own cleanup for {}", path.display()))
            }
        };
        let identity = file
            .as_ref()
            .map(|file| {
                file.metadata()
                    .map(|metadata| (metadata.dev(), metadata.ino()))
            })
            .transpose()
            .context("Cannot inspect owned audio file")?;
        Ok(Self {
            path: path.to_owned(),
            identity,
            _file: file,
        })
    }
}

impl Drop for CleanupFile {
    fn drop(&mut self) {
        let Some(identity) = self.identity else {
            return;
        };
        match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) if (metadata.dev(), metadata.ino()) == identity => {
                if let Err(error) = std::fs::remove_file(&self.path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(path = %self.path.display(), %error, "Failed to clean owned audio file");
                    }
                }
            }
            Ok(_) => {} // Another operation owns this path now.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(path = %self.path.display(), %error, "Failed to inspect owned audio file")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_removes_owned_file_but_preserves_replacements() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("recording.wav");
        std::fs::write(&path, "owned").unwrap();
        drop(CleanupFile::new(&path).unwrap());
        assert!(!path.exists());
        std::fs::write(&path, "owned").unwrap();
        let guard = CleanupFile::new(&path).unwrap();
        std::fs::rename(&path, root.path().join("moved.wav")).unwrap();
        std::fs::write(&path, "replacement").unwrap();
        drop(guard);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "replacement");
        let missing = root.path().join("absent.wav");
        let guard = CleanupFile::new(&missing).unwrap();
        std::fs::write(&missing, "new owner").unwrap();
        drop(guard);
        assert!(missing.exists());
    }

    #[tokio::test]
    async fn cancellation_drops_owned_audio() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("recording.wav");
        std::fs::write(&path, "owned").unwrap();
        let guard = CleanupFile::new(&path).unwrap();
        let task = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!path.exists());
    }
}
