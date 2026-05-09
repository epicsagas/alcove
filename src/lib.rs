pub mod config;
#[cfg(feature = "alcove-full")]
pub mod embedding;
pub mod index;
pub mod lint;
pub mod policy;
pub mod promote;
pub mod vault;

#[cfg(feature = "alcove-full")]
pub mod vector;

pub use config::{DocConfig, default_docs_root, load_config};

pub use index::{build_index, ensure_index_fresh, index_exists, is_index_stale, search_indexed};

pub use policy::validate_doc_name;

pub use vault::{
    VaultInfo, add_to_vault, create_vault, link_vault, list_vaults, remove_vault, vaults_root,
};

#[cfg(feature = "alcove-full")]
pub use config::EmbeddingConfig;
#[cfg(feature = "alcove-full")]
pub use embedding::{EmbeddingModelChoice, EmbeddingService, ModelState};

#[cfg(feature = "alcove-full")]
pub use index::search_hybrid;

#[cfg(feature = "alcove-full")]
pub use vector::{
    VectorMeta, VectorResult, VectorStore, cosine_similarity, reciprocal_rank_fusion,
};
