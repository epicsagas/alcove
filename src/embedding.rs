//! Embedding service for hybrid search (embed feature)
//!
//! Uses fastembed-rs (ONNX Runtime) for local inference with lazy model
//! download via HuggingFace Hub and LRU query caching. Graceful degradation
//! ensures BM25-only search works even when models aren't ready.

#[cfg(feature = "embed")]
use std::path::{Path, PathBuf};
#[cfg(feature = "embed")]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(feature = "embed")]
use std::time::{Duration, Instant};

#[cfg(feature = "embed")]
use anyhow::{Context, Result};
#[cfg(feature = "embed")]
use fastembed::EmbeddingModel as FastEmbedModel;
#[cfg(feature = "embed")]
use fastembed::TextEmbedding;
#[cfg(feature = "embed")]
use fastembed::TextInitOptions;

// ---------------------------------------------------------------------------
// ModelState
// ---------------------------------------------------------------------------

/// Model state for graceful degradation
#[cfg(feature = "embed")]
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ModelState {
    #[default]
    NotLoaded,
    Loading,
    Ready,
    Disabled,
    Failed(String),
}

#[cfg(feature = "embed")]
impl std::fmt::Display for ModelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotLoaded => write!(f, "not_loaded"),
            Self::Loading => write!(f, "loading"),
            Self::Ready => write!(f, "ready"),
            Self::Disabled => write!(f, "disabled"),
            Self::Failed(e) => write!(f, "failed: {}", e),
        }
    }
}

// ---------------------------------------------------------------------------
// EmbeddingModelChoice
// ---------------------------------------------------------------------------

/// Supported embedding models via fastembed-rs (ONNX Runtime).
///
/// Config names are matched by `parse()`. Models prefixed with `Arctic` are
/// backward-compatible aliases for the Snowflake Arctic Embed series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmbeddingModelChoice {
    // ── MiniLM ───────────────────────────────────────────────
    AllMiniLML6V2,
    AllMiniLML6V2Q,
    AllMiniLML12V2,
    AllMiniLML12V2Q,
    // ── MPNet ────────────────────────────────────────────────
    AllMpnetBaseV2,
    // ── E5 multilingual ─────────────────────────────────────
    MultilingualE5Small,
    MultilingualE5Base,
    MultilingualE5Large,
    // ── BGE English ──────────────────────────────────────────
    BGESmallENV15,
    BGESmallENV15Q,
    BGEBaseENV15,
    BGEBaseENV15Q,
    BGELargeENV15,
    BGELargeENV15Q,
    // ── BGE Chinese ──────────────────────────────────────────
    BGESmallZHV15,
    BGELargeZHV15,
    // ── BGE Multilingual ────────────────────────────────────
    BGEM3,
    // ── ModernBERT ───────────────────────────────────────────
    ModernBertEmbedLarge,
    // ── Nomic ────────────────────────────────────────────────
    NomicEmbedTextV1,
    NomicEmbedTextV15,
    NomicEmbedTextV15Q,
    // ── Paraphrase multilingual ─────────────────────────────
    ParaphraseMLMiniLML12V2,
    ParaphraseMLMiniLML12V2Q,
    ParaphraseMLMpnetBaseV2,
    // ── MixedBread ───────────────────────────────────────────
    MxbaiEmbedLargeV1,
    MxbaiEmbedLargeV1Q,
    // ── GTE ──────────────────────────────────────────────────
    GTEBaseENV15,
    GTEBaseENV15Q,
    GTELargeENV15,
    GTELargeENV15Q,
    // ── Jina ─────────────────────────────────────────────────
    JinaEmbeddingsV2BaseCode,
    JinaEmbeddingsV2BaseEN,
    // ── Gemma ────────────────────────────────────────────────
    EmbeddingGemma300M,
    // ── Snowflake Arctic (backward-compat aliases) ──────────
    #[default]
    ArcticEmbedXS,
    ArcticEmbedXSQ,
    ArcticEmbedS,
    ArcticEmbedSQ,
    ArcticEmbedM,
    ArcticEmbedMQ,
    ArcticEmbedMLong,
    ArcticEmbedMLongQ,
    ArcticEmbedL,
    ArcticEmbedLQ,
}

