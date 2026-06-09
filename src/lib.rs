pub mod config;
#[cfg(feature = "embed")]
pub mod embedding;
pub mod index;
pub mod lint;
pub mod platform;
pub mod policy;
pub mod promote;
pub mod transpile;
pub mod vault;

pub mod code_index;

#[cfg(feature = "vector")]
pub mod vector;

pub use config::{DocConfig, default_docs_root, load_config};

pub use index::{build_index, ensure_index_fresh, index_exists, is_index_stale, search_indexed};

pub use policy::validate_doc_name;

pub use vault::{
    VaultInfo, add_to_vault, create_vault, link_vault, list_vaults, remove_vault, vaults_root,
};

#[cfg(feature = "embed")]
pub use config::EmbeddingConfig;
#[cfg(feature = "embed")]
pub use embedding::{
    EmbeddingModel, EmbeddingService, ModelState, parse_legacy_model, resolve_model,
};

#[cfg(feature = "embed")]
pub use index::search_hybrid;

#[cfg(feature = "vector")]
pub use vector::{
    VectorMeta, VectorResult, VectorStore, cosine_similarity, reciprocal_rank_fusion,
};

// Re-export llm-kernel vector index types for downstream consumers.
#[cfg(feature = "vector")]
pub use llm_kernel::embedding::vector_index::{SearchHit, VectorIndex};

pub use code_index::{CodeIndexResult, index_code_structure, index_code_structure_with_lang};
