//! Vector store using SQLite for persistence (alcove-full feature)
//!
//! Stores document chunk embeddings as BLOBs and computes cosine similarity in Rust.
//! This avoids complex FFI dependencies while providing efficient vector search.

use std::path::Path;

#[cfg(feature = "alcove-full")]
use anyhow::Result;
#[cfg(feature = "alcove-full")]
use rusqlite::{Connection, params};

/// Vector store metadata
#[cfg(feature = "alcove-full")]
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct VectorMeta {
    /// Model name used for embeddings
    pub model: String,
    /// Embedding dimension
    pub dimension: usize,
    /// Number of vectors stored
    pub count: i64,
}

/// Search result with similarity score
#[cfg(feature = "alcove-full")]
#[derive(Debug, Clone)]
pub struct VectorResult {
    /// Project name
    pub project: String,
    /// File path relative to docs_root
    pub file: String,
    /// Chunk ID within the file
    pub chunk_id: u64,
    /// Cosine similarity score (0.0 to 1.0)
    pub score: f32,
}

// ---------------------------------------------------------------------------
// Feature-gated implementation
// ---------------------------------------------------------------------------

/// A single cached HNSW index entry scoped to one project key.
///
/// `key = None`       → index built from ALL projects
/// `key = Some(name)` → index built from that project only
#[cfg(feature = "alcove-full")]
struct HnswCacheEntry {
    index: hnsw_rs::prelude::Hnsw<'static, f32, hnsw_rs::prelude::DistCosine>,
    /// Maps HNSW d_id (= SQLite row `id` as usize) → (project, file, chunk_id)
    id_map: std::collections::HashMap<usize, (String, String, u64)>,
    /// Timestamp of the last access — used for TTL-based eviction.
    last_accessed: std::time::Instant,
}

#[cfg(feature = "alcove-full")]
pub struct VectorStore {
    conn: Connection,
    dimension: usize,
    #[allow(dead_code)]
    model: String,
    /// Per-project HNSW cache.
    ///
    /// Key `None`       = index over all projects (used when no filter is given).
    /// Key `Some(name)` = index limited to that project.
    ///
    /// `RefCell` is safe because `Connection` is `!Send`, so `VectorStore` is
    /// already confined to a single thread.
    hnsw_cache: std::cell::RefCell<std::collections::HashMap<Option<String>, HnswCacheEntry>>,
}

// ---------------------------------------------------------------------------
// TTL eviction helper (module-private)
// ---------------------------------------------------------------------------

#[cfg(feature = "alcove-full")]
fn evict_stale(
    cache: &mut std::collections::HashMap<Option<String>, HnswCacheEntry>,
) {
    let ttl = std::time::Duration::from_secs(300); // 5 minutes
    cache.retain(|_, entry| entry.last_accessed.elapsed() < ttl);
}

// ---------------------------------------------------------------------------
// VectorStore impl
// ---------------------------------------------------------------------------

#[cfg(feature = "alcove-full")]
impl VectorStore {
    /// Open or create a vector store at the given path
    pub fn open(path: &Path, model: &str, dimension: usize) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut conn = Connection::open(path)?;

        // Create tables
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS vectors (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project TEXT NOT NULL,
                file TEXT NOT NULL,
                chunk_id INTEGER NOT NULL,
                embedding BLOB NOT NULL,
                UNIQUE(project, file, chunk_id)
            );

