//! Size cap for `debug.log`.
//!
//! Every ears invocation appends to `$XDG_RUNTIME_DIR/ears/debug.log`, which
//! lives on tmpfs (RAM). Nothing else ever trims it, so a long-running mode
//! rotates it when it starts: above [`ROTATE_BYTES`] the file becomes
//! `debug.log.1`, replacing an older one, and the starting process opens a
//! fresh `debug.log`. At most two files remain, about 2 x 5 MB of RAM.
//!
//! Short-lived subcommands never call this. Rotation is a rename, so it is
//! safe against processes that have the file open: their writes follow the
//! renamed file, and every later open creates a new `debug.log`. Two owners
//! starting at once take turns on a lock file, and the second sees the
//! fresh small file and leaves it alone.

use crate::lock::FileLock;
use std::io;
use std::path::{Path, PathBuf};

/// Rotate above 5 MB: about 25 days of logging at the rate measured after
/// #271 (~0.2 MB/day), and still small next to the RAM it lives in.
pub const ROTATE_BYTES: u64 = 5 * 1024 * 1024;

/// `debug.log` -> `debug.log.1`.
pub fn rotated_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    PathBuf::from(name)
}

/// Rotate `path` to [`rotated_path`] if it is larger than `limit` bytes.
/// Returns whether it rotated. A missing file, or another process holding
/// the rotation lock, is not an error: nothing is rotated.
pub fn rotate_if_larger(path: &Path, limit: u64) -> io::Result<bool> {
    let mut lock_name = path.as_os_str().to_owned();
    lock_name.push(".lock");
    let mut lock = FileLock::new(PathBuf::from(lock_name)).map_err(io::Error::other)?;
    if !lock.try_lock().map_err(io::Error::other)? {
        // Another owner is rotating right now.
        return Ok(false);
    }
    // Checked under the lock, so a concurrent owner that just rotated is seen.
    let len = match std::fs::metadata(path) {
        Ok(meta) => meta.len(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if len <= limit {
        return Ok(false);
    }
    match std::fs::rename(path, rotated_path(path)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(path: &Path, bytes: usize) {
        std::fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    #[test]
    fn small_log_stays() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("debug.log");
        write(&log, 100);
        assert!(!rotate_if_larger(&log, 1000).unwrap());
        assert_eq!(std::fs::metadata(&log).unwrap().len(), 100);
        assert!(!rotated_path(&log).exists());
    }

    #[test]
    fn missing_log_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!rotate_if_larger(&dir.path().join("debug.log"), 10).unwrap());
    }

    #[test]
    fn large_log_moves_to_dot_one_replacing_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("debug.log");
        let old = rotated_path(&log);
        write(&old, 7);
        write(&log, 2000);
        assert!(rotate_if_larger(&log, 1000).unwrap());
        assert!(!log.exists());
        assert_eq!(std::fs::metadata(&old).unwrap().len(), 2000);
    }

    #[test]
    fn concurrent_appender_keeps_writing_and_new_opens_start_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("debug.log");
        write(&log, 2000);
        let mut appender = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        assert!(rotate_if_larger(&log, 1000).unwrap());
        // The open appender's write lands in the rotated file, not lost.
        appender.write_all(b"late").unwrap();
        assert_eq!(std::fs::metadata(rotated_path(&log)).unwrap().len(), 2004);
        // The next process to open the log gets a new, empty one.
        let mut fresh = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .unwrap();
        fresh.write_all(b"new").unwrap();
        assert_eq!(std::fs::read(&log).unwrap(), b"new");
    }

    #[test]
    fn second_owner_does_not_rotate_the_fresh_log_away() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("debug.log");
        write(&log, 2000);
        assert!(rotate_if_larger(&log, 1000).unwrap());
        write(&log, 10);
        // A second owner starting right after sees the small new file.
        assert!(!rotate_if_larger(&log, 1000).unwrap());
        assert_eq!(std::fs::metadata(rotated_path(&log)).unwrap().len(), 2000);
    }

    #[test]
    fn busy_lock_skips_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("debug.log");
        write(&log, 2000);
        let mut held = FileLock::new(dir.path().join("debug.log.lock")).unwrap();
        assert!(held.try_lock().unwrap());
        assert!(!rotate_if_larger(&log, 1000).unwrap());
        assert!(log.exists());
    }
}
