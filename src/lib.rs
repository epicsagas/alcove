pub mod config;
pub mod embedding;
pub mod index;

pub use config::{default_docs_root, load_config, DocConfig};
pub use embedding::{EmbeddingConfig, EmbeddingModelChoice, EmbeddingService, ModelState};
pub use index::{
    build_index, ensure_index_fresh, index_exists, is_index_stale, search_indexed,
};