impl EmbeddingModelChoice {
    pub fn dimension(&self) -> usize {
        match self {
            Self::AllMiniLML6V2
            | Self::AllMiniLML6V2Q
            | Self::AllMiniLML12V2
            | Self::AllMiniLML12V2Q
            | Self::MultilingualE5Small
            | Self::BGESmallENV15
            | Self::BGESmallENV15Q
            | Self::ParaphraseMLMiniLML12V2
            | Self::ParaphraseMLMiniLML12V2Q
            | Self::ArcticEmbedXS
            | Self::ArcticEmbedXSQ
            | Self::ArcticEmbedS
            | Self::ArcticEmbedSQ => 384,

            Self::BGESmallZHV15 => 512,

            Self::AllMpnetBaseV2
            | Self::MultilingualE5Base
            | Self::BGEBaseENV15
            | Self::BGEBaseENV15Q
            | Self::NomicEmbedTextV1
            | Self::NomicEmbedTextV15
            | Self::NomicEmbedTextV15Q
            | Self::ParaphraseMLMpnetBaseV2
            | Self::GTEBaseENV15
            | Self::GTEBaseENV15Q
            | Self::JinaEmbeddingsV2BaseCode
            | Self::JinaEmbeddingsV2BaseEN
            | Self::EmbeddingGemma300M
            | Self::ArcticEmbedM
            | Self::ArcticEmbedMQ
            | Self::ArcticEmbedMLong
            | Self::ArcticEmbedMLongQ => 768,

            Self::MultilingualE5Large
            | Self::BGELargeENV15
            | Self::BGELargeENV15Q
            | Self::BGELargeZHV15
            | Self::BGEM3
            | Self::ModernBertEmbedLarge
            | Self::MxbaiEmbedLargeV1
            | Self::MxbaiEmbedLargeV1Q
            | Self::GTELargeENV15
            | Self::GTELargeENV15Q
            | Self::ArcticEmbedL
            | Self::ArcticEmbedLQ => 1024,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "AllMiniLML6V2" => Some(Self::AllMiniLML6V2),
            "AllMiniLML6V2Q" => Some(Self::AllMiniLML6V2Q),
            "AllMiniLML12V2" => Some(Self::AllMiniLML12V2),
            "AllMiniLML12V2Q" => Some(Self::AllMiniLML12V2Q),
            "AllMpnetBaseV2" => Some(Self::AllMpnetBaseV2),
            "MultilingualE5Small" => Some(Self::MultilingualE5Small),
            "MultilingualE5Base" => Some(Self::MultilingualE5Base),
            "MultilingualE5Large" => Some(Self::MultilingualE5Large),
            "BGESmallENV15" => Some(Self::BGESmallENV15),
            "BGESmallENV15Q" => Some(Self::BGESmallENV15Q),
            "BGEBaseENV15" => Some(Self::BGEBaseENV15),
            "BGEBaseENV15Q" => Some(Self::BGEBaseENV15Q),
            "BGELargeENV15" => Some(Self::BGELargeENV15),
            "BGELargeENV15Q" => Some(Self::BGELargeENV15Q),
            "BGESmallZHV15" => Some(Self::BGESmallZHV15),
            "BGELargeZHV15" => Some(Self::BGELargeZHV15),
            "BGEM3" => Some(Self::BGEM3),
            "ModernBertEmbedLarge" => Some(Self::ModernBertEmbedLarge),
            "NomicEmbedTextV1" => Some(Self::NomicEmbedTextV1),
            "NomicEmbedTextV15" => Some(Self::NomicEmbedTextV15),
            "NomicEmbedTextV15Q" => Some(Self::NomicEmbedTextV15Q),
            "ParaphraseMLMiniLML12V2" => Some(Self::ParaphraseMLMiniLML12V2),
            "ParaphraseMLMiniLML12V2Q" => Some(Self::ParaphraseMLMiniLML12V2Q),
            "ParaphraseMLMpnetBaseV2" => Some(Self::ParaphraseMLMpnetBaseV2),
            "MxbaiEmbedLargeV1" => Some(Self::MxbaiEmbedLargeV1),
            "MxbaiEmbedLargeV1Q" => Some(Self::MxbaiEmbedLargeV1Q),
            "GTEBaseENV15" => Some(Self::GTEBaseENV15),
            "GTEBaseENV15Q" => Some(Self::GTEBaseENV15Q),
            "GTELargeENV15" => Some(Self::GTELargeENV15),
            "GTELargeENV15Q" => Some(Self::GTELargeENV15Q),
            "JinaEmbeddingsV2BaseCode" => Some(Self::JinaEmbeddingsV2BaseCode),
            "JinaEmbeddingsV2BaseEN" => Some(Self::JinaEmbeddingsV2BaseEN),
            "EmbeddingGemma300M" => Some(Self::EmbeddingGemma300M),
            "ArcticEmbedXS" => Some(Self::ArcticEmbedXS),
            "ArcticEmbedXSQ" => Some(Self::ArcticEmbedXSQ),
            "ArcticEmbedS" => Some(Self::ArcticEmbedS),
            "ArcticEmbedSQ" => Some(Self::ArcticEmbedSQ),
            "ArcticEmbedM" => Some(Self::ArcticEmbedM),
            "ArcticEmbedMQ" => Some(Self::ArcticEmbedMQ),
            "ArcticEmbedMLong" => Some(Self::ArcticEmbedMLong),
            "ArcticEmbedMLongQ" => Some(Self::ArcticEmbedMLongQ),
            "ArcticEmbedL" => Some(Self::ArcticEmbedL),
            "ArcticEmbedLQ" => Some(Self::ArcticEmbedLQ),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllMiniLML6V2 => "AllMiniLML6V2",
            Self::AllMiniLML6V2Q => "AllMiniLML6V2Q",
            Self::AllMiniLML12V2 => "AllMiniLML12V2",
            Self::AllMiniLML12V2Q => "AllMiniLML12V2Q",
            Self::AllMpnetBaseV2 => "AllMpnetBaseV2",
            Self::MultilingualE5Small => "MultilingualE5Small",
            Self::MultilingualE5Base => "MultilingualE5Base",
            Self::MultilingualE5Large => "MultilingualE5Large",
            Self::BGESmallENV15 => "BGESmallENV15",
            Self::BGESmallENV15Q => "BGESmallENV15Q",
            Self::BGEBaseENV15 => "BGEBaseENV15",
            Self::BGEBaseENV15Q => "BGEBaseENV15Q",
            Self::BGELargeENV15 => "BGELargeENV15",
            Self::BGELargeENV15Q => "BGELargeENV15Q",
            Self::BGESmallZHV15 => "BGESmallZHV15",
            Self::BGELargeZHV15 => "BGELargeZHV15",
            Self::BGEM3 => "BGEM3",
            Self::ModernBertEmbedLarge => "ModernBertEmbedLarge",
            Self::NomicEmbedTextV1 => "NomicEmbedTextV1",
            Self::NomicEmbedTextV15 => "NomicEmbedTextV15",
            Self::NomicEmbedTextV15Q => "NomicEmbedTextV15Q",
            Self::ParaphraseMLMiniLML12V2 => "ParaphraseMLMiniLML12V2",
            Self::ParaphraseMLMiniLML12V2Q => "ParaphraseMLMiniLML12V2Q",
            Self::ParaphraseMLMpnetBaseV2 => "ParaphraseMLMpnetBaseV2",
            Self::MxbaiEmbedLargeV1 => "MxbaiEmbedLargeV1",
            Self::MxbaiEmbedLargeV1Q => "MxbaiEmbedLargeV1Q",
            Self::GTEBaseENV15 => "GTEBaseENV15",
            Self::GTEBaseENV15Q => "GTEBaseENV15Q",
            Self::GTELargeENV15 => "GTELargeENV15",
            Self::GTELargeENV15Q => "GTELargeENV15Q",
            Self::JinaEmbeddingsV2BaseCode => "JinaEmbeddingsV2BaseCode",
            Self::JinaEmbeddingsV2BaseEN => "JinaEmbeddingsV2BaseEN",
            Self::EmbeddingGemma300M => "EmbeddingGemma300M",
            Self::ArcticEmbedXS => "ArcticEmbedXS",
            Self::ArcticEmbedXSQ => "ArcticEmbedXSQ",
            Self::ArcticEmbedS => "ArcticEmbedS",
            Self::ArcticEmbedSQ => "ArcticEmbedSQ",
            Self::ArcticEmbedM => "ArcticEmbedM",
            Self::ArcticEmbedMQ => "ArcticEmbedMQ",
            Self::ArcticEmbedMLong => "ArcticEmbedMLong",
            Self::ArcticEmbedMLongQ => "ArcticEmbedMLongQ",
            Self::ArcticEmbedL => "ArcticEmbedL",
            Self::ArcticEmbedLQ => "ArcticEmbedLQ",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::AllMiniLML6V2,
            Self::AllMiniLML6V2Q,
            Self::AllMiniLML12V2,
            Self::AllMiniLML12V2Q,
            Self::AllMpnetBaseV2,
            Self::MultilingualE5Small,
            Self::MultilingualE5Base,
            Self::MultilingualE5Large,
            Self::BGESmallENV15,
            Self::BGESmallENV15Q,
            Self::BGEBaseENV15,
            Self::BGEBaseENV15Q,
            Self::BGELargeENV15,
            Self::BGELargeENV15Q,
            Self::BGESmallZHV15,
            Self::BGELargeZHV15,
            Self::BGEM3,
            Self::ModernBertEmbedLarge,
            Self::NomicEmbedTextV1,
            Self::NomicEmbedTextV15,
            Self::NomicEmbedTextV15Q,
            Self::ParaphraseMLMiniLML12V2,
            Self::ParaphraseMLMiniLML12V2Q,
            Self::ParaphraseMLMpnetBaseV2,
            Self::MxbaiEmbedLargeV1,
            Self::MxbaiEmbedLargeV1Q,
            Self::GTEBaseENV15,
            Self::GTEBaseENV15Q,
            Self::GTELargeENV15,
            Self::GTELargeENV15Q,
            Self::JinaEmbeddingsV2BaseCode,
            Self::JinaEmbeddingsV2BaseEN,
            Self::EmbeddingGemma300M,
            Self::ArcticEmbedXS,
            Self::ArcticEmbedXSQ,
            Self::ArcticEmbedS,
            Self::ArcticEmbedSQ,
            Self::ArcticEmbedM,
            Self::ArcticEmbedMQ,
            Self::ArcticEmbedMLong,
            Self::ArcticEmbedMLongQ,
            Self::ArcticEmbedL,
            Self::ArcticEmbedLQ,
        ]
    }

