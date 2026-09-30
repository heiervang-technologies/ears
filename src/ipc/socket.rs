//! A socket pathname belongs to one listener for its entire lifetime.
use crate::lock::FileLock;
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};

pub(super) struct OwnedSocket {
    pub listener: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
    // Never unlink the lock file: doing so would create two independent locks.
    _lock: FileLock,
}

impl OwnedSocket {
    pub async fn bind(path: PathBuf) -> io::Result<Self> {
        let mut lock_path = path.as_os_str().to_owned();
        lock_path.push(".lock");
        let mut lock = FileLock::new(PathBuf::from(lock_path)).map_err(io::Error::other)?;
        if !lock.try_lock().map_err(io::Error::other)? {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "IPC socket owner is active",
            ));
        }
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.file_type().is_socket() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "IPC path is not a socket",
                    ));
                }
                // Also protect listeners from older Ears versions, which have no lock.
                match tokio::time::timeout(Duration::from_millis(200), UnixStream::connect(&path))
                    .await
                {
                    Ok(Err(e))
                        if matches!(
                            e.kind(),
                            io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                        ) => {}
                    Ok(Ok(_)) => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            "IPC listener is active",
                        ))
                    }
                    Ok(Err(e)) => return Err(e),
                    Err(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Cannot establish that IPC socket is stale",
                        ))
                    }
                }
                match std::fs::symlink_metadata(&path) {
                    Ok(now) if (now.dev(), now.ino()) == (metadata.dev(), metadata.ino()) => {
                        std::fs::remove_file(&path)?
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            "IPC path changed during startup",
                        ))
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(&path)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        Ok(Self {
            listener,
            path,
            identity: (metadata.dev(), metadata.ino()),
            _lock: lock,
        })
    }
}

impl Drop for OwnedSocket {
    fn drop(&mut self) {
        // A late shutdown must never remove a replacement socket.
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|m| m.file_type().is_socket() && (m.dev(), m.ino()) == self.identity)
        {
            if let Err(e) = std::fs::remove_file(&self.path) {
                tracing::warn!(path = %self.path.display(), "Cannot remove owned IPC socket: {e}");
            }
        }
    }
}
