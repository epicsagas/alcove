//! Vector store backends for document embedding search.
//!
//! Two backends are available:
//! - **turboquant** (recommended): TurboQuant 4-bit compressed index — 8x memory reduction,
//!   SIMD-accelerated search, built-in filtered search and remove.
//! - **vector** (fallback): hnsw_rs HNSW index with SQLite persistence.
//!
//! Both backends expose the same public API: `VectorStore`, `VectorResult`, `VectorMeta`.

use std::path::Path;

// ---------------------------------------------------------------------------
// Shared types (available with either backend)
// ---------------------------------------------------------------------------

/// Search result with similarity score
#[cfg(any(feature = "vector", feature = "turboquant"))]
#[derive(Debug, Clone, PartialEq)]
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

/// Vector store metadata
#[cfg(any(feature = "vector", feature = "turboquant"))]
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

// ---------------------------------------------------------------------------
// hnsw_rs backend (feature = "vector", no turboquant)
// ---------------------------------------------------------------------------

#[cfg(all(feature = "vector", not(feature = "turboquant")))]
use std::cmp::Ordering;
#[cfg(all(feature = "vector", not(feature = "turboquant")))]
use std::collections::BinaryHeap;

#[cfg(all(feature = "vector", not(feature = "turboquant")))]
use anyhow::Result;
#[cfg(all(feature = "vector", not(feature = "turboquant")))]
use rusqlite::{Connection, params};

/// Wrapper for min-heap ordering used by `search_linear` top-K selection.
#[cfg(all(feature = "vector", not(feature = "turboquant")))]
#[derive(PartialEq)]
struct MinScoreEntry {
    score: f32,
    result: VectorResult,
}

#[cfg(all(feature = "vector", not(feature = "turboquant")))]
impl Eq for MinScoreEntry {}

#[cfg(all(feature = "vector", not(feature = "turboquant")))]
impl PartialOrd for MinScoreEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(all(feature = "vector", not(feature = "turboquant")))]
impl Ord for MinScoreEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .partial_cmp(&self.score)
            .unwrap_or(Ordering::Equal)
    }
}

// hnsw_rs: cache entry
#[cfg(all(feature = "vector", not(feature = "turboquant")))]
struct HnswCacheEntry {
    index: hnsw_rs::prelude::Hnsw<'static, f32, hnsw_rs::prelude::DistCosine>,
    id_map: std::collections::HashMap<usize, (String, String, u64)>,
    last_accessed: std::time::Instant,
}

#[cfg(all(feature = "vector", not(feature = "turboquant")))]
const HNSW_CACHE_MAX_ENTRIES: usize = 3;

#[cfg(all(feature = "vector", not(feature = "turboquant")))]
pub struct VectorStore {
    conn: Connection,
    dimension: usize,
    #[allow(dead_code)]
    model: String,
    hnsw_cache: std::cell::RefCell<std::collections::HashMap<Option<String>, HnswCacheEntry>>,
    last_evict: std::cell::Cell<std::time::Instant>,
}

#[cfg(all(feature = "vector", not(feature = "turboquant")))]
fn evict_stale(cache: &mut std::collections::HashMap<Option<String>, HnswCacheEntry>) {
    let ttl = std::time::Duration::from_secs(300);
    cache.retain(|_, entry| entry.last_accessed.elapsed() < ttl);
}

#[cfg(all(feature = "vector", not(feature = "turboquant")))]
impl VectorStore {
    #[cfg_attr(not(feature = "embed"), allow(dead_code))]
    pub fn open(path: &Path, model: &str, dimension: usize) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut conn = Connection::open(path)?;
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