    /// Map to the corresponding fastembed EmbeddingModel variant.
    #[cfg(feature = "embed")]
    pub(crate) fn to_fastembed(self) -> FastEmbedModel {
        match self {
            Self::AllMiniLML6V2 => FastEmbedModel::AllMiniLML6V2,
            Self::AllMiniLML6V2Q => FastEmbedModel::AllMiniLML6V2Q,
            Self::AllMiniLML12V2 => FastEmbedModel::AllMiniLML12V2,
            Self::AllMiniLML12V2Q => FastEmbedModel::AllMiniLML12V2Q,
            Self::AllMpnetBaseV2 => FastEmbedModel::AllMpnetBaseV2,
            Self::MultilingualE5Small => FastEmbedModel::MultilingualE5Small,
            Self::MultilingualE5Base => FastEmbedModel::MultilingualE5Base,
            Self::MultilingualE5Large => FastEmbedModel::MultilingualE5Large,
            Self::BGESmallENV15 => FastEmbedModel::BGESmallENV15,
            Self::BGESmallENV15Q => FastEmbedModel::BGESmallENV15Q,
            Self::BGEBaseENV15 => FastEmbedModel::BGEBaseENV15,
            Self::BGEBaseENV15Q => FastEmbedModel::BGEBaseENV15Q,
            Self::BGELargeENV15 => FastEmbedModel::BGELargeENV15,
            Self::BGELargeENV15Q => FastEmbedModel::BGELargeENV15Q,
            Self::BGESmallZHV15 => FastEmbedModel::BGESmallZHV15,
            Self::BGELargeZHV15 => FastEmbedModel::BGELargeZHV15,
            Self::BGEM3 => FastEmbedModel::BGEM3,
            Self::ModernBertEmbedLarge => FastEmbedModel::ModernBertEmbedLarge,
            Self::NomicEmbedTextV1 => FastEmbedModel::NomicEmbedTextV1,
            Self::NomicEmbedTextV15 => FastEmbedModel::NomicEmbedTextV15,
            Self::NomicEmbedTextV15Q => FastEmbedModel::NomicEmbedTextV15Q,
            Self::ParaphraseMLMiniLML12V2 => FastEmbedModel::ParaphraseMLMiniLML12V2,
            Self::ParaphraseMLMiniLML12V2Q => FastEmbedModel::ParaphraseMLMiniLML12V2Q,
            Self::ParaphraseMLMpnetBaseV2 => FastEmbedModel::ParaphraseMLMpnetBaseV2,
            Self::MxbaiEmbedLargeV1 => FastEmbedModel::MxbaiEmbedLargeV1,
            Self::MxbaiEmbedLargeV1Q => FastEmbedModel::MxbaiEmbedLargeV1Q,
            Self::GTEBaseENV15 => FastEmbedModel::GTEBaseENV15,
            Self::GTEBaseENV15Q => FastEmbedModel::GTEBaseENV15Q,
            Self::GTELargeENV15 => FastEmbedModel::GTELargeENV15,
            Self::GTELargeENV15Q => FastEmbedModel::GTELargeENV15Q,
            Self::JinaEmbeddingsV2BaseCode => FastEmbedModel::JinaEmbeddingsV2BaseCode,
            Self::JinaEmbeddingsV2BaseEN => FastEmbedModel::JinaEmbeddingsV2BaseEN,
            Self::EmbeddingGemma300M => FastEmbedModel::EmbeddingGemma300M,
            // Arctic backward-compat aliases → Snowflake fastembed variants
            Self::ArcticEmbedXS => FastEmbedModel::SnowflakeArcticEmbedXS,
            Self::ArcticEmbedXSQ => FastEmbedModel::SnowflakeArcticEmbedXSQ,
            Self::ArcticEmbedS => FastEmbedModel::SnowflakeArcticEmbedS,
            Self::ArcticEmbedSQ => FastEmbedModel::SnowflakeArcticEmbedSQ,
            Self::ArcticEmbedM => FastEmbedModel::SnowflakeArcticEmbedM,
            Self::ArcticEmbedMQ => FastEmbedModel::SnowflakeArcticEmbedMQ,
            Self::ArcticEmbedMLong => FastEmbedModel::SnowflakeArcticEmbedMLong,
            Self::ArcticEmbedMLongQ => FastEmbedModel::SnowflakeArcticEmbedMLongQ,
            Self::ArcticEmbedL => FastEmbedModel::SnowflakeArcticEmbedL,
            Self::ArcticEmbedLQ => FastEmbedModel::SnowflakeArcticEmbedLQ,
        }
    }

