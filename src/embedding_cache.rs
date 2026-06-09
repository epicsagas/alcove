//! Persistent content-hash embedding cache.
//!
//! Stores (blake3_hash, model_name) → embedding_vector in a separate SQLite
//! database that survives full index rebuilds. This avoids re-running ONNX
//! inference for unchanged chunk content.

use anyhow::Result;
use rusqlite::params;

/// Persistent embedding cache backed by SQLite.
///
/// The database file (`embedding_cache.db`) lives alongside `vectors.db` but
/// is **not** deleted during `force_rebuild`, so cached embeddings survive
/// across rebuilds.
pub struct EmbeddingCache {
    conn: rusqlite::Connection,
}

impl EmbeddingCache {
    /// Open (or create) the embedding cache at `path`.
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS embedding_cache (
                 content_hash BLOB NOT NULL,
                 model        TEXT  NOT NULL,
                 embedding    BLOB NOT NULL,
                 PRIMARY KEY (content_hash, model)
             ) WITHOUT ROWID;",
        )?;
        Ok(Self { conn })
    }

    /// Look up a single cached embedding.
    pub fn get(&self, hash: &[u8; 32], model: &str) -> Result<Option<Vec<f32>>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT embedding FROM embedding_cache WHERE content_hash = ?1 AND model = ?2",
        )?;
        let mut rows = stmt.query(params![hash.as_slice(), model])?;
        match rows.next()? {
            Some(row) => {
                let blob: Vec<u8> = row.get(0)?;
                Ok(Some(decode_embedding(&blob)))
            }
            None => Ok(None),
        }
    }

    /// Batch store: writes all entries inside a single transaction.
    pub fn put_batch(
        &self,
        hashes: &[[u8; 32]],
        model: &str,
        embeddings: &[Vec<f32>],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO embedding_cache (content_hash, model, embedding) VALUES (?1, ?2, ?3)",
            )?;
            for (hash, emb) in hashes.iter().zip(embeddings) {
                let blob = encode_embedding(emb);
                stmt.execute(params![hash.as_slice(), model, blob])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Encoding helpers (same pattern as vector.rs but standalone)
// ---------------------------------------------------------------------------

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
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}
