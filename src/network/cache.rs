// src/network/cache.rs
// Disk-based caching for kline data with 1-hour TTL

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

const CACHE_DIR: &str = ".cache";
const CACHE_TTL: Duration = Duration::from_secs(3600); // 1 hour

/// Internal cache entry wrapper with timestamp
#[derive(Serialize, Deserialize)]
struct CacheEntry<T> {
    timestamp: u64, // Unix timestamp when cached
    data: T,
}

/// Disk-based cache for kline data
///
/// Stores kline data in JSON files with 1-hour TTL.
/// Files are stored in .cache/ directory with naming: klines_{symbol}_{interval}.json
pub struct KlineCache {
    cache_dir: PathBuf,
}

impl Default for KlineCache {
    fn default() -> Self {
        Self::new()
    }
}

impl KlineCache {
    /// Create a new cache with default .cache/ directory
    pub fn new() -> Self {
        Self {
            cache_dir: PathBuf::from(CACHE_DIR),
        }
    }

    /// Create a new cache with custom directory (for testing)
    #[cfg(test)]
    pub fn with_dir(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    /// Generate cache file path for given exchange, symbol and interval
    fn cache_path(&self, exchange: &str, symbol: &str, interval: &str) -> PathBuf {
        let filename = format!("klines_{}_{}_{}.json", exchange, symbol.to_lowercase(), interval);
        self.cache_dir.join(filename)
    }

    /// Check if cache file is fresh (less than 1 hour old)
    fn is_fresh(&self, path: &Path) -> bool {
        let Ok(metadata) = fs::metadata(path) else {
            return false;
        };

        let Ok(modified) = metadata.modified() else {
            return false;
        };

        match SystemTime::now().duration_since(modified) {
            Ok(age) => age < CACHE_TTL,
            Err(_) => false, // System time went backwards, treat as stale
        }
    }

    /// Read cached kline data if it exists and is fresh
    ///
    /// Returns None if:
    /// - File doesn't exist
    /// - File is stale (> 1 hour old)
    /// - File cannot be parsed
    pub fn read<T: DeserializeOwned>(&self, exchange: &str, symbol: &str, interval: &str) -> Option<Vec<T>> {
        let path = self.cache_path(exchange, symbol, interval);

        if !path.exists() {
            debug!("Cache miss: file does not exist for {}/{}", symbol, interval);
            return None;
        }

        if !self.is_fresh(&path) {
            debug!("Cache stale: file older than 1 hour for {}/{}", symbol, interval);
            return None;
        }

        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                warn!("Failed to open cache file: {}", e);
                return None;
            }
        };

        let reader = BufReader::new(file);
        match serde_json::from_reader::<_, CacheEntry<Vec<T>>>(reader) {
            Ok(entry) => {
                debug!("Cache hit: loaded {} items for {}/{}", entry.data.len(), symbol, interval);
                Some(entry.data)
            }
            Err(e) => {
                warn!("Failed to parse cache file: {}", e);
                None
            }
        }
    }

    /// Write kline data to cache
    ///
    /// Creates .cache/ directory if it doesn't exist.
    pub fn write<T: Serialize>(&self, exchange: &str, symbol: &str, interval: &str, data: &[T]) -> Result<(), std::io::Error> {
        // Create cache directory if missing
        if !self.cache_dir.exists() {
            fs::create_dir_all(&self.cache_dir)?;
            debug!("Created cache directory: {:?}", self.cache_dir);
        }

        let path = self.cache_path(exchange, symbol, interval);
        let file = File::create(&path)?;
        let writer = BufWriter::new(file);

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let entry = CacheEntry {
            timestamp,
            data,
        };

        serde_json::to_writer(writer, &entry)?;
        debug!("Cache write: saved {} items for {}/{}", data.len(), symbol, interval);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct TestKline {
        open_time: u64,
        close: String,
    }

    #[test]
    fn test_write_then_read_returns_same_data() {
        let temp_dir = TempDir::new().unwrap();
        let cache = KlineCache::with_dir(temp_dir.path().to_path_buf());

        let data = vec![
            TestKline { open_time: 1000, close: "100.0".to_string() },
            TestKline { open_time: 2000, close: "200.0".to_string() },
        ];

        cache.write("asterdex", "BTCUSDT", "1m", &data).unwrap();
        let result: Option<Vec<TestKline>> = cache.read("asterdex", "BTCUSDT", "1m");

        assert!(result.is_some());
        assert_eq!(result.unwrap(), data);
    }

    #[test]
    fn test_read_nonexistent_file_returns_none() {
        let temp_dir = TempDir::new().unwrap();
        let cache = KlineCache::with_dir(temp_dir.path().to_path_buf());

        let result: Option<Vec<TestKline>> = cache.read("asterdex", "NONEXISTENT", "1m");
        assert!(result.is_none());
    }

    #[test]
    fn test_stale_cache_returns_none() {
        let temp_dir = TempDir::new().unwrap();
        let cache = KlineCache::with_dir(temp_dir.path().to_path_buf());

        let data = vec![TestKline { open_time: 1000, close: "100.0".to_string() }];
        cache.write("asterdex", "BTCUSDT", "1m", &data).unwrap();

        // Manually set file modification time to 2 hours ago
        let path = cache.cache_path("asterdex", "BTCUSDT", "1m");
        let two_hours_ago = SystemTime::now() - Duration::from_secs(7200);
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(two_hours_ago)).unwrap();

        let result: Option<Vec<TestKline>> = cache.read("asterdex", "BTCUSDT", "1m");
        assert!(result.is_none());
    }

    #[test]
    fn test_creates_cache_directory_if_missing() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path().join("nested").join("cache");
        let cache = KlineCache::with_dir(cache_dir.clone());

        assert!(!cache_dir.exists());

        let data = vec![TestKline { open_time: 1000, close: "100.0".to_string() }];
        cache.write("asterdex", "BTCUSDT", "1m", &data).unwrap();

        assert!(cache_dir.exists());
    }

    #[test]
    fn test_cache_path_format() {
        let cache = KlineCache::new();
        let path = cache.cache_path("asterdex", "BTCUSDT", "1m");
        assert_eq!(path, PathBuf::from(".cache/klines_asterdex_btcusdt_1m.json"));
    }

    #[test]
    fn test_exchange_prefixed_paths_dont_collide() {
        let cache = KlineCache::new();
        let exchange_a_path = cache.cache_path("exchange_a", "BTCUSDT", "1m");
        let exchange_b_path = cache.cache_path("exchange_b", "BTCUSDT", "1m");
        assert_ne!(exchange_a_path, exchange_b_path);
        assert_eq!(exchange_a_path, PathBuf::from(".cache/klines_exchange_a_btcusdt_1m.json"));
        assert_eq!(exchange_b_path, PathBuf::from(".cache/klines_exchange_b_btcusdt_1m.json"));
    }
}