    pub fn query_prefix(self) -> Option<&'static str> {
        match self {
            Self::MultilingualE5Small | Self::MultilingualE5Base | Self::MultilingualE5Large => {
                Some("query: ")
            }

            Self::NomicEmbedTextV15 | Self::NomicEmbedTextV15Q => Some("search_query: "),

            Self::ArcticEmbedXS
            | Self::ArcticEmbedXSQ
            | Self::ArcticEmbedS
            | Self::ArcticEmbedSQ
            | Self::ArcticEmbedM
            | Self::ArcticEmbedMQ
            | Self::ArcticEmbedMLong
            | Self::ArcticEmbedMLongQ
            | Self::ArcticEmbedL
            | Self::ArcticEmbedLQ => {
                Some("Represent this sentence for searching relevant passages: ")
            }

            _ => None,
        }
    }

    pub fn doc_prefix(self) -> Option<&'static str> {
        match self {
            Self::MultilingualE5Small | Self::MultilingualE5Base | Self::MultilingualE5Large => {
                Some("passage: ")
            }

            Self::NomicEmbedTextV15 | Self::NomicEmbedTextV15Q => Some("search_document: "),

            _ => None,
        }
    }

    /// Maximum sequence length (tokens) supported by the model.
    pub fn max_seq_length(self) -> usize {
        match self {
            Self::AllMiniLML6V2
            | Self::AllMiniLML6V2Q
            | Self::AllMiniLML12V2
            | Self::AllMiniLML12V2Q => 256,

            Self::AllMpnetBaseV2 => 384,

            // Arctic Embed non-Long variants: 512 token limit
            Self::ArcticEmbedXS
            | Self::ArcticEmbedXSQ
            | Self::ArcticEmbedS
            | Self::ArcticEmbedSQ
            | Self::ArcticEmbedM
            | Self::ArcticEmbedMQ
            | Self::ArcticEmbedL
            | Self::ArcticEmbedLQ => 512,

            // Long-context models
            Self::BGEM3
            | Self::NomicEmbedTextV1
            | Self::NomicEmbedTextV15
            | Self::NomicEmbedTextV15Q
            | Self::JinaEmbeddingsV2BaseCode
            | Self::JinaEmbeddingsV2BaseEN
            | Self::EmbeddingGemma300M
            | Self::ArcticEmbedMLong
            | Self::ArcticEmbedMLongQ => 8192,

            // All others default to 512
            _ => 512,
        }
    }

    /// Approximate model size in MB (ONNX format estimates).
    pub fn size_mb(&self) -> usize {
        match self {
            Self::AllMiniLML6V2 | Self::AllMiniLML6V2Q => 80,
            Self::AllMiniLML12V2 | Self::AllMiniLML12V2Q => 120,
            Self::AllMpnetBaseV2 => 420,
            Self::MultilingualE5Small => 470,
            Self::MultilingualE5Base => 1100,
            Self::MultilingualE5Large => 2200,
            Self::BGESmallENV15 => 130,
            Self::BGESmallENV15Q => 40,
            Self::BGEBaseENV15 => 430,
            Self::BGEBaseENV15Q => 130,
            Self::BGELargeENV15 => 1300,
            Self::BGELargeENV15Q => 400,
            Self::BGESmallZHV15 => 100,
            Self::BGELargeZHV15 => 1300,
            Self::BGEM3 => 600,
            Self::ModernBertEmbedLarge => 600,
            Self::NomicEmbedTextV1 => 550,
            Self::NomicEmbedTextV15 | Self::NomicEmbedTextV15Q => 550,
            Self::ParaphraseMLMiniLML12V2 => 420,
            Self::ParaphraseMLMiniLML12V2Q => 130,
            Self::ParaphraseMLMpnetBaseV2 => 1100,
            Self::MxbaiEmbedLargeV1 => 670,
            Self::MxbaiEmbedLargeV1Q => 200,
            Self::GTEBaseENV15 => 430,
            Self::GTEBaseENV15Q => 130,
            Self::GTELargeENV15 => 1300,
            Self::GTELargeENV15Q => 400,
            Self::JinaEmbeddingsV2BaseCode => 550,
            Self::JinaEmbeddingsV2BaseEN => 550,
            Self::EmbeddingGemma300M => 600,
            Self::ArcticEmbedXS | Self::ArcticEmbedXSQ => 90,
            Self::ArcticEmbedS | Self::ArcticEmbedSQ => 130,
            Self::ArcticEmbedM
            | Self::ArcticEmbedMQ
            | Self::ArcticEmbedMLong
            | Self::ArcticEmbedMLongQ => 430,
            Self::ArcticEmbedL | Self::ArcticEmbedLQ => 1300,
        }
    }

    /// HuggingFace model repo ID for cache management.
    pub fn model_id(self) -> &'static str {
        match self {
            Self::AllMiniLML6V2 => "Qdrant/all-MiniLM-L6-v2-onnx",
            Self::AllMiniLML6V2Q => "Xenova/all-MiniLM-L6-v2",
            Self::AllMiniLML12V2 => "Xenova/all-MiniLM-L12-v2",
            Self::AllMiniLML12V2Q => "Qdrant/all-MiniLM-L12-v2-onnx-Q",
            Self::AllMpnetBaseV2 => "Xenova/all-mpnet-base-v2",
            Self::MultilingualE5Small => "intfloat/multilingual-e5-small",
            Self::MultilingualE5Base => "intfloat/multilingual-e5-base",
            Self::MultilingualE5Large => "Qdrant/multilingual-e5-large-onnx",
            Self::BGESmallENV15 => "Xenova/bge-small-en-v1.5",
            Self::BGESmallENV15Q => "Qdrant/bge-small-en-v1.5-onnx-Q",
            Self::BGEBaseENV15 => "Xenova/bge-base-en-v1.5",
            Self::BGEBaseENV15Q => "Qdrant/bge-base-en-v1.5-onnx-Q",
            Self::BGELargeENV15 => "Xenova/bge-large-en-v1.5",
            Self::BGELargeENV15Q => "Qdrant/bge-large-en-v1.5-onnx-Q",
            Self::BGESmallZHV15 => "Xenova/bge-small-zh-v1.5",
            Self::BGELargeZHV15 => "Xenova/bge-large-zh-v1.5",
            Self::BGEM3 => "BAAI/bge-m3",
            Self::ModernBertEmbedLarge => "lightonai/modernbert-embed-large",
            Self::NomicEmbedTextV1 => "nomic-ai/nomic-embed-text-v1",
            Self::NomicEmbedTextV15 => "nomic-ai/nomic-embed-text-v1.5",
            Self::NomicEmbedTextV15Q => "nomic-ai/nomic-embed-text-v1.5",
            Self::ParaphraseMLMiniLML12V2 => "Xenova/paraphrase-multilingual-MiniLM-L12-v2",
            Self::ParaphraseMLMiniLML12V2Q => "Qdrant/paraphrase-multilingual-MiniLM-L12-v2-onnx-Q",
            Self::ParaphraseMLMpnetBaseV2 => "Xenova/paraphrase-multilingual-mpnet-base-v2",
            Self::MxbaiEmbedLargeV1 => "mixedbread-ai/mxbai-embed-large-v1",
            Self::MxbaiEmbedLargeV1Q => "Qdrant/mxbai-embed-large-v1-onnx-Q",
            Self::GTEBaseENV15 => "Alibaba-NLP/gte-base-en-v1.5",
            Self::GTEBaseENV15Q => "Qdrant/gte-base-en-v1.5-onnx-Q",
            Self::GTELargeENV15 => "Alibaba-NLP/gte-large-en-v1.5",
            Self::GTELargeENV15Q => "Qdrant/gte-large-en-v1.5-onnx-Q",
            Self::JinaEmbeddingsV2BaseCode => "jinaai/jina-embeddings-v2-base-code",
            Self::JinaEmbeddingsV2BaseEN => "jinaai/jina-embeddings-v2-base-en",
            Self::EmbeddingGemma300M => "onnx-community/embeddinggemma-300m-ONNX",
            Self::ArcticEmbedXS | Self::ArcticEmbedXSQ => "snowflake/snowflake-arctic-embed-xs",
            Self::ArcticEmbedS | Self::ArcticEmbedSQ => "snowflake/snowflake-arctic-embed-s",
            Self::ArcticEmbedM | Self::ArcticEmbedMQ => "Snowflake/snowflake-arctic-embed-m",
            Self::ArcticEmbedMLong | Self::ArcticEmbedMLongQ => {
                "snowflake/snowflake-arctic-embed-m-long"
            }
            Self::ArcticEmbedL | Self::ArcticEmbedLQ => "snowflake/snowflake-arctic-embed-l",
        }
    }
}