        let existing_model: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'model'", [], |row| {
                row.get(0)
            })
            .ok();
        let existing_dim: Option<usize> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'dimension'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| s.parse().ok());

        {
            let tx = conn.transaction()?;
            let model_changed = existing_model.is_some_and(|em| em != model);
            let dim_changed = existing_dim.is_some_and(|ed| ed != dimension);
            if model_changed || dim_changed {
                tx.execute("DELETE FROM vectors", [])?;
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
            last_evict: std::cell::Cell::new(std::time::Instant::now()),
        })
    }

    #[cfg_attr(not(feature = "embed"), allow(dead_code))]
    pub fn batch_upsert(
        &mut self,
        embeddings: impl Iterator<Item = (String, String, u64, Vec<f32>)>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let mut affected_projects: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO vectors (project, file, chunk_id, embedding)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (project, file, chunk_id, embedding) in embeddings {
                let blob = Self::encode_embedding(&embedding);
                stmt.execute(params![project, file, chunk_id as i64, blob])?;
                affected_projects.insert(project);
            }
        }
        tx.commit()?;
        let mut cache = self.hnsw_cache.borrow_mut();
        for proj in &affected_projects {
            cache.remove(&Some(proj.clone()));
        }
        cache.remove(&None);
        Ok(())
    }

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
            "INSERT OR REPLACE INTO vectors (project, file, chunk_id, embedding)
             VALUES (?1, ?2, ?3, ?4)",
            params![project, file, chunk_id as i64, blob],
        )?;
        let mut cache = self.hnsw_cache.borrow_mut();
        cache.remove(&Some(project.to_string()));
        cache.remove(&None);
        Ok(())
    }

    #[cfg_attr(not(feature = "embed"), allow(dead_code))]
    pub fn has_file(&self, project: &str, file: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM vectors WHERE project = ?1 AND file = ?2",
            params![project, file],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

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

    #[allow(dead_code)]
    pub fn delete_project(&self, project: &str) -> Result<usize> {
        let count = self
            .conn
            .execute("DELETE FROM vectors WHERE project = ?1", params![project])?;
        let mut cache = self.hnsw_cache.borrow_mut();
        cache.remove(&Some(project.to_string()));
        cache.remove(&None);
        Ok(count)
    }

    #[allow(dead_code)]
    pub fn remove_by_ids(&self, ids: &[u64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "DELETE FROM vectors WHERE id IN ({})",
            placeholders.join(", ")
        );
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
            .iter()
            .map(|&id| Box::new(id as i64) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let count = self.conn.execute(&sql, param_refs.as_slice())?;
        self.hnsw_cache.borrow_mut().clear();
        Ok(count)
    }

    #[allow(dead_code)]
    pub fn search(&self, query: &[f32], limit: usize) -> Result<Vec<VectorResult>> {
        self.search_with_filter(query, limit, None)
    }

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
            self.conn
                .query_row("SELECT COUNT(*) FROM vectors", [], |row| row.get(0))?
        };
        if count >= 5000 {
            return self.search_hnsw(query, limit, project_filter);
        }
        self.search_linear(query, limit, project_filter)
    }

    #[allow(dead_code)]
    pub fn search_filtered(
        &self,
        query: &[f32],
        limit: usize,
        allowlist: &[u64],
    ) -> Result<Vec<VectorResult>> {
        if allowlist.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: Vec<String> = (1..=allowlist.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT project, file, chunk_id, embedding FROM vectors WHERE id IN ({})",
            placeholders.join(", ")
        );
        let sql_params: Vec<Box<dyn rusqlite::types::ToSql>> = allowlist
            .iter()
            .map(|&id| Box::new(id as i64) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            sql_params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? as u64,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })?;
        let mut heap: BinaryHeap<MinScoreEntry> = BinaryHeap::with_capacity(limit + 1);
        for row_result in rows {
            let (proj, file, chunk_id, blob) = row_result?;
            let embedding = Self::decode_embedding(&blob);
            let score = cosine_similarity(query, &embedding);
            if score > 0.0 {
                heap.push(MinScoreEntry {
                    score,
                    result: VectorResult {
                        project: proj,
                        file,
                        chunk_id,
                        score,
                    },
                });
                if heap.len() > limit {
                    heap.pop();
                }
            }
        }
        let mut results: Vec<VectorResult> = heap.into_iter().map(|e| e.result).collect();
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
        Ok(results)
    }

    fn search_linear(
        &self,
        query: &[f32],
        limit: usize,
        project_filter: Option<&str>,
    ) -> Result<Vec<VectorResult>> {
        let mut heap: BinaryHeap<MinScoreEntry> = BinaryHeap::with_capacity(limit + 1);
        #[inline]
        fn push_if_positive(
            heap: &mut BinaryHeap<MinScoreEntry>,
            limit: usize,
            project: String,
            file: String,
            chunk_id: u64,
            score: f32,
        ) {
            if score > 0.0 {
                heap.push(MinScoreEntry {
                    score,
                    result: VectorResult {
                        project,
                        file,
                        chunk_id,
                        score,
                    },
                });
                if heap.len() > limit {
                    heap.pop();
                }
            }
        }
        if let Some(project) = project_filter {
            let mut stmt = self.conn.prepare(
                "SELECT project, file, chunk_id, embedding FROM vectors WHERE project = ?1",
            )?;
            let mapped = stmt.query_map(params![project], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            })?;
            for row_result in mapped {
                let (proj, file, chunk_id, blob) = row_result?;
                let embedding = Self::decode_embedding(&blob);
                let score = cosine_similarity(query, &embedding);
                push_if_positive(&mut heap, limit, proj, file, chunk_id, score);
            }
        } else {
            let mut stmt = self
                .conn
                .prepare("SELECT project, file, chunk_id, embedding FROM vectors")?;
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            })?;
            for row_result in mapped {
                let (proj, file, chunk_id, blob) = row_result?;
                let embedding = Self::decode_embedding(&blob);
                let score = cosine_similarity(query, &embedding);
                push_if_positive(&mut heap, limit, proj, file, chunk_id, score);
            }
        }
        let mut results: Vec<VectorResult> = heap.into_iter().map(|e| e.result).collect();
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal));
        Ok(results)
    }

    fn search_hnsw(
        &self,
        query: &[f32],
        limit: usize,
        project_filter: Option<&str>,
    ) -> Result<Vec<VectorResult>> {
        use hnsw_rs::prelude::*;
        use std::collections::HashMap;

        let cache_key: Option<String> = project_filter.map(|s| s.to_string());
        {
            let evict_interval = std::time::Duration::from_secs(60);
            if self.last_evict.get().elapsed() >= evict_interval {
                let mut cache = self.hnsw_cache.borrow_mut();
                evict_stale(&mut cache);
                self.last_evict.set(std::time::Instant::now());
            }
        }
        if !self.hnsw_cache.borrow().contains_key(&cache_key) {
            let mut vectors: Vec<(i64, String, String, u64, Vec<f32>)> = Vec::new();
            if let Some(project) = project_filter {
                let mut stmt = self.conn.prepare(
                    "SELECT id, project, file, chunk_id, embedding FROM vectors WHERE project = ?1",
                )?;
                let rows = stmt.query_map(params![project], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)? as u64,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                })?;
                for row_result in rows {
                    let (id, proj, file, chunk_id, blob) = row_result?;
                    vectors.push((id, proj, file, chunk_id, Self::decode_embedding(&blob)));
                }
            } else {
                let mut stmt = self
                    .conn
                    .prepare("SELECT id, project, file, chunk_id, embedding FROM vectors")?;
                let rows = stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)? as u64,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                })?;
                for row_result in rows {
                    let (id, proj, file, chunk_id, blob) = row_result?;
                    vectors.push((id, proj, file, chunk_id, Self::decode_embedding(&blob)));
                }
            }
            if vectors.is_empty() {
                return Ok(Vec::new());
            }
            let ef_build = 200;
            let hnsw =
                Hnsw::<f32, DistCosine>::new(16, vectors.len().max(1), 16, ef_build, DistCosine {});
            let mut id_map: HashMap<usize, (String, String, u64)> =
                HashMap::with_capacity(vectors.len());
            for (id, project, file, chunk_id, embedding) in &vectors {
                let d_id = *id as usize;
                hnsw.insert((embedding.as_slice(), d_id));
                id_map.insert(d_id, (project.clone(), file.clone(), *chunk_id));
            }
            #[cfg(test)]
            {
                let ef_search = (limit * 2).max(50);
                let neighbors = hnsw.search(query, limit, ef_search);
                let mut results: Vec<VectorResult> = Vec::new();
                for neighbor in &neighbors {
                    if let Some((project, file, chunk_id)) = id_map.get(&neighbor.d_id) {
                        results.push(VectorResult {
                            project: project.clone(),
                            file: file.clone(),
                            chunk_id: *chunk_id,
                            score: 1.0 - neighbor.distance,
                        });
                    }
                }
                return Ok(results);
            }
            #[cfg(not(test))]
            {
                let mut cache = self.hnsw_cache.borrow_mut();
                if cache.len() >= HNSW_CACHE_MAX_ENTRIES
                    && let Some(lru_key) = cache
                        .iter()
                        .min_by_key(|(_, e)| e.last_accessed)
                        .map(|(k, _)| k.clone())
                {
                    cache.remove(&lru_key);
                }
                cache.insert(
                    cache_key.clone(),
                    HnswCacheEntry {
                        index: hnsw,
                        id_map,
                        last_accessed: std::time::Instant::now(),
                    },
                );
            }
        }
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

    #[allow(dead_code)]
    pub fn meta(&self) -> Result<VectorMeta> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM vectors", [], |row| row.get(0))?;
        Ok(VectorMeta {
            model: self.model.clone(),
            dimension: self.dimension,
            count,
        })
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> Result<bool> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM vectors", [], |row| row.get(0))?;
        Ok(count == 0)
    }

    #[allow(dead_code)]
    pub fn clear(&self) -> Result<()> {
        self.conn.execute("DELETE FROM vectors", [])?;
        self.hnsw_cache.borrow_mut().clear();
        Ok(())
    }

    fn encode_embedding(embedding: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(embedding.len() * 4);
        for &v in embedding {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes
    }

    fn decode_embedding(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// TurboQuant backend (feature = "turboquant")
// ---------------------------------------------------------------------------

#[cfg(feature = "turboquant")]
use anyhow::Result;
#[cfg(feature = "turboquant")]
use llm_kernel::embedding::VectorIndex;
#[cfg(feature = "turboquant")]
use rusqlite::{Connection, params};

/// Maps external row IDs to (project, file, chunk_id) metadata.
type IdMetaMap = std::collections::HashMap<u64, (String, String, u64)>;

#[cfg(feature = "turboquant")]
pub struct VectorStore {
    /// SQLite connection for metadata and raw vector storage.
    conn: Connection,
    dimension: usize,
    #[allow(dead_code)]
    model: String,
    /// In-memory TurboQuant compressed index.
    index: std::cell::RefCell<Option<llm_kernel_vector_index::TurbovecIndex>>,
    /// Maps external ID → (project, file, chunk_id).
    id_map: std::cell::RefCell<IdMetaMap>,
    /// Path to the saved index file (for save/load).
    index_path: std::path::PathBuf,
}

#[cfg(feature = "turboquant")]
impl VectorStore {
    /// Open or create a vector store at the given path.
    ///
    /// The SQLite database stores raw vectors and metadata.
    /// The TurboQuant index is saved alongside as `<path>.tvim`.
    #[cfg_attr(not(feature = "embed"), allow(dead_code))]
    pub fn open(path: &Path, model: &str, dimension: usize) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut conn = Connection::open(path)?;
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

        let existing_model: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'model'", [], |row| {
                row.get(0)
            })
            .ok();
        let existing_dim: Option<usize> = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'dimension'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| s.parse().ok());

        {
            let tx = conn.transaction()?;
            let model_changed = existing_model.is_some_and(|em| em != model);
            let dim_changed = existing_dim.is_some_and(|ed| ed != dimension);
            if model_changed || dim_changed {
                tx.execute("DELETE FROM vectors", [])?;
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

        let index_path = path.with_extension("tvim");
        let store = Self {
            conn,
            dimension,
            model: model.to_string(),
            index: std::cell::RefCell::new(None),
            id_map: std::cell::RefCell::new(IdMetaMap::new()),
            index_path,
        };

        // Try loading a previously saved index.
        if store.index_path.exists()
            && let Ok(idx) = llm_kernel_vector_index::TurbovecIndex::load(&store.index_path)
        {
            // Rebuild id_map from SQLite to stay in sync.
            store.rebuild_id_map();
            *store.index.borrow_mut() = Some(idx);
        }

        Ok(store)
    }

    /// Load all (id, project, file, chunk_id) from SQLite into id_map.
    fn rebuild_id_map(&self) {
        let mut map = IdMetaMap::new();
        let mut stmt = self
            .conn
            .prepare("SELECT id, project, file, chunk_id FROM vectors")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)? as u64,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)? as u64,
                ))
            })
            .unwrap();
        for row_result in rows.flatten() {
            let (id, project, file, chunk_id) = row_result;
            map.insert(id, (project, file, chunk_id));
        }
        *self.id_map.borrow_mut() = map;
    }

    /// Ensure the in-memory index is populated. Rebuilds from SQLite if needed.
    fn ensure_index(&self) -> Result<()> {
        if self.index.borrow().is_some() {
            return Ok(());
        }
        // Load all vectors from SQLite and build the index.
        let mut stmt = self
            .conn
            .prepare("SELECT id, embedding FROM vectors ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)? as u64, row.get::<_, Vec<u8>>(1)?))
        })?;

        let mut ids: Vec<u64> = Vec::new();
        let mut vectors: Vec<Vec<f32>> = Vec::new();
        let mut map = IdMetaMap::new();

        // Also rebuild id_map with full metadata.
        let mut meta_stmt = self
            .conn
            .prepare("SELECT id, project, file, chunk_id FROM vectors ORDER BY id")?;
        let meta_rows = meta_stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? as u64,
            ))
        })?;
        for row_result in meta_rows.flatten() {
            let (id, project, file, chunk_id) = row_result;
            map.insert(id, (project, file, chunk_id));
        }

        for row_result in rows {
            let (id, blob) = row_result?;
            let embedding = Self::decode_embedding(&blob);
            ids.push(id);
            vectors.push(embedding);
        }

        let mut idx = llm_kernel_vector_index::TurbovecIndex::new(self.dimension, 4)?;
        if !ids.is_empty() {
            idx.add_with_ids(&vectors, &ids)?;
        }
        *self.index.borrow_mut() = Some(idx);
        *self.id_map.borrow_mut() = map;
        Ok(())
    }

    /// Insert or update multiple vectors.
    #[cfg_attr(not(feature = "embed"), allow(dead_code))]
    pub fn batch_upsert(
        &mut self,
        embeddings: impl Iterator<Item = (String, String, u64, Vec<f32>)>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let mut new_rows: Vec<(u64, String, String, u64, Vec<f32>)> = Vec::new();
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO vectors (project, file, chunk_id, embedding)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (project, file, chunk_id, embedding) in embeddings {
                let blob = Self::encode_embedding(&embedding);
                stmt.execute(params![project, file, chunk_id as i64, blob])?;
                new_rows.push((0, project, file, chunk_id, embedding));
            }
        }
        tx.commit()?;

        // Retrieve the auto-assigned row IDs.
        for row in &mut new_rows {
            let id: i64 = self.conn.query_row(
                "SELECT id FROM vectors WHERE project = ?1 AND file = ?2 AND chunk_id = ?3",
                params![row.1, row.2, row.3 as i64],
                |r| r.get(0),
            )?;
            row.0 = id as u64;
        }

        // Add to in-memory index.
        let mut index = self.index.borrow_mut();
        if let Some(ref mut idx) = *index {
            for (id, _project, _file, _chunk_id, embedding) in &new_rows {
                idx.add_with_ids(std::slice::from_ref(embedding), &[*id])?;
            }
        } else {
            // Index not yet built; will be built on first search.
        }

        // Update id_map.
        let mut id_map = self.id_map.borrow_mut();
        for (id, project, file, chunk_id, _) in &new_rows {
            id_map.insert(*id, (project.clone(), file.clone(), *chunk_id));
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub fn has_file(&self, project: &str, file: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM vectors WHERE project = ?1 AND file = ?2",
            params![project, file],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    #[allow(dead_code)]
    pub fn delete_file(&self, project: &str, file: &str) -> Result<usize> {
        // Collect IDs before deleting.
        let ids: Vec<u64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM vectors WHERE project = ?1 AND file = ?2")?;
            stmt.query_map(params![project, file], |row| {
                row.get::<_, i64>(0).map(|id| id as u64)
            })?
            .filter_map(|r| r.ok())
            .collect()
        };
        let count = self.conn.execute(
            "DELETE FROM vectors WHERE project = ?1 AND file = ?2",
            params![project, file],
        )?;
        self.remove_from_index(&ids);
        Ok(count)
    }

    #[allow(dead_code)]
    pub fn delete_project(&self, project: &str) -> Result<usize> {
        let ids: Vec<u64> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM vectors WHERE project = ?1")?;
            stmt.query_map(params![project], |row| {
                row.get::<_, i64>(0).map(|id| id as u64)
            })?
            .filter_map(|r| r.ok())
            .collect()
        };
        let count = self
            .conn
            .execute("DELETE FROM vectors WHERE project = ?1", params![project])?;
        self.remove_from_index(&ids);
        Ok(count)
    }

    #[allow(dead_code)]
    pub fn remove_by_ids(&self, ids: &[u64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "DELETE FROM vectors WHERE id IN ({})",
            placeholders.join(", ")
        );
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = ids
            .iter()
            .map(|&id| Box::new(id as i64) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let count = self.conn.execute(&sql, param_refs.as_slice())?;
        self.remove_from_index(ids);
        Ok(count)
    }

    /// Remove IDs from the in-memory index and id_map.
    fn remove_from_index(&self, ids: &[u64]) {
        if let Some(ref mut idx) = *self.index.borrow_mut() {
            let _ = idx.remove(ids);
        }
        let mut id_map = self.id_map.borrow_mut();
        for &id in ids {
            id_map.remove(&id);
        }
    }

    #[allow(dead_code)]
    pub fn search(&self, query: &[f32], limit: usize) -> Result<Vec<VectorResult>> {
        self.search_with_filter(query, limit, None)
    }

    pub fn search_with_filter(
        &self,
        query: &[f32],
        limit: usize,
        project_filter: Option<&str>,
    ) -> Result<Vec<VectorResult>> {
        self.ensure_index()?;

        let index = self.index.borrow();
        let idx = index.as_ref().unwrap();
        let id_map = self.id_map.borrow();

        let hits = if let Some(project) = project_filter {
            // Build allowlist of IDs belonging to this project.
            let allowlist: Vec<u64> = id_map
                .iter()
                .filter(|(_, (proj, _, _))| proj == project)
                .map(|(&id, _)| id)
                .collect();
            if allowlist.is_empty() {
                return Ok(Vec::new());
            }
            idx.search_filtered(query, limit, &allowlist)?
        } else {
            idx.search(query, limit)?
        };

        let results = hits
            .into_iter()
            .filter_map(|hit| {
                id_map
                    .get(&hit.id)
                    .map(|(project, file, chunk_id)| VectorResult {
                        project: project.clone(),
                        file: file.clone(),
                        chunk_id: *chunk_id,
                        score: hit.score,
                    })
            })
            .collect();
        Ok(results)
    }

    #[allow(dead_code)]
    pub fn search_filtered(
        &self,
        query: &[f32],
        limit: usize,
        allowlist: &[u64],
    ) -> Result<Vec<VectorResult>> {
        if allowlist.is_empty() {
            return Ok(Vec::new());
        }
        self.ensure_index()?;

        let index = self.index.borrow();
        let idx = index.as_ref().unwrap();
        let id_map = self.id_map.borrow();

        let hits = idx.search_filtered(query, limit, allowlist)?;
        let results = hits
            .into_iter()
            .filter_map(|hit| {
                id_map
                    .get(&hit.id)
                    .map(|(project, file, chunk_id)| VectorResult {
                        project: project.clone(),
                        file: file.clone(),
                        chunk_id: *chunk_id,
                        score: hit.score,
                    })
            })
            .collect();
        Ok(results)
    }

    #[allow(dead_code)]
    pub fn meta(&self) -> Result<VectorMeta> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM vectors", [], |row| row.get(0))?;
        Ok(VectorMeta {
            model: self.model.clone(),
            dimension: self.dimension,
            count,
        })
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> Result<bool> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM vectors", [], |row| row.get(0))?;
        Ok(count == 0)
    }

    #[allow(dead_code)]
    pub fn clear(&self) -> Result<()> {
        self.conn.execute("DELETE FROM vectors", [])?;
        *self.index.borrow_mut() = None;
        self.id_map.borrow_mut().clear();
        Ok(())
    }

    /// Save the TurboQuant index to disk.
    #[allow(dead_code)]
    pub fn save_index(&self) -> Result<()> {
        if let Some(ref idx) = *self.index.borrow() {
            idx.save(&self.index_path)?;
        }
        Ok(())
    }

    fn encode_embedding(embedding: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(embedding.len() * 4);
        for &v in embedding {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes
    }

    fn decode_embedding(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Shared helper functions
// ---------------------------------------------------------------------------

/// Compute cosine similarity between two vectors.
///
/// Delegates to `llm_kernel::embedding::cosine_similarity` which accumulates
/// in f64 for better ranking stability with high-dimensional (384–1024) vectors.
#[cfg(all(feature = "vector", not(feature = "turboquant")))]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    llm_kernel::embedding::cosine_similarity(a, b) as f32
}

/// Compute cosine similarity (TurboQuant backend — delegates to llm-kernel).
#[cfg(feature = "turboquant")]
#[allow(dead_code)]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    llm_kernel::embedding::cosine_similarity(a, b) as f32
}

