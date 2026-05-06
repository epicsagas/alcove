use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tantivy::{Index, IndexReader, ReloadPolicy};

use super::lock::index_dir;

// ---------------------------------------------------------------------------
// Process-level IndexReader cache with TTL eviction
// ---------------------------------------------------------------------------
//
// Opening a Tantivy index on every search call is expensive: it re-mmaps all
// segment files and re-initialises the reader.  We keep one Arc<IndexReader>
// per docs_root path alive for the lifetime of the process (important for the
// long-lived MCP/HTTP server; harmless for CLI single-shot calls).
//
// TTL eviction: readers idle longer than `memory.reader_ttl_secs` (default 300 s)
// are dropped at the next `get_cached_reader` call, provided no search holds a
// clone of the Arc.  This bounds resident memory for long-lived server processes.
//
// On `rebuild_index` the entry is evicted before rebuilding so the next search
// picks up freshly written segments.

#[allow(dead_code)]
pub(crate) struct CachedReaderEntry {
    pub(crate) reader: Arc<IndexReader>,
    pub(crate) last_used: Instant,
}

static PROJECT_READER_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedReaderEntry>>> = OnceLock::new();
static VAULT_READER_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedReaderEntry>>> = OnceLock::new();

/// Cache category — determines which reader cache to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheCategory {
    Project,
    Vault,
}

pub(crate) fn reader_cache_for(cat: CacheCategory) -> &'static Mutex<HashMap<PathBuf, CachedReaderEntry>> {
    match cat {
        CacheCategory::Project => PROJECT_READER_CACHE.get_or_init(|| Mutex::new(HashMap::new())),
        CacheCategory::Vault => VAULT_READER_CACHE.get_or_init(|| Mutex::new(HashMap::new())),
    }
}

/// Return a cached reader for `index_dir`, creating one if absent.
/// Calls `reload()` so that segments written after the reader was first
/// created are visible (no-op when nothing changed).
///
/// TTL eviction runs on every call: idle entries where no external Arc clone
/// is held are dropped.  The cache is also bounded by `memory.max_cached_readers`
/// (default 1); excess entries are evicted LRU-first.
pub(crate) fn get_cached_reader(index_dir: &Path, index: &Index, cat: CacheCategory) -> Result<Arc<IndexReader>> {
    // Tests create a unique TempDir per case, so every path is a cache miss.
    // Bypass the global static cache entirely to prevent unbounded accumulation
    // across the test suite.
    #[cfg(test)]
    {
        let _ = (index_dir, cat);
        Ok(Arc::new(
            index
                .reader_builder()
                .reload_policy(ReloadPolicy::Manual)
                .try_into()
                .context("Failed to create index reader")?,
        ))
    }

    #[cfg(not(test))]
    {
        let mem_cfg = crate::config::load_config().memory_config_with_defaults();
        let ttl = if mem_cfg.reader_ttl_secs > 0 {
            Some(Duration::from_secs(mem_cfg.reader_ttl_secs))
        } else {
            None
        };
        let max_readers = mem_cfg.max_cached_readers.clamp(1, 4);

        let mut cache = reader_cache_for(cat).lock().unwrap_or_else(|e| {
            eprintln!("[alcove] reader cache mutex poisoned — clearing stale entries and recovering");
            let mut guard = e.into_inner();
            guard.clear();
            guard
        });

        if let Some(ttl) = ttl {
            cache.retain(|_, entry| {
                if entry.last_used.elapsed() > ttl {
                    Arc::strong_count(&entry.reader) > 1
                } else {
                    true
                }
            });
        }

        if let Some(entry) = cache.get_mut(index_dir) {
            entry.last_used = Instant::now();
            let _ = entry.reader.reload();
            return Ok(Arc::clone(&entry.reader));
        }

        if cache.len() >= max_readers {
            let lru_key = cache
                .iter()
                .filter(|(_, e)| Arc::strong_count(&e.reader) <= 1)
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone());
            if let Some(key) = lru_key {
                cache.remove(&key);
            }
        }

        let reader = Arc::new(
            index
                .reader_builder()
                .reload_policy(ReloadPolicy::Manual)
                .try_into()
                .context("Failed to create index reader")?,
        );
        cache.insert(
            index_dir.to_path_buf(),
            CachedReaderEntry {
                reader: Arc::clone(&reader),
                last_used: Instant::now(),
            },
        );
        Ok(reader)
    }
}

/// Evict the cached reader for `docs_root`.
/// Must be called whenever the index is fully rebuilt so the next search
/// does not read stale segment data.
pub(crate) fn invalidate_reader_cache(docs_root: &Path, cat: CacheCategory) {
    let dir = index_dir(docs_root);
    if let Ok(mut cache) = reader_cache_for(cat).lock() {
        cache.remove(&dir);
    }
}