impl std::fmt::Display for EmbeddingModelChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", (*self).as_str())
    }
}

// ---------------------------------------------------------------------------
// LRU cache backed by IndexMap
// ---------------------------------------------------------------------------

pub struct QueryEmbedCache {
    capacity: usize,
    map: indexmap::IndexMap<String, Vec<f32>>,
}

impl QueryEmbedCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            map: indexmap::IndexMap::new(),
        }
    }

    pub fn get(&mut self, key: &str) -> Option<&Vec<f32>> {
        if self.capacity == 0 {
            return None;
        }
        if let Some((k, v)) = self.map.shift_remove_entry(key) {
            self.map.insert(k, v);
            self.map.get(key)
        } else {
            None
        }
    }

    pub fn insert(&mut self, key: String, value: Vec<f32>) {
        if self.capacity == 0 {
            return;
        }
        if let Some((k, _)) = self.map.shift_remove_entry(&key) {
            self.map.insert(k, value);
        } else {
            if self.map.len() >= self.capacity {
                self.map.shift_remove_index(0);
            }
            self.map.insert(key, value);
        }
    }
}

// ---------------------------------------------------------------------------
// FastEmbed session (ONNX Runtime inference engine)
// ---------------------------------------------------------------------------

/// Holds a loaded fastembed TextEmbedding, ready for inference.
#[cfg(feature = "embed")]
struct FastEmbedSession {
    model: TextEmbedding,
}

#[cfg(feature = "embed")]
impl FastEmbedSession {
    /// Download model (if needed) via fastembed's HuggingFace Hub integration
    /// and build an ONNX Runtime session.
    fn load(choice: &EmbeddingModelChoice, cache_dir: &Path) -> Result<Self> {
        #[allow(unused_mut)]
        let mut opts = TextInitOptions::new(choice.to_fastembed())
            .with_cache_dir(cache_dir.to_path_buf())
            .with_show_download_progress(true);

        // DirectML GPU acceleration on Windows
        #[cfg(all(feature = "embed-directml", target_os = "windows"))]
        {
            use fastembed::ExecutionProviderDispatch;
            opts = opts.with_execution_providers(vec![ExecutionProviderDispatch::from(
                ort::execution_providers::DirectMLExecutionProvider::default(),
            )]);
        }

        let model =
            TextEmbedding::try_new(opts).context("Failed to load embedding model via fastembed")?;
        Ok(Self { model })
    }

