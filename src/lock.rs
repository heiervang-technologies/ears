use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during lock operations
#[derive(Debug, Error)]
pub enum LockError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Failed to acquire lock: another instance is running")]
    LockHeld,

    #[error("flock system call failed: {0}")]
    FlockFailed(String),
}

/// A file-based lock using flock, similar to the Bash implementation
pub struct FileLock {
    file: File,
    lock_path: PathBuf,
    held: bool,
}

impl FileLock {
    /// Create a new FileLock
    pub fn new<P: AsRef<Path>>(lock_path: P) -> Result<Self, LockError> {
        let lock_path = lock_path.as_ref().to_path_buf();

        // Ensure parent directory exists
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Open or create the lock file
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .open(&lock_path)?;

        Ok(Self {
            file,
            lock_path,
            held: false,
        })
    }

    /// Try to acquire the lock (non-blocking)
    /// Returns Ok(true) if lock was acquired, Ok(false) if lock is held by another process
    pub fn try_lock(&mut self) -> Result<bool, LockError> {
        if self.held {
            return Ok(true);
        }

        // Use flock with LOCK_EX | LOCK_NB for non-blocking exclusive lock
        let fd = self.file.as_raw_fd();
        let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

        if result == 0 {
            self.held = true;
            Ok(true)
        } else {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                // Lock is held by another process
                Ok(false)
            } else {
                Err(LockError::FlockFailed(err.to_string()))
            }
        }
    }

    /// Acquire the lock (blocking)
    pub fn lock(&mut self) -> Result<(), LockError> {
        if self.held {
            return Ok(());
        }

        // Use flock with LOCK_EX for blocking exclusive lock
        let fd = self.file.as_raw_fd();
        let result = unsafe { libc::flock(fd, libc::LOCK_EX) };

        if result == 0 {
            self.held = true;
            Ok(())
        } else {
            Err(LockError::FlockFailed(
                io::Error::last_os_error().to_string(),
            ))
        }
    }

    /// Release the lock
    pub fn unlock(&mut self) -> Result<(), LockError> {
        if !self.held {
            return Ok(());
        }

        let fd = self.file.as_raw_fd();
        let result = unsafe { libc::flock(fd, libc::LOCK_UN) };

        if result == 0 {
            self.held = false;
            Ok(())
        } else {
            Err(LockError::FlockFailed(
                io::Error::last_os_error().to_string(),
            ))
        }
    }

    /// Check if the lock is currently held by this instance
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Get the path to the lock file
    pub fn path(&self) -> &Path {
        &self.lock_path
    }

    /// Clean up stale lock file
    /// This is safe because flock locks are automatically released when the process exits
    pub fn cleanup_stale(&self) -> Result<(), LockError> {
        // flock locks are automatically released when the file descriptor is closed
        // or when the process exits, so we don't need to do anything special here.
        // The lock file can persist, but it won't prevent other processes from acquiring the lock.
        Ok(())
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Unlock when the FileLock is dropped
        let _ = self.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;
    use tempfile::TempDir;

    #[test]
    fn test_lock_acquisition() {
        let temp_dir = TempDir::new().unwrap();
        let lock_path = temp_dir.path().join("test.lock");

        let mut lock = FileLock::new(&lock_path).unwrap();
        assert!(!lock.is_held());

        // Should be able to acquire lock
        assert!(lock.try_lock().unwrap());
        assert!(lock.is_held());

        // Should still be held
        assert!(lock.try_lock().unwrap());
        assert!(lock.is_held());
    }

    #[test]
    fn test_lock_release() {
        let temp_dir = TempDir::new().unwrap();
        let lock_path = temp_dir.path().join("test.lock");

        let mut lock = FileLock::new(&lock_path).unwrap();
        lock.try_lock().unwrap();
        assert!(lock.is_held());

        lock.unlock().unwrap();
        assert!(!lock.is_held());
    }

    #[test]
    fn test_concurrent_locks() {
        let temp_dir = TempDir::new().unwrap();
        let lock_path = temp_dir.path().join("test.lock");

        // First lock acquires successfully
        let mut lock1 = FileLock::new(&lock_path).unwrap();
        assert!(lock1.try_lock().unwrap());

        // Second lock on same file should fail
        let mut lock2 = FileLock::new(&lock_path).unwrap();
        assert!(!lock2.try_lock().unwrap());

        // After releasing first lock, second should succeed
        lock1.unlock().unwrap();
        assert!(lock2.try_lock().unwrap());
    }

    #[test]
    fn test_lock_auto_release_on_drop() {
        let temp_dir = TempDir::new().unwrap();
        let lock_path = temp_dir.path().join("test.lock");

        {
            let mut lock1 = FileLock::new(&lock_path).unwrap();
            lock1.try_lock().unwrap();
            // lock1 drops here
        }

        // Should be able to acquire after drop
        let mut lock2 = FileLock::new(&lock_path).unwrap();
        assert!(lock2.try_lock().unwrap());
    }

    #[test]
    fn test_multi_threaded_locks() {
        let temp_dir = TempDir::new().unwrap();
        let lock_path = Arc::new(temp_dir.path().join("test.lock"));
        let barrier = Arc::new(Barrier::new(3));
        let success_count = Arc::new(std::sync::Mutex::new(0));

        let mut handles = vec![];

        for _ in 0..3 {
            let lock_path = Arc::clone(&lock_path);
            let barrier = Arc::clone(&barrier);
            let success_count = Arc::clone(&success_count);

            let handle = thread::spawn(move || {
                // Wait for all threads to start
                barrier.wait();

                let mut lock = FileLock::new(lock_path.as_ref()).unwrap();
                if lock.try_lock().unwrap() {
                    // Hold the lock briefly
                    thread::sleep(std::time::Duration::from_millis(10));

                    let mut count = success_count.lock().unwrap();
                    *count += 1;

                    lock.unlock().unwrap();
                }
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Only one thread should have acquired the lock initially
        let final_count = *success_count.lock().unwrap();
        assert_eq!(final_count, 1);
    }

    #[test]
    fn test_blocking_lock() {
        let temp_dir = TempDir::new().unwrap();
        let lock_path = Arc::new(temp_dir.path().join("test.lock"));
        let barrier = Arc::new(Barrier::new(2));

        let lock_path_clone = Arc::clone(&lock_path);
        let barrier_clone = Arc::clone(&barrier);

        // Thread 1: Acquire lock, wait, then release
        let handle = thread::spawn(move || {
            let mut lock = FileLock::new(lock_path_clone.as_ref()).unwrap();
            lock.lock().unwrap();

            // Signal that we have the lock
            barrier_clone.wait();

            // Hold for a bit
            thread::sleep(std::time::Duration::from_millis(50));

            lock.unlock().unwrap();
        });

        // Wait for thread 1 to acquire lock
        barrier.wait();

        // Thread 2: Try to acquire lock (should succeed after thread 1 releases)
        let start = std::time::Instant::now();
        let mut lock2 = FileLock::new(lock_path.as_ref()).unwrap();

        // This should block until thread 1 releases
        lock2.lock().unwrap();
        let elapsed = start.elapsed();

        // Should have waited at least a bit
        assert!(elapsed.as_millis() >= 40);

        handle.join().unwrap();
    }
}