            CREATE INDEX IF NOT EXISTS idx_vectors_project ON vectors(project);
            CREATE INDEX IF NOT EXISTS idx_vectors_file ON vectors(file);
            "#,
        )?;

        // Check/set metadata
        let existing_model: Option<String> = conn.query_row(
            "SELECT value FROM meta WHERE key = 'model'",
            [],
            |row| row.get(0),
        ).ok();

        let existing_dim: Option<usize> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'dimension'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| s.parse().ok());

        // Atomically clear stale vectors and update metadata in one transaction.
        {
            let tx = conn.transaction()?;
            if let Some(em) = existing_model {
                if em != model {
                    tx.execute("DELETE FROM vectors", [])?;
                }
            }
            if let Some(ed) = existing_dim {
                if ed != dimension {
                    tx.execute("DELETE FROM vectors", [])?;
                }
            }
            tx.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('model', ?1)",
                params![model],
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('dimension', ?1)",
                params![dimension.to_string()],
            )?;
            tx.commit()?;
        }

        Ok(Self {
            conn,
            dimension,
            model: model.to_string(),
            hnsw_cache: std::cell::RefCell::new(std::collections::HashMap::new()),
        })
    }

    /// Insert or update multiple vectors efficiently using a transaction.
    ///
    /// Invalidates the cached entry for every affected project AND the
    /// global `None` entry (which spans all projects).
    pub fn batch_upsert(
        &mut self,
        embeddings: impl Iterator<Item = (String, String, u64, Vec<f32>)>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let mut affected_projects: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT OR REPLACE INTO vectors (project, file, chunk_id, embedding)
                VALUES (?1, ?2, ?3, ?4)
                "#
            )?;
            for (project, file, chunk_id, embedding) in embeddings {
                let blob = Self::encode_embedding(&embedding);
                stmt.execute(params![project, file, chunk_id as i64, blob])?;
                affected_projects.insert(project);
            }
        }
        tx.commit()?;

        // Invalidate: affected project keys + global key.
        let mut cache = self.hnsw_cache.borrow_mut();
        for proj in &affected_projects {
            cache.remove(&Some(proj.clone()));
        }
        cache.remove(&None);
        Ok(())
    }

    /// Insert or update a single vector.
    ///
    /// Invalidates the cached entry for this project AND the global `None` entry.
    #[allow(dead_code)]
    pub fn upsert(
        &self,
        project: &str,
        file: &str,
        chunk_id: u64,
        embedding: &[f32],
    ) -> Result<()> {
        let blob = Self::encode_embedding(embedding);

        self.conn.execute(
            r#"
            INSERT OR REPLACE INTO vectors (project, file, chunk_id, embedding)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![project, file, chunk_id as i64, blob],
        )?;

        // Targeted invalidation.
        let mut cache = self.hnsw_cache.borrow_mut();
        cache.remove(&Some(project.to_string()));
        cache.remove(&None);
        Ok(())
    }

    /// Check if a file already has vectors in the store
    pub fn has_file(&self, project: &str, file: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM vectors WHERE project = ?1 AND file = ?2",
            params![project, file],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Delete vectors for a file.
    ///
    /// Removes `Some(project)` and `None` cache entries.
    #[allow(dead_code)]
    pub fn delete_file(&self, project: &str, file: &str) -> Result<usize> {
        let count = self.conn.execute(
            "DELETE FROM vectors WHERE project = ?1 AND file = ?2",
            params![project, file],
        )?;
        let mut cache = self.hnsw_cache.borrow_mut();
        cache.remove(&Some(project.to_string()));
        cache.remove(&None);
        Ok(count)
    }

    /// Delete all vectors for a project.
    ///
    /// Removes `Some(project)` and `None` cache entries.
    #[allow(dead_code)]
    pub fn delete_project(&self, project: &str) -> Result<usize> {
        let count = self.conn.execute(
            "DELETE FROM vectors WHERE project = ?1",
            params![project],
        )?;
        let mut cache = self.hnsw_cache.borrow_mut();
        cache.remove(&Some(project.to_string()));
        cache.remove(&None);
        Ok(count)
    }

    /// Search for similar vectors using HNSW (large datasets) or linear scan (small).
    ///
    /// When `project_filter` is `Some`, only that project's vectors are loaded
    /// into the HNSW index (or searched linearly), reducing memory footprint.
    pub fn search(
        &self,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<VectorResult>> {
        self.search_with_filter(query, limit, None)
    }

    /// Search with an optional project filter.
    pub fn search_with_filter(
        &self,
        query: &[f32],
        limit: usize,
        project_filter: Option<&str>,
    ) -> Result<Vec<VectorResult>> {
        let count: i64 = if let Some(proj) = project_filter {
            self.conn.query_row(
                "SELECT COUNT(*) FROM vectors WHERE project = ?1",
                params![proj],
                |row| row.get(0),
            )?
        } else {
            self.conn.query_row(
                "SELECT COUNT(*) FROM vectors",
                [],
                |row| row.get(0),
            )?
        };

        // Use HNSW for large datasets (>= 5000 vectors)
        #[cfg(feature = "alcove-full")]
        if count >= 5000 {
            return self.search_hnsw(query, limit, project_filter);
        }

        // Fall back to linear search for small datasets
        self.search_linear(query, limit, project_filter)
    }

    /// Linear search (O(n)) - good for small datasets
    ///
    /// When `project_filter` is `Some`, only rows belonging to that project are
    /// fetched from SQLite (uses the `idx_vectors_project` index).
    fn search_linear(
        &self,
        query: &[f32],
        limit: usize,
        project_filter: Option<&str>,
    ) -> Result<Vec<VectorResult>> {
        // Two separate prepare+query branches to keep rusqlite param types clean.
        let rows: Vec<(String, String, u64, Vec<u8>)> = if let Some(project) = project_filter {
            let mut stmt = self.conn.prepare(
                "SELECT project, file, chunk_id, embedding \
                 FROM vectors WHERE project = ?1",
            )?;
            stmt.query_map(params![project], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT project, file, chunk_id, embedding FROM vectors",
            )?;
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?
        };

        let mut results: Vec<VectorResult> = Vec::new();

        for (project, file, chunk_id, blob) in rows {
            let embedding = Self::decode_embedding(&blob);

            let score = cosine_similarity(query, &embedding);
            if score > 0.0 {
                results.push(VectorResult {
                    project,
                    file,
                    chunk_id,
                    score,
                });
            }
        }

        // Sort by score descending
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        Ok(results)
    }

    /// HNSW search (O(log n)) - good for large datasets.
    ///
    /// The index is scoped to `project_filter` and cached under the matching key.
    /// Stale entries (TTL = 5 min) are evicted at the start of each call.
    #[cfg(feature = "alcove-full")]
    fn search_hnsw(
        &self,
        query: &[f32],
        limit: usize,
        project_filter: Option<&str>,
    ) -> Result<Vec<VectorResult>> {
        use hnsw_rs::prelude::*;
        use std::collections::HashMap;

        let cache_key: Option<String> = project_filter.map(|s| s.to_string());

        // Evict stale entries first.
        {
            let mut cache = self.hnsw_cache.borrow_mut();
            evict_stale(&mut cache);
        }

        // Cache miss: build the index for this key.
        if !self.hnsw_cache.borrow().contains_key(&cache_key) {
            // Load vectors from SQLite — project-scoped when possible.
            let mut vectors: Vec<(i64, String, String, u64, Vec<f32>)> = Vec::new();

            if let Some(project) = project_filter {
                let mut stmt = self.conn.prepare(
                    "SELECT id, project, file, chunk_id, embedding \
                     FROM vectors WHERE project = ?1",
                )?;
                let rows = stmt.query_map(params![project], |row| {
                    let id: i64 = row.get(0)?;
                    let proj: String = row.get(1)?;
                    let file: String = row.get(2)?;
                    let chunk_id: u64 = row.get::<_, i64>(3)? as u64;
                    let blob: Vec<u8> = row.get(4)?;
                    Ok((id, proj, file, chunk_id, blob))
                })?;
                for row_result in rows {
                    let (id, proj, file, chunk_id, blob) = row_result?;
                    let embedding = Self::decode_embedding(&blob);
                    vectors.push((id, proj, file, chunk_id, embedding));
                }
            } else {
                let mut stmt = self.conn.prepare(
                    "SELECT id, project, file, chunk_id, embedding FROM vectors",
                )?;
                let rows = stmt.query_map([], |row| {
                    let id: i64 = row.get(0)?;
                    let proj: String = row.get(1)?;
                    let file: String = row.get(2)?;
                    let chunk_id: u64 = row.get::<_, i64>(3)? as u64;
                    let blob: Vec<u8> = row.get(4)?;
                    Ok((id, proj, file, chunk_id, blob))
                })?;
                for row_result in rows {
                    let (id, proj, file, chunk_id, blob) = row_result?;
                    let embedding = Self::decode_embedding(&blob);
                    vectors.push((id, proj, file, chunk_id, embedding));
                }
            }

            if vectors.is_empty() {
                return Ok(Vec::new());
            }

            let ef_build = (limit * 2).max(50);
            let hnsw = Hnsw::<f32, DistCosine>::new(
                16,
                vectors.len().max(1),
                16,
                ef_build,
                DistCosine {},
            );

            let mut id_map: HashMap<usize, (String, String, u64)> =
                HashMap::with_capacity(vectors.len());

            for (id, project, file, chunk_id, embedding) in &vectors {
                let d_id = *id as usize;
                hnsw.insert((embedding.as_slice(), d_id));
                id_map.insert(d_id, (project.clone(), file.clone(), *chunk_id));
            }

            self.hnsw_cache.borrow_mut().insert(
                cache_key.clone(),
                HnswCacheEntry {
                    index: hnsw,
                    id_map,
                    last_accessed: std::time::Instant::now(),
                },
            );
        }

        // Use the cached entry — update last_accessed.
        let mut cache = self.hnsw_cache.borrow_mut();
        let entry = cache.get_mut(&cache_key).unwrap();
        entry.last_accessed = std::time::Instant::now();

        let ef_search = (limit * 2).max(50);
        let neighbors = entry.index.search(query, limit, ef_search);

        let mut vector_results: Vec<VectorResult> = Vec::new();
        for neighbor in neighbors {
            if let Some((project, file, chunk_id)) = entry.id_map.get(&neighbor.d_id) {
                vector_results.push(VectorResult {
                    project: project.clone(),
                    file: file.clone(),
                    chunk_id: *chunk_id,
                    score: 1.0 - neighbor.distance,
                });
            }
        }

        Ok(vector_results)
    }

    /// Get store metadata
    #[allow(dead_code)]
    pub fn meta(&self) -> Result<VectorMeta> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM vectors",
            [],
            |row| row.get(0),
        )?;

        Ok(VectorMeta {
            model: self.model.clone(),
            dimension: self.dimension,
            count: count.try_into().unwrap(),
        })
    }

    /// Check if store is empty
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM vectors",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
        count == 0
    }

    /// Clear all vectors
    #[allow(dead_code)]
    pub fn clear(&self) -> Result<()> {
        self.conn.execute("DELETE FROM vectors", [])?;
        self.hnsw_cache.borrow_mut().clear();
        Ok(())
    }

    /// Encode embedding as bytes (little-endian f32)
    fn encode_embedding(embedding: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(embedding.len() * 4);
        for &v in embedding {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes
    }

    /// Decode embedding from bytes
    fn decode_embedding(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Compute cosine similarity between two vectors
#[cfg(feature = "alcove-full")]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }

    dot / (mag_a * mag_b)
}

/// Reciprocal Rank Fusion (RRF) for combining BM25 and vector search results
///
/// RRF score = sum(1 / (k + rank_i)) for each ranking list
/// Default k = 60 (commonly used value)
#[cfg(feature = "alcove-full")]
pub fn reciprocal_rank_fusion(
    bm25_results: &[(String, String, u64, f32)], // (project, file, chunk_id, score)
    vector_results: &[VectorResult],
    k: u32,
) -> Vec<(String, String, u64, f32)> {
    use std::collections::HashMap;

    let mut scores: HashMap<(String, String, u64), f32> = HashMap::new();

    // Add BM25 contributions
    for (rank, (project, file, chunk_id, _score)) in bm25_results.iter().enumerate() {
        let key = (project.clone(), file.clone(), *chunk_id);
        let rrf = 1.0 / (k as f32 + (rank + 1) as f32);
        *scores.entry(key).or_default() += rrf;
    }

    // Add vector contributions
    for (rank, result) in vector_results.iter().enumerate() {
        let key = (result.project.clone(), result.file.clone(), result.chunk_id);
        let rrf = 1.0 / (k as f32 + (rank + 1) as f32);
        *scores.entry(key).or_default() += rrf;
    }

    // Sort by combined score
    let mut combined: Vec<_> = scores.into_iter().collect();
    combined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    combined
        .into_iter()
        .map(|((project, file, chunk_id), score)| (project, file, chunk_id, score))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &b).abs() < 0.0001);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - (-1.0)).abs() < 0.0001);
    }

    #[test]
    fn test_cosine_similarity_partial() {
        let a = vec![1.0, 1.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        // cos(45°) ≈ 0.707
        assert!((cosine_similarity(&a, &b) - 0.7071).abs() < 0.01);
    }

    /// Fix 1: reopening with a different model must clear all vectors atomically.
    #[cfg(feature = "alcove-full")]
    #[test]
    fn test_open_model_change_clears_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vec.db");

        {
            let mut store = VectorStore::open(&db_path, "model-a", 3).unwrap();
            store
                .batch_upsert(
                    vec![("proj".into(), "f.md".into(), 0u64, vec![1.0, 0.0, 0.0])].into_iter(),
                )
                .unwrap();
            assert!(!store.is_empty());
        }

        // Reopen with a different model — vectors must be gone
        {
            let store = VectorStore::open(&db_path, "model-b", 3).unwrap();
            assert!(store.is_empty(), "vectors must be cleared when model changes");
        }
    }

    /// Fix 2: search_linear with project_filter must only return matching project rows.
    #[cfg(feature = "alcove-full")]
    #[test]
    fn test_search_linear_project_filter() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vec2.db");

        let mut store = VectorStore::open(&db_path, "test-model", 3).unwrap();
        store
            .batch_upsert(
                vec![
                    ("proj-a".into(), "a.md".into(), 0u64, vec![1.0, 0.0, 0.0]),
                    ("proj-b".into(), "b.md".into(), 0u64, vec![1.0, 0.0, 0.0]),
                ]
                .into_iter(),
            )
            .unwrap();

        let results = store
            .search_linear(&[1.0, 0.0, 0.0], 10, Some("proj-a"))
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].project, "proj-a");

        let all = store.search_linear(&[1.0, 0.0, 0.0], 10, None).unwrap();
        assert_eq!(all.len(), 2);
    }

    /// Cache: second search reuses the HNSW index without rebuilding it.
    /// Uses `contains_key` checks on the per-project HashMap.
    #[cfg(feature = "alcove-full")]
    #[test]
    fn test_hnsw_cache_reused_on_second_search() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vec_cache.db");

        let dim = 4usize;
        let store = VectorStore::open(&db_path, "cache-model", dim).unwrap();

        let embeddings: Vec<(String, String, u64, Vec<f32>)> = (0u64..5000)
            .map(|i| {
                let mut v = vec![0.0f32; dim];
                v[(i as usize) % dim] = 1.0;
                ("proj".to_string(), format!("file_{}.md", i), i, v)
            })
            .collect();

        drop(store);
        let mut store = VectorStore::open(&db_path, "cache-model", dim).unwrap();
        store.batch_upsert(embeddings.into_iter()).unwrap();

        // Cache is empty before first search.
        {
            let cache = store.hnsw_cache.borrow();
            assert!(
                !cache.contains_key(&None),
                "global cache key must be absent before first search"
            );
        }

        let query = vec![1.0f32, 0.0, 0.0, 0.0];
        let _r1 = store.search_hnsw(&query, 5, None).unwrap();

        // After first search the global key must be present.
        {
            let cache = store.hnsw_cache.borrow();
            assert!(
                cache.contains_key(&None),
                "global cache key must be present after first search"
            );
        }

        // Second search — cache must still be present (no write happened).
        let _r2 = store.search_hnsw(&query, 5, None).unwrap();
        {
            let cache = store.hnsw_cache.borrow();
            assert!(
                cache.contains_key(&None),
                "global cache key must still exist after second search"
            );
        }

        // Insert one more vector — global cache key must be invalidated.
        store
            .batch_upsert(
                vec![("proj".into(), "extra.md".into(), 9999u64, vec![0.0, 0.0, 0.0, 1.0])]
                    .into_iter(),
            )
            .unwrap();

        {
            let cache = store.hnsw_cache.borrow();
            assert!(
                !cache.contains_key(&None),
                "global cache key must be invalidated after insert"
            );
        }

        // Search rebuilds the cache.
        let _r3 = store.search_hnsw(&query, 5, None).unwrap();
        {
            let cache = store.hnsw_cache.borrow();
            assert!(
                cache.contains_key(&None),
                "global cache key must be rebuilt after insert"
            );
        }
    }

    /// Per-project cache: searching proj-a must populate only the proj-a key,
    /// not the global None key or the proj-b key.
    #[cfg(feature = "alcove-full")]
    #[test]
    fn test_hnsw_cache_per_project() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vec_per_proj.db");

        let dim = 4usize;
        // Insert 5000 vectors for proj-a and 5000 for proj-b to cross HNSW threshold.
        let embeddings: Vec<(String, String, u64, Vec<f32>)> = (0u64..5000)
            .flat_map(|i| {
                let mut va = vec![0.0f32; dim];
                va[(i as usize) % dim] = 1.0;
                let mut vb = vec![0.0f32; dim];
                vb[(i as usize) % dim] = 0.5;
                vec![
                    ("proj-a".to_string(), format!("a_{}.md", i), i, va),
                    ("proj-b".to_string(), format!("b_{}.md", i), i, vb),
                ]
            })
            .collect();

        let mut store = VectorStore::open(&db_path, "pp-model", dim).unwrap();
        store.batch_upsert(embeddings.into_iter()).unwrap();

        // Search with project filter for proj-a.
        let query = vec![1.0f32, 0.0, 0.0, 0.0];
        let _results = store.search_hnsw(&query, 5, Some("proj-a")).unwrap();

        {
            let cache = store.hnsw_cache.borrow();
            assert!(
                cache.contains_key(&Some("proj-a".to_string())),
                "proj-a cache key must be populated"
            );
            assert!(
                !cache.contains_key(&None),
                "global (None) cache key must NOT be populated when searching proj-a only"
            );
            assert!(
                !cache.contains_key(&Some("proj-b".to_string())),
                "proj-b cache key must NOT be populated"
            );
        }
    }

    /// TTL eviction: a manually backdated entry must be evicted on next search.
    #[cfg(feature = "alcove-full")]
    #[test]
    fn test_hnsw_cache_ttl_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vec_ttl.db");

        let dim = 4usize;
        let embeddings: Vec<(String, String, u64, Vec<f32>)> = (0u64..5000)
            .map(|i| {
                let mut v = vec![0.0f32; dim];
                v[(i as usize) % dim] = 1.0;
                ("proj".to_string(), format!("f_{}.md", i), i, v)
            })
            .collect();

        let mut store = VectorStore::open(&db_path, "ttl-model", dim).unwrap();
        store.batch_upsert(embeddings.into_iter()).unwrap();

        // Warm the cache.
        let query = vec![1.0f32, 0.0, 0.0, 0.0];
        store.search_hnsw(&query, 5, None).unwrap();

        // Backdate the last_accessed timestamp by 10 minutes (past TTL of 5 min).
        {
            let mut cache = store.hnsw_cache.borrow_mut();
            if let Some(entry) = cache.get_mut(&None) {
                entry.last_accessed =
                    std::time::Instant::now()
                        .checked_sub(std::time::Duration::from_secs(600))
                        .unwrap_or_else(std::time::Instant::now);
            }
        }

        // Confirm the stale entry is still there before the next search.
        assert!(
            store.hnsw_cache.borrow().contains_key(&None),
            "stale entry should still exist before eviction"
        );

        // Next search triggers evict_stale, which removes the backdated entry,
        // then rebuilds the cache for this key.
        store.search_hnsw(&query, 5, None).unwrap();

        // After the search the key must exist again (rebuilt after eviction).
        assert!(
            store.hnsw_cache.borrow().contains_key(&None),
            "cache must be rebuilt after stale eviction"
        );
    }

    #[test]
    fn test_rrf_combines_rankings() {
        let bm25 = vec![
            ("p1".into(), "a.md".into(), 0, 3.0),
            ("p1".into(), "b.md".into(), 0, 2.0),
        ];
        let vector = vec![
            VectorResult { project: "p1".into(), file: "b.md".into(), chunk_id: 0, score: 0.9 },
            VectorResult { project: "p1".into(), file: "c.md".into(), chunk_id: 0, score: 0.8 },
        ];

        let fused = reciprocal_rank_fusion(&bm25, &vector, 60);

        // b.md should rank higher (appears in both)
        assert_eq!(fused[0].1, "b.md");
        assert!(fused.len() >= 2);
    }
}
