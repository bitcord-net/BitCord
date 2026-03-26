use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};
use thiserror::Error;
use tracing::debug;

/// Error returned when available storage is insufficient.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage limit exceeded: need {required} bytes but only {available} bytes available")]
    LimitExceeded { required: u64, available: u64 },
}

/// Guards disk usage for BitCord's data directory, enforcing a configurable
/// soft ceiling and supporting LRU eviction of old channel saves.
pub struct StorageGuard {
    data_dir: PathBuf,
    limit_bytes: u64,
    used_bytes: Arc<AtomicU64>,
}

impl StorageGuard {
    /// Create a guard for `data_dir` with `limit_mb` megabytes of storage.
    ///
    /// Performs an initial directory scan to populate the usage counter.
    pub fn new(data_dir: PathBuf, limit_mb: u64) -> Self {
        let used = dir_size(&data_dir);
        debug!(
            "StorageGuard: {used} / {} bytes in {:?}",
            limit_mb * 1024 * 1024,
            data_dir
        );
        Self {
            data_dir,
            limit_bytes: limit_mb * 1024 * 1024,
            used_bytes: Arc::new(AtomicU64::new(used)),
        }
    }

    /// Current cached disk usage in bytes.
    pub fn used_bytes(&self) -> u64 {
        self.used_bytes.load(Ordering::Relaxed)
    }

    /// Remaining capacity in bytes under the configured limit.
    pub fn available_bytes(&self) -> u64 {
        self.limit_bytes.saturating_sub(self.used_bytes())
    }

    /// Re-scan the data directory and refresh the cached usage counter.
    pub fn refresh_usage(&self) {
        let used = dir_size(&self.data_dir);
        self.used_bytes.store(used, Ordering::Relaxed);
    }

    /// Returns `Ok(())` if at least `required_bytes` are available below the
    /// limit, or `Err(StorageError::LimitExceeded)` otherwise.
    pub fn check_available(&self, required_bytes: u64) -> Result<(), StorageError> {
        let available = self.available_bytes();
        if required_bytes > available {
            return Err(StorageError::LimitExceeded {
                required: required_bytes,
                available,
            });
        }
        Ok(())
    }

    /// Record that `bytes` were written to disk, updating the usage counter.
    pub fn record_write(&self, bytes: u64) {
        self.used_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Record that `bytes` were freed from disk (e.g. after eviction).
    pub fn record_free(&self, bytes: u64) {
        self.used_bytes
            .fetch_sub(bytes.min(self.used_bytes()), Ordering::Relaxed);
    }

    /// Returns `true` when usage is at or above 90% of the limit (eviction
    /// threshold).
    pub fn near_limit(&self) -> bool {
        self.used_bytes() >= self.limit_bytes * 9 / 10
    }

    /// Returns paths of `.amrg` files under the data dir sorted by modification
    /// time ascending (oldest first). The caller should delete or truncate these
    /// to free space.
    pub fn oldest_channel_files(&self) -> Vec<PathBuf> {
        let mut entries: Vec<(SystemTime, PathBuf)> = Vec::new();
        collect_amrg(&self.data_dir, &mut entries);
        entries.sort_by_key(|(t, _)| *t);
        entries.into_iter().map(|(_, p)| p).collect()
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn dir_size(dir: &Path) -> u64 {
    if !dir.exists() {
        return 0;
    }
    walk_size(dir)
}

fn walk_size(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += walk_size(&path);
        } else if let Ok(meta) = path.metadata() {
            total += meta.len();
        }
    }
    total
}

fn collect_amrg(dir: &Path, out: &mut Vec<(SystemTime, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_amrg(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("amrg") {
            if let Ok(meta) = path.metadata() {
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                out.push((mtime, path));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn storage_limit_passes_within_limit() {
        let tmp = TempDir::new().unwrap();
        let guard = StorageGuard::new(tmp.path().to_path_buf(), 1);
        // 512 KiB < 1 MiB — should pass
        guard.check_available(512 * 1024).unwrap();
    }

    #[test]
    fn storage_limit_exceeded_returns_error() {
        let tmp = TempDir::new().unwrap();
        let guard = StorageGuard::new(tmp.path().to_path_buf(), 1);
        // 2 MiB > 1 MiB — should fail
        let err = guard.check_available(2 * 1024 * 1024);
        assert!(matches!(err, Err(StorageError::LimitExceeded { .. })));
    }

    #[test]
    fn near_limit_detects_threshold() {
        let tmp = TempDir::new().unwrap();
        let guard = StorageGuard::new(tmp.path().to_path_buf(), 1);
        // Write enough to hit 90%
        guard.record_write(950 * 1024); // 950 KiB of 1024 KiB ≈ 92.8%
        assert!(guard.near_limit());
    }

    #[test]
    fn oldest_channel_files_sorted() {
        let tmp = TempDir::new().unwrap();
        // Create two .amrg files with different timestamps
        let f1 = tmp.path().join("a.amrg");
        let f2 = tmp.path().join("b.amrg");
        std::fs::write(&f1, b"old").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&f2, b"new").unwrap();

        let guard = StorageGuard::new(tmp.path().to_path_buf(), 10);
        let files = guard.oldest_channel_files();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0], f1, "oldest file should come first");
    }
}
