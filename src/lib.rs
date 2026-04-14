pub mod config;
pub mod embedding;
pub mod index;
pub mod lint;
pub mod policy;
pub mod promote;
pub mod vault;

#[cfg(feature = "alcove-full")]
pub mod vector;

pub use config::{default_docs_root, load_config, DocConfig};

pub use index::{
    build_index, ensure_index_fresh, index_exists, is_index_stale, search_indexed,
};

pub use policy::validate_doc_name;

pub use vault::{
    add_to_vault, create_vault, link_vault, list_vaults, remove_vault, vaults_root, VaultInfo,
};

#[cfg(feature = "alcove-full")]
pub use embedding::{EmbeddingModelChoice, EmbeddingService, ModelState};
pub use config::EmbeddingConfig;

#[cfg(feature = "alcove-full")]
pub use index::search_hybrid;

#[cfg(feature = "alcove-full")]
pub use vector::{cosine_similarity, reciprocal_rank_fusion, VectorMeta, VectorResult, VectorStore};
