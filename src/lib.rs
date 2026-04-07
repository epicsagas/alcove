pub mod config;
pub mod embedding;
pub mod index;
pub mod vector;

pub use config::{default_docs_root, load_config, DocConfig};
pub use embedding::{EmbeddingConfig, EmbeddingModelChoice, EmbeddingService, ModelState};
pub use index::{
    build_index, ensure_index_fresh, index_exists, is_index_stale, search_indexed, search_hybrid,
};
pub use vector::{cosine_similarity, reciprocal_rank_fusion, VectorMeta, VectorResult, VectorStore};
