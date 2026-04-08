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
pub struct VectorMeta {
    /// Model name used for embeddings
    pub model: String,
    /// Embedding dimension
    pub dimension: usize,
    /// Number of vectors stored
    pub count: usize,
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

#[cfg(feature = "alcove-full")]
pub struct VectorStore {
    conn: Connection,
    dimension: usize,
    model: String,
}

#[cfg(feature = "alcove-full")]
impl VectorStore {
    /// Open or create a vector store at the given path
    pub fn open(path: &Path, model: &str, dimension: usize) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;

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

        // If model or dimension changed, clear existing vectors
        if let Some(em) = existing_model {
            if em != model {
                conn.execute("DELETE FROM vectors", [])?;
            }
        }
        if let Some(ed) = existing_dim {
            if ed != dimension {
                conn.execute("DELETE FROM vectors", [])?;
            }
        }

        // Update metadata
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('model', ?1)",
            params![model],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('dimension', ?1)",
            params![dimension.to_string()],
        )?;

        Ok(Self {
            conn,
            dimension,
            model: model.to_string(),
        })
    }

    /// Insert or update a vector
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
            params![project, file, chunk_id, blob],
        )?;

        Ok(())
    }

    /// Delete vectors for a file
    pub fn delete_file(&self, project: &str, file: &str) -> Result<usize> {
        let count = self.conn.execute(
            "DELETE FROM vectors WHERE project = ?1 AND file = ?2",
            params![project, file],
        )?;
        Ok(count)
    }

    /// Delete all vectors for a project
    pub fn delete_project(&self, project: &str) -> Result<usize> {
        let count = self.conn.execute(
            "DELETE FROM vectors WHERE project = ?1",
            params![project],
        )?;
        Ok(count)
    }

    /// Search for similar vectors using HNSW (large datasets) or linear scan (small)
    pub fn search(&self, query: &[f32], limit: usize) -> Result<Vec<VectorResult>> {
        let count: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM vectors",
            [],
            |row| row.get(0),
        )?;

        // Use HNSW for large datasets (>= 5000 vectors)
        #[cfg(feature = "alcove-full")]
        if count >= 5000 {
            return self.search_hnsw(query, limit);
        }

        // Fall back to linear search for small datasets
        self.search_linear(query, limit)
    }

    /// Linear search (O(n)) - good for small datasets
    fn search_linear(&self, query: &[f32], limit: usize) -> Result<Vec<VectorResult>> {
        let mut stmt = self.conn.prepare(
            "SELECT project, file, chunk_id, embedding FROM vectors"
        )?;

        let rows = stmt.query_map([], |row| {
            let project: String = row.get(0)?;
            let file: String = row.get(1)?;
            let chunk_id: u64 = row.get(2)?;
            let blob: Vec<u8> = row.get(3)?;
            Ok((project, file, chunk_id, blob))
        })?;

        let mut results: Vec<VectorResult> = Vec::new();

        for row_result in rows {
            let (project, file, chunk_id, blob) = row_result?;
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

    /// HNSW search (O(log n)) - good for large datasets
    #[cfg(feature = "alcove-full")]
    fn search_hnsw(&self, query: &[f32], limit: usize) -> Result<Vec<VectorResult>> {
        use hnsw_rs::prelude::*;

        // Load all vectors from SQLite
        let mut stmt = self.conn.prepare(
            "SELECT id, project, file, chunk_id, embedding FROM vectors"
        )?;

        let rows = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let project: String = row.get(1)?;
            let file: String = row.get(2)?;
            let chunk_id: u64 = row.get(3)?;
            let blob: Vec<u8> = row.get(4)?;
            Ok((id, project, file, chunk_id, blob))
        })?;

        let mut vectors: Vec<(i64, String, String, u64, Vec<f32>)> = Vec::new();
        for row_result in rows {
            let (id, project, file, chunk_id, blob) = row_result?;
            let embedding = Self::decode_embedding(&blob);
            vectors.push((id, project, file, chunk_id, embedding));
        }

        if vectors.is_empty() {
            return Ok(Vec::new());
        }

        // Build HNSW index
        let ef_search = (limit * 2).max(50);
        let hnsw = Hnsw::<f32, DistCosine>::new(
            vectors.len().max(1),  // max elements
            self.dimension,
            16,                     // M (connectivity)
            ef_search,              // ef (search parameter)
            DistCosine {},          // distance function
        );

        // Insert all vectors
        for (id, _, _, _, embedding) in &vectors {
            hnsw.insert((embedding.as_slice(), *id as usize));
        }

        // Search - hnsw_rs API: search(data, knbn, ef_arg)
        let results = hnsw.search(query, limit, ef_search);

        // Map back to VectorResult
        let mut vector_results: Vec<VectorResult> = Vec::new();
        for neighbor in results {
            if let Some((_id, project, file, chunk_id, _)) = vectors
                .iter()
                .find(|(vid, _, _, _, _)| *vid as usize == neighbor.d_id)
            {
                vector_results.push(VectorResult {
                    project: project.clone(),
                    file: file.clone(),
                    chunk_id: *chunk_id,
                    score: 1.0 - neighbor.distance, // Convert distance to similarity
                });
            }
        }

        Ok(vector_results)
    }

    /// Get store metadata
    pub fn meta(&self) -> Result<VectorMeta> {
        let count: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM vectors",
            [],
            |row| row.get(0),
        )?;

        Ok(VectorMeta {
            model: self.model.clone(),
            dimension: self.dimension,
            count,
        })
    }

    /// Check if store is empty
    pub fn is_empty(&self) -> bool {
        let count: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM vectors",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
        count == 0
    }

    /// Clear all vectors
    pub fn clear(&self) -> Result<()> {
        self.conn.execute("DELETE FROM vectors", [])?;
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