    /// Embed a batch of texts, returning one normalized Vec<f32> per text.
    fn embed_batch(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let embeddings = self.model.embed(texts, None)?;
        // L2 normalize — fastembed returns raw pooled output
        Ok(embeddings
            .into_iter()
            .map(|mut e| {
                let norm: f64 = e.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
                if norm > 0.0 {
                    for v in e.iter_mut() {
                        *v /= norm as f32;
                    }
                }
                e
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// EmbeddingService — public API
// ---------------------------------------------------------------------------

#[cfg(feature = "embed")]
pub struct EmbeddingService {
    state: Arc<Mutex<ModelState>>,
    download_cvar: Arc<Condvar>,
    internal_config: InternalEmbeddingConfig,
    session: Arc<Mutex<Option<FastEmbedSession>>>,
    #[allow(dead_code)]
    previous_dimension: Arc<Mutex<Option<usize>>>,
    last_embed_at: Arc<Mutex<Instant>>,
    query_cache: Arc<Mutex<QueryEmbedCache>>,
}

#[cfg(feature = "embed")]
struct InternalEmbeddingConfig {
    model: EmbeddingModelChoice,
    cache_dir: PathBuf,
    enabled: bool,
}

#[cfg(feature = "embed")]
impl EmbeddingService {
    pub fn new(config: crate::config::EmbeddingConfig) -> Self {
        let model_choice = EmbeddingModelChoice::parse(&config.model).unwrap_or_else(|| {
            eprintln!("Warning: Unknown model '{}', using default", config.model);
            EmbeddingModelChoice::default()
        });

        let enabled = config.enabled;
        let internal_config = InternalEmbeddingConfig {
            model: model_choice,
            cache_dir: PathBuf::from(&config.cache_dir),
            enabled,
        };

        let initial_state = if enabled {
            ModelState::NotLoaded
        } else {
            ModelState::Disabled
        };

        Self {
            state: Arc::new(Mutex::new(initial_state)),
            download_cvar: Arc::new(Condvar::new()),
            internal_config,
            session: Arc::new(Mutex::new(None)),
            previous_dimension: Arc::new(Mutex::new(None)),
            last_embed_at: Arc::new(Mutex::new(Instant::now())),
            query_cache: Arc::new(Mutex::new(QueryEmbedCache::new(config.query_cache_size))),
        }
    }

    pub fn state(&self) -> ModelState {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn dimension(&self) -> usize {
        self.internal_config.model.dimension()
    }

    #[allow(dead_code)]
    pub fn dimension_changed(&self) -> bool {
        let current = self.internal_config.model.dimension();
        let mut prev = self
            .previous_dimension
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(p) = *prev {
            p != current
        } else {
            *prev = Some(current);
            false
        }
    }

    pub fn ensure_model(&self) -> Result<(), String> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        match state {
            ModelState::Ready => return Ok(()),
            ModelState::Disabled => {
                return Err("Embedding is disabled in configuration".to_string());
            }
            ModelState::Failed(e) => return Err(e),
            ModelState::NotLoaded | ModelState::Loading => {
                // In test builds, skip model download entirely — the ONNX Runtime
                // native library increases per-process resource pressure enough to
                // cause EAGAIN on Tantivy commits when many tests run in parallel.
                #[cfg(test)]
                {
                    *self.state.lock().unwrap_or_else(|e| e.into_inner()) =
                        ModelState::Failed("Model download skipped in test build".to_string());
                    self.download_cvar.notify_all();
                    return Err("Model download skipped in test build".to_string());
                }
                #[cfg(not(test))]
                self.start_download();
            }
        }

        let deadline = Instant::now() + Duration::from_secs(300);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match state.clone() {
                ModelState::Ready => return Ok(()),
                ModelState::Failed(e) => return Err(e),
                ModelState::NotLoaded | ModelState::Loading => {
                    drop(state);
                    self.start_download();
                    state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                _ => {
                    let timeout = deadline.saturating_duration_since(Instant::now());
                    if timeout.is_zero() {
                        return Err("Timed out waiting for model download to complete".to_string());
                    }
                    let (new_state, timed_out) = self
                        .download_cvar
                        .wait_timeout(state, timeout)
                        .unwrap_or_else(|e| e.into_inner());
                    state = new_state;
                    if timed_out.timed_out() {
                        return Err("Timed out waiting for model download to complete".to_string());
                    }
                }
            }
        }
    }

    fn start_download(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if *state != ModelState::NotLoaded {
                return;
            }
            // Transition to Loading inside the lock so a concurrent caller sees
            // a non-NotLoaded state and returns early — prevents double-spawn.
            *state = ModelState::Loading;
        }

        let state_arc = Arc::clone(&self.state);
        let cvar_arc = Arc::clone(&self.download_cvar);
        let session_arc = Arc::clone(&self.session);
        let cache_dir = self.internal_config.cache_dir.clone();
        let model = self.internal_config.model;

        let result = std::thread::Builder::new()
            .name("alcove-model-download".into())
            .spawn({
                let state_arc = state_arc.clone();
                let cvar_arc = cvar_arc.clone();
                move || {
                    macro_rules! set_state_and_notify {
                        ($s:expr) => {{
                            *state_arc.lock().unwrap_or_else(|e| e.into_inner()) = $s;
                            cvar_arc.notify_all();
                        }};
                    }

                    if let Err(e) = std::fs::create_dir_all(&cache_dir) {
                        set_state_and_notify!(ModelState::Failed(e.to_string()));
                        return;
                    }

                    match FastEmbedSession::load(&model, &cache_dir) {
                        Ok(session) => {
                            let mut state_guard =
                                state_arc.lock().unwrap_or_else(|e| e.into_inner());
                            *session_arc.lock().unwrap_or_else(|e| e.into_inner()) = Some(session);
                            *state_guard = ModelState::Ready;
                            drop(state_guard);
                            cvar_arc.notify_all();
                        }
                        Err(e) => {
                            set_state_and_notify!(ModelState::Failed(e.to_string()));
                        }
                    }
                }
            });

        if let Err(e) = result {
            *state_arc.lock().unwrap_or_else(|e| e.into_inner()) =
                ModelState::Failed(format!("Failed to spawn download thread: {}", e));
            cvar_arc.notify_all();
        }
    }

    fn try_unload_if_idle(&self) -> bool {
        let unload_secs = crate::config::load_config()
            .memory_config_with_defaults()
            .model_unload_secs;
        if unload_secs == 0 {
            return false;
        }
        let elapsed = self
            .last_embed_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .elapsed();
        if elapsed <= Duration::from_secs(unload_secs) {
            return false;
        }

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *state != ModelState::Ready {
            return false;
        }
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        *session = None;
        *state = ModelState::NotLoaded;
        true
    }

    pub fn embed(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, String> {
        if self.try_unload_if_idle() {
            return Err(
                "Model unloaded after idle timeout; will reload on next request".to_string(),
            );
        }

        let state = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if state != ModelState::Ready {
            return Err(format!("Model not ready: {}", state));
        }

        let mut results: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
        let mut miss_indices: Vec<usize> = Vec::new();
        {
            let mut cache = self.query_cache.lock().unwrap_or_else(|e| e.into_inner());
            for (i, text) in texts.iter().enumerate() {
                if let Some(cached) = cache.get(text) {
                    results[i] = Some(cached.clone());
                } else {
                    miss_indices.push(i);
                }
            }
        }

        if miss_indices.is_empty() {
            return Ok(results.into_iter().flatten().collect());
        }

        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let session = session
            .as_mut()
            .ok_or_else(|| "Session not loaded".to_string())?;

        let miss_texts: Vec<String> = miss_indices.iter().map(|&i| texts[i].to_string()).collect();
        let inferred = session
            .embed_batch(&miss_texts)
            .map_err(|e| e.to_string())?;

        {
            let mut cache = self.query_cache.lock().unwrap_or_else(|e| e.into_inner());
            for (miss_text, vec) in miss_texts.iter().zip(inferred.iter()) {
                cache.insert(miss_text.clone(), vec.clone());
            }
        }
        for (slot, vec) in miss_indices.iter().zip(inferred) {
            results[*slot] = Some(vec);
        }

        *self.last_embed_at.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();

        Ok(results.into_iter().flatten().collect())
    }

    #[allow(dead_code)]
    pub fn remove_cache(&self) -> std::result::Result<(), String> {
        // hf-hub cache uses "models--{org}--{repo}" folder naming.
        // Remove only this model's folder so other cached models are unaffected.
        let model_id = self.internal_config.model.model_id();
        let folder_name = format!("models--{}", model_id.replace('/', "--"));
        let model_dir = self.internal_config.cache_dir.join(folder_name);
        if model_dir.exists() {
            std::fs::remove_dir_all(&model_dir)
                .map_err(|e| format!("Failed to remove cache: {}", e))?;
        }

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        *session = None;
        *state = ModelState::NotLoaded;

        Ok(())
    }

    #[allow(dead_code)]
    pub fn is_enabled(&self) -> bool {
        self.internal_config.enabled
    }

    pub fn model_name(&self) -> &'static str {
        self.internal_config.model.as_str()
    }

    pub fn model_choice(&self) -> EmbeddingModelChoice {
        self.internal_config.model
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[cfg(feature = "embed")]
    use super::*;

    #[test]
    fn test_model_choice_dimension() {
        #[cfg(feature = "embed")]
        {
            assert_eq!(EmbeddingModelChoice::AllMiniLML6V2.dimension(), 384);
            assert_eq!(EmbeddingModelChoice::MultilingualE5Small.dimension(), 384);
            assert_eq!(EmbeddingModelChoice::MultilingualE5Base.dimension(), 768);
            assert_eq!(EmbeddingModelChoice::MultilingualE5Large.dimension(), 1024);
            assert_eq!(EmbeddingModelChoice::BGEM3.dimension(), 1024);
            assert_eq!(EmbeddingModelChoice::ArcticEmbedXS.dimension(), 384);
            assert_eq!(EmbeddingModelChoice::ArcticEmbedS.dimension(), 384);
            assert_eq!(EmbeddingModelChoice::ArcticEmbedM.dimension(), 768);
            assert_eq!(EmbeddingModelChoice::ArcticEmbedL.dimension(), 1024);
            assert_eq!(EmbeddingModelChoice::BGESmallZHV15.dimension(), 512);
            assert_eq!(EmbeddingModelChoice::ArcticEmbedMLong.dimension(), 768);
        }
    }

    #[test]
    fn test_model_max_seq_length() {
        #[cfg(feature = "embed")]
        {
            assert_eq!(EmbeddingModelChoice::AllMiniLML6V2.max_seq_length(), 256);
            assert_eq!(EmbeddingModelChoice::AllMpnetBaseV2.max_seq_length(), 384);
            // Arctic non-Long: 512
            assert_eq!(EmbeddingModelChoice::ArcticEmbedXS.max_seq_length(), 512);
            assert_eq!(EmbeddingModelChoice::ArcticEmbedS.max_seq_length(), 512);
            assert_eq!(EmbeddingModelChoice::ArcticEmbedM.max_seq_length(), 512);
            assert_eq!(EmbeddingModelChoice::ArcticEmbedL.max_seq_length(), 512);
            // Arctic Long: 8192
            assert_eq!(EmbeddingModelChoice::ArcticEmbedMLong.max_seq_length(), 8192);
            assert_eq!(EmbeddingModelChoice::BGEM3.max_seq_length(), 8192);
            assert_eq!(EmbeddingModelChoice::NomicEmbedTextV15.max_seq_length(), 8192);
        }
    }

    #[test]
    fn test_model_id_uniqueness() {
        #[cfg(feature = "embed")]
        {
            use std::collections::HashMap;
            let mut id_to_models: HashMap<&str, Vec<&str>> = HashMap::new();
            for m in EmbeddingModelChoice::all() {
                id_to_models.entry(m.model_id()).or_default().push(m.as_str());
            }
            // Verify quantized variants no longer share repos with non-Q counterparts
            assert_ne!(
                EmbeddingModelChoice::AllMiniLML12V2.model_id(),
                EmbeddingModelChoice::AllMiniLML12V2Q.model_id()
            );
            assert_ne!(
                EmbeddingModelChoice::GTEBaseENV15.model_id(),
                EmbeddingModelChoice::GTEBaseENV15Q.model_id()
            );
            assert_ne!(
                EmbeddingModelChoice::GTELargeENV15.model_id(),
                EmbeddingModelChoice::GTELargeENV15Q.model_id()
            );
            assert_ne!(
                EmbeddingModelChoice::MxbaiEmbedLargeV1.model_id(),
                EmbeddingModelChoice::MxbaiEmbedLargeV1Q.model_id()
            );
        }
    }

    #[test]
    fn test_model_prefixes() {
        #[cfg(feature = "embed")]
        {
            assert_eq!(EmbeddingModelChoice::AllMiniLML6V2.query_prefix(), None);
            assert_eq!(EmbeddingModelChoice::AllMiniLML6V2.doc_prefix(), None);

            for m in [
                EmbeddingModelChoice::MultilingualE5Small,
                EmbeddingModelChoice::MultilingualE5Base,
                EmbeddingModelChoice::MultilingualE5Large,
            ] {
                assert_eq!(m.query_prefix(), Some("query: "), "{:?} query prefix", m);
                assert_eq!(m.doc_prefix(), Some("passage: "), "{:?} doc prefix", m);
            }

            assert_eq!(EmbeddingModelChoice::BGEM3.query_prefix(), None);
            assert_eq!(EmbeddingModelChoice::BGEM3.doc_prefix(), None);

            assert_eq!(
                EmbeddingModelChoice::NomicEmbedTextV15.query_prefix(),
                Some("search_query: ")
            );
            assert_eq!(
                EmbeddingModelChoice::NomicEmbedTextV15.doc_prefix(),
                Some("search_document: ")
            );

            let arctic_query_prefix = "Represent this sentence for searching relevant passages: ";
            for m in [
                EmbeddingModelChoice::ArcticEmbedXS,
                EmbeddingModelChoice::ArcticEmbedS,
                EmbeddingModelChoice::ArcticEmbedM,
                EmbeddingModelChoice::ArcticEmbedL,
            ] {
                assert_eq!(
                    m.query_prefix(),
                    Some(arctic_query_prefix),
                    "{:?} query prefix",
                    m
                );
                assert_eq!(m.doc_prefix(), None, "{:?} doc prefix", m);
            }
        }
    }

    #[test]
    fn test_model_choice_parse() {
        #[cfg(feature = "embed")]
        {
            // Backward compat names
            assert_eq!(
                EmbeddingModelChoice::parse("AllMiniLML6V2"),
                Some(EmbeddingModelChoice::AllMiniLML6V2)
            );
            assert_eq!(
                EmbeddingModelChoice::parse("BGEM3"),
                Some(EmbeddingModelChoice::BGEM3)
            );
            assert_eq!(
                EmbeddingModelChoice::parse("ArcticEmbedXS"),
                Some(EmbeddingModelChoice::ArcticEmbedXS)
            );
            assert_eq!(
                EmbeddingModelChoice::parse("ArcticEmbedL"),
                Some(EmbeddingModelChoice::ArcticEmbedL)
            );
            // New models
            assert_eq!(
                EmbeddingModelChoice::parse("BGESmallENV15"),
                Some(EmbeddingModelChoice::BGESmallENV15)
            );
            assert_eq!(
                EmbeddingModelChoice::parse("NomicEmbedTextV15"),
                Some(EmbeddingModelChoice::NomicEmbedTextV15)
            );
            // Unknown
            assert_eq!(EmbeddingModelChoice::parse("InvalidModel"), None);
        }
    }

    #[test]
    fn test_model_state_display() {
        #[cfg(feature = "embed")]
        {
            assert_eq!(format!("{}", ModelState::NotLoaded), "not_loaded");
            assert_eq!(format!("{}", ModelState::Loading), "loading");
            assert_eq!(format!("{}", ModelState::Ready), "ready");
            assert_eq!(
                format!("{}", ModelState::Failed("oops".to_string())),
                "failed: oops"
            );
        }
    }

    #[test]
    fn test_embedding_cache_hit_skips_inference() {
        let mut cache = super::QueryEmbedCache::new(2);
        assert!(cache.get("hello").is_none());
        cache.insert("hello".to_string(), vec![1.0, 2.0]);
        assert_eq!(cache.get("hello"), Some(&vec![1.0, 2.0]));
        cache.insert("world".to_string(), vec![3.0, 4.0]);
        assert_eq!(cache.get("world"), Some(&vec![3.0, 4.0]));
        assert_eq!(cache.get("hello"), Some(&vec![1.0, 2.0]));
        cache.insert("foo".to_string(), vec![5.0, 6.0]);
        assert!(
            cache.get("world").is_none(),
            "world should have been evicted"
        );
        assert!(cache.get("hello").is_some());
        assert!(cache.get("foo").is_some());
    }

    #[test]
    fn test_embedding_cache_lru_order() {
        let mut cache = super::QueryEmbedCache::new(2);
        cache.insert("a".to_string(), vec![1.0]);
        cache.insert("b".to_string(), vec![2.0]);
        let _ = cache.get("a");
        cache.insert("c".to_string(), vec![3.0]);
        assert!(cache.get("b").is_none(), "b should be evicted as LRU");
        assert!(cache.get("a").is_some());
        assert!(cache.get("c").is_some());
    }

    #[test]
    #[cfg(feature = "embed")]
    fn test_mutex_poison_recovery() {
        use std::sync::{Arc, Mutex};
        let mutex: Arc<Mutex<ModelState>> = Arc::new(Mutex::new(ModelState::NotLoaded));
        let mutex_clone = Arc::clone(&mutex);
        let _ = std::panic::catch_unwind(move || {
            let _guard = mutex_clone.lock().unwrap();
            panic!("intentional poison");
        });
        let recovered = mutex.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(recovered, ModelState::NotLoaded);
    }
}
