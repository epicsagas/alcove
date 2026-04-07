pub mod config;
pub mod embedding;
pub mod index;

#[cfg(feature = "alcove-full")]
pub mod vector;

pub use config::{default_docs_root, load_config, DocConfig};
pub use embedding::EmbeddingModelChoice;

#[cfg(feature = "alcove-full")]
pub use embedding::{EmbeddingConfig, EmbeddingService, ModelState};

pub use index::{
    build_index, ensure_index_fresh, index_exists, is_index_stale, search_indexed,
};

#[cfg(feature = "alcove-full")]
pub use index::search_hybrid;

#[cfg(feature = "alcove-full")]
pub use vector::{cosine_similarity, reciprocal_rank_fusion, VectorMeta, VectorResult, VectorStore};