/// Reciprocal Rank Fusion (RRF) for combining BM25 and vector search results.
#[cfg(any(feature = "vector", feature = "turboquant"))]
#[cfg_attr(not(feature = "embed"), allow(dead_code))]
pub fn reciprocal_rank_fusion(
    bm25_results: &[(String, String, u64, f32)],
    vector_results: &[VectorResult],
    k: u32,
) -> Vec<(String, String, u64, f32)> {
    use std::collections::HashMap;

    let bm25_weight: f32 = 0.6;
    let vector_weight: f32 = 0.4;

    let mut scores: HashMap<(String, String, u64), f32> = HashMap::new();

    for (rank, (project, file, chunk_id, _score)) in bm25_results.iter().enumerate() {
        let key = (project.clone(), file.clone(), *chunk_id);
        let rrf = bm25_weight / (k as f32 + (rank + 1) as f32);
        *scores.entry(key).or_default() += rrf;
    }

    for (rank, result) in vector_results.iter().enumerate() {
        let key = (result.project.clone(), result.file.clone(), result.chunk_id);
        let rrf = vector_weight / (k as f32 + (rank + 1) as f32);
        *scores.entry(key).or_default() += rrf;
    }

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
        let a = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert!(cosine_similarity(&a, &b).abs() < 0.0001);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let b = vec![-1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - (-1.0)).abs() < 0.0001);
    }

    #[test]
    fn test_cosine_similarity_partial() {
        let a = vec![1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
    }

    /// Reopening with a different model clears all vectors atomically.
    #[test]
    fn test_open_model_change_clears_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vec.db");

        {
            let mut store = VectorStore::open(&db_path, "model-a", 8).unwrap();
            store
                .batch_upsert(
                    vec![(
                        "proj".into(),
                        "f.md".into(),
                        0u64,
                        vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                    )]
                    .into_iter(),
                )
                .unwrap();
            assert!(!store.is_empty().unwrap());
        }
        {
            let store = VectorStore::open(&db_path, "model-b", 8).unwrap();
            assert!(
                store.is_empty().unwrap(),
                "vectors must be cleared when model changes"
            );
        }
    }

    /// search_with_filter with project_filter returns only matching project rows.
    #[test]
    fn test_search_with_project_filter() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vec2.db");

        let mut store = VectorStore::open(&db_path, "test-model", 8).unwrap();
        store
            .batch_upsert(
                vec![
                    (
                        "proj-a".into(),
                        "a.md".into(),
                        0u64,
                        vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                    ),
                    (
                        "proj-b".into(),
                        "b.md".into(),
                        0u64,
                        vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                    ),
                ]
                .into_iter(),
            )
            .unwrap();

        let results = store
            .search_with_filter(
                &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                10,
                Some("proj-a"),
            )
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].project, "proj-a");

        let all = store
            .search_with_filter(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 10, None)
            .unwrap();
        assert_eq!(all.len(), 2);
    }

    /// remove_by_ids: insert vectors, remove some, verify they're gone.
    #[test]
    fn test_remove_by_ids() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vec_rm.db");

        let mut store = VectorStore::open(&db_path, "rm-model", 8).unwrap();
        store
            .batch_upsert(
                vec![
                    (
                        "proj".into(),
                        "a.md".into(),
                        0u64,
                        vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                    ),
                    (
                        "proj".into(),
                        "b.md".into(),
                        1u64,
                        vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                    ),
                    (
                        "proj".into(),
                        "c.md".into(),
                        2u64,
                        vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                    ),
                ]
                .into_iter(),
            )
            .unwrap();

        let row_ids: Vec<u64> = {
            let mut stmt = store
                .conn
                .prepare("SELECT id FROM vectors ORDER BY chunk_id")
                .unwrap();
            stmt.query_map([], |row| row.get::<_, i64>(0).map(|id| id as u64))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(row_ids.len(), 3);

        let removed = store.remove_by_ids(&row_ids[..2]).unwrap();
        assert_eq!(removed, 2);

        let results = store
            .search_with_filter(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0], 10, None)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].chunk_id, 2);
    }

    /// remove_by_ids with empty slice is a no-op.
    #[test]
    fn test_remove_by_ids_empty() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vec_rm_empty.db");

        let mut store = VectorStore::open(&db_path, "rm-model", 8).unwrap();
        store
            .batch_upsert(
                vec![(
                    "proj".into(),
                    "a.md".into(),
                    0u64,
                    vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                )]
                .into_iter(),
            )
            .unwrap();

        let removed = store.remove_by_ids(&[]).unwrap();
        assert_eq!(removed, 0);
        assert!(!store.is_empty().unwrap());
    }

    /// search_filtered: only vectors in the allowlist are scored.
    #[test]
    fn test_search_filtered_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vec_filt.db");

        let mut store = VectorStore::open(&db_path, "filt-model", 8).unwrap();
        store
            .batch_upsert(
                vec![
                    (
                        "proj".into(),
                        "a.md".into(),
                        0u64,
                        vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                    ),
                    (
                        "proj".into(),
                        "b.md".into(),
                        1u64,
                        vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                    ),
                    (
                        "proj".into(),
                        "c.md".into(),
                        2u64,
                        vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                    ),
                ]
                .into_iter(),
            )
            .unwrap();

        let row_ids: Vec<u64> = {
            let mut stmt = store
                .conn
                .prepare("SELECT id FROM vectors ORDER BY chunk_id")
                .unwrap();
            stmt.query_map([], |row| row.get::<_, i64>(0).map(|id| id as u64))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };

        let results = store
            .search_filtered(
                &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                10,
                &[row_ids[0], row_ids[2]],
            )
            .unwrap();
        // With TurboQuant 4-bit quantization, orthogonal vectors may receive small
        // positive scores. Assert that a.md (chunk 0) is the top result regardless.
        assert!(!results.is_empty(), "should return at least one result");
        assert_eq!(results[0].chunk_id, 0, "a.md should be the top hit");
    }

    /// search_filtered with empty allowlist returns empty results.
    #[test]
    fn test_search_filtered_empty_allowlist() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vec_filt_empty.db");

        let mut store = VectorStore::open(&db_path, "filt-model", 8).unwrap();
        store
            .batch_upsert(
                vec![(
                    "proj".into(),
                    "a.md".into(),
                    0u64,
                    vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                )]
                .into_iter(),
            )
            .unwrap();

        let results = store
            .search_filtered(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 10, &[])
            .unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_rrf_combines_rankings() {
        let bm25 = vec![
            ("p1".into(), "a.md".into(), 0, 3.0),
            ("p1".into(), "b.md".into(), 0, 2.0),
        ];
        let vector = vec![
            VectorResult {
                project: "p1".into(),
                file: "b.md".into(),
                chunk_id: 0,
                score: 0.9,
            },
            VectorResult {
                project: "p1".into(),
                file: "c.md".into(),
                chunk_id: 0,
                score: 0.8,
            },
        ];
        let fused = reciprocal_rank_fusion(&bm25, &vector, 60);
        assert_eq!(fused[0].1, "b.md");
        assert!(fused.len() >= 2);
    }
}
