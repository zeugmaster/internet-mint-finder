//! Simple in-memory TTL cache for analytics query results.
//!
//! Absorbs burst loads when multiple dashboard tiles fire simultaneously.
//! Each cache entry expires after `TTL` seconds.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const DEFAULT_TTL: Duration = Duration::from_secs(10);
const MAX_ENTRIES: usize = 128;
const MAX_ENTRY_BYTES: usize = 256 * 1024;
const MAX_KEY_BYTES: usize = 8 * 1024;

struct CacheEntry {
    data: String, // JSON-serialized
    expires_at: Instant,
}

/// A simple TTL cache keyed by string.
pub struct ResponseCache {
    entries: Mutex<HashMap<String, CacheEntry>>,
    ttl: Duration,
}

impl Default for ResponseCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ResponseCache {
    /// Create a new cache with the default 10-second TTL.
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl: DEFAULT_TTL,
        }
    }

    /// Try to get a cached JSON string for the given key.
    pub fn get(&self, key: &str) -> Option<String> {
        let mut entries = self.entries.lock().ok()?;
        if let Some(entry) = entries.get(key) {
            if entry.expires_at > Instant::now() {
                return Some(entry.data.clone());
            }
            // Expired -- remove it
            entries.remove(key);
        }
        None
    }

    /// Store a value in the cache. The value is serialized to JSON.
    pub fn set<T: Serialize>(&self, key: String, value: &T) {
        if let Ok(json) = serde_json::to_string(value) {
            if json.len() > MAX_ENTRY_BYTES || key.len() > MAX_KEY_BYTES {
                return;
            }
            if let Ok(mut entries) = self.entries.lock() {
                let now = Instant::now();
                entries.retain(|_, entry| entry.expires_at > now);
                if entries.len() >= MAX_ENTRIES && !entries.contains_key(&key) {
                    if let Some(oldest) = entries
                        .iter()
                        .min_by_key(|(_, entry)| entry.expires_at)
                        .map(|(key, _)| key.clone())
                    {
                        entries.remove(&oldest);
                    }
                }
                entries.insert(
                    key,
                    CacheEntry {
                        data: json,
                        expires_at: now + self.ttl,
                    },
                );
            }
        }
    }

    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.clear();
        }
    }

    /// Build a cache key from a request path and query string.
    pub fn key(path: &str, query: &str) -> String {
        if query.is_empty() {
            path.to_string()
        } else {
            format!("{}?{}", path, query)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_query_keys_cannot_grow_cache_without_bound() {
        let cache = ResponseCache::new();
        for n in 0..MAX_ENTRIES * 2 {
            cache.set(n.to_string(), &n);
        }
        assert_eq!(cache.entries.lock().unwrap().len(), MAX_ENTRIES);
        assert!(cache.get("0").is_none());
        cache.set("large".into(), &"x".repeat(MAX_ENTRY_BYTES));
        assert!(cache.get("large").is_none());
        cache.clear();
        assert!(cache.entries.lock().unwrap().is_empty());
    }

    #[test]
    fn writes_sweep_expired_keys_even_when_never_read_again() {
        let cache = ResponseCache {
            ttl: Duration::ZERO,
            ..ResponseCache::new()
        };
        cache.set("first".into(), &1);
        cache.set("second".into(), &2);
        assert_eq!(cache.entries.lock().unwrap().len(), 1);
        assert!(cache.get("second").is_none());
    }
}
