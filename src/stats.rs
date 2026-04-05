//! File access statistics with in-memory accumulator.
//!
//! Uses [`DashMap`] with atomic counters for lock-free concurrent access tracking.
//! Statistics can be periodically flushed to a [`BlobDatabase`](crate::db::BlobDatabase).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;

/// Per-blob access counters (lock-free).
struct BlobCounters {
    egress_bytes: AtomicU64,
    last_accessed: AtomicU64,
}

/// In-memory accumulator for file access statistics.
///
/// Designed for high-throughput concurrent recording. Call [`flush`](StatsAccumulator::flush)
/// periodically to persist accumulated stats to a database.
pub struct StatsAccumulator {
    counters: Arc<DashMap<String, BlobCounters>>,
}

impl StatsAccumulator {
    pub fn new() -> Self {
        Self {
            counters: Arc::new(DashMap::new()),
        }
    }

    /// Record a download/access event for a blob.
    pub fn record_access(&self, sha256: &str, bytes_served: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.counters
            .entry(sha256.to_string())
            .or_insert_with(|| BlobCounters {
                egress_bytes: AtomicU64::new(0),
                last_accessed: AtomicU64::new(0),
            })
            .egress_bytes
            .fetch_add(bytes_served, Ordering::Relaxed);

        if let Some(entry) = self.counters.get(sha256) {
            entry.last_accessed.store(now, Ordering::Relaxed);
        }
    }

    /// Drain accumulated stats and return them as `(sha256, egress_bytes, last_accessed)` tuples.
    ///
    /// After calling this, the accumulator is empty. Use the returned data to
    /// persist via [`BlobDatabase::record_access`](crate::db::BlobDatabase::record_access).
    pub fn drain(&self) -> Vec<(String, u64, u64)> {
        let keys: Vec<String> = self.counters.iter().map(|e| e.key().clone()).collect();
        let mut results = Vec::with_capacity(keys.len());

        for key in keys {
            if let Some((sha256, counters)) = self.counters.remove(&key) {
                let egress = counters.egress_bytes.load(Ordering::Relaxed);
                let last = counters.last_accessed.load(Ordering::Relaxed);
                if egress > 0 {
                    results.push((sha256, egress, last));
                }
            }
        }

        results
    }

    /// Flush accumulated stats to a database.
    pub fn flush(&self, db: &mut dyn crate::db::BlobDatabase) {
        for (sha256, egress, _last) in self.drain() {
            if let Err(e) = db.record_access(&sha256, egress) {
                tracing::warn!(
                    component = "blossom.stats",
                    blob_sha = %sha256,
                    error = %e,
                    "failed to flush stats to database"
                );
            }
        }
    }

    /// Number of blobs currently being tracked.
    pub fn tracked_count(&self) -> usize {
        self.counters.len()
    }

    /// Get current accumulated egress for a blob (without draining).
    pub fn get_egress(&self, sha256: &str) -> u64 {
        self.counters
            .get(sha256)
            .map(|c| c.egress_bytes.load(Ordering::Relaxed))
            .unwrap_or(0)
    }
}

impl Default for StatsAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_drain() {
        let stats = StatsAccumulator::new();
        let sha = "a".repeat(64);

        stats.record_access(&sha, 100);
        stats.record_access(&sha, 200);
        stats.record_access(&sha, 50);

        assert_eq!(stats.get_egress(&sha), 350);
        assert_eq!(stats.tracked_count(), 1);

        let drained = stats.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].0, sha);
        assert_eq!(drained[0].1, 350);
        assert!(drained[0].2 > 0); // last_accessed timestamp

        // After drain, accumulator is empty.
        assert_eq!(stats.tracked_count(), 0);
        assert_eq!(stats.get_egress(&sha), 0);
    }

    #[test]
    fn test_multiple_blobs() {
        let stats = StatsAccumulator::new();
        let sha1 = "a".repeat(64);
        let sha2 = "b".repeat(64);

        stats.record_access(&sha1, 100);
        stats.record_access(&sha2, 200);
        stats.record_access(&sha1, 50);

        assert_eq!(stats.tracked_count(), 2);
        assert_eq!(stats.get_egress(&sha1), 150);
        assert_eq!(stats.get_egress(&sha2), 200);
    }

    #[test]
    fn test_drain_empty() {
        let stats = StatsAccumulator::new();
        let drained = stats.drain();
        assert!(drained.is_empty());
    }

    #[test]
    fn test_flush_to_memory_db() {
        use crate::db::BlobDatabase;
        let stats = StatsAccumulator::new();
        let mut db = crate::db::MemoryDatabase::new();
        let sha = "c".repeat(64);

        stats.record_access(&sha, 500);
        stats.record_access(&sha, 300);

        stats.flush(&mut db);

        let file_stats = db.get_stats(&sha).unwrap();
        assert_eq!(file_stats.egress_bytes, 800);
        assert!(file_stats.last_accessed > 0);

        // Accumulator should be drained.
        assert_eq!(stats.tracked_count(), 0);
    }

    #[test]
    fn test_concurrent_access() {
        use std::thread;

        let stats = Arc::new(StatsAccumulator::new());
        let sha = "d".repeat(64);

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let stats = stats.clone();
                let sha = sha.clone();
                thread::spawn(move || {
                    for _ in 0..100 {
                        stats.record_access(&sha, 1);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(stats.get_egress(&sha), 1000);
    }
}
