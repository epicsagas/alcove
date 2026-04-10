//! Embedding service for hybrid search (alcove-full feature)
//!
//! Provides lazy model download, ONNX inference, and vector index management.
//! Graceful degradation ensures BM25-only search works even when models aren't ready.

#[cfg(feature = "alcove-full")]
use std::path::PathBuf;

#[cfg(feature = "alcove-full")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "alcove-full")]
use anyhow::Result;
#[cfg(feature = "alcove-full")]
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

/// Model state for graceful degradation
#[cfg(feature = "alcove-full")]
#[derive(Debug, Clone, PartialEq)]
pub enum ModelState {
    /// Model not downloaded yet
    NotDownloaded,
    /// Background download in progress (percentage)
    Downloading { progress_pct: u8 },
    /// Model cached, waiting to load ONNX session
    Cached,
    /// ONNX session loaded, ready for embedding
    Ready,
    /// Download or load failed
    Failed(String),
}

#[cfg(feature = "alcove-full")]
impl Default for ModelState {
    fn default() -> Self {
        Self::NotDownloaded
    }
}

#[cfg(feature = "alcove-full")]
impl std::fmt::Display for ModelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotDownloaded => write!(f, "not_downloaded"),
            Self::Downloading { progress_pct } => write!(f, "downloading ({}%)", progress_pct),
            Self::Cached => write!(f, "cached"),
            Self::Ready => write!(f, "ready"),
            Self::Failed(e) => write!(f, "failed: {}", e),
        }
    }
}

/// Supported embedding models (Korean + multilingual)
#[cfg_attr(not(feature = "alcove-full"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmbeddingModelChoice {
    #[default]
    MultilingualE5Small,
    MultilingualE5Base,
    MultilingualE5Large,
    SnowflakeArcticEmbedXS,
    SnowflakeArcticEmbedXSQ,
    SnowflakeArcticEmbedS,
    SnowflakeArcticEmbedSQ,
    SnowflakeArcticEmbedM,
    SnowflakeArcticEmbedMQ,
    BGEM3,
}

#[cfg_attr(not(feature = "alcove-full"), allow(dead_code))]
impl EmbeddingModelChoice {
    /// Get the fastembed model enum variant
    #[cfg(feature = "alcove-full")]
    pub fn as_fastembed_model(self) -> EmbeddingModel {
        match self {
            Self::MultilingualE5Small => EmbeddingModel::MultilingualE5Small,
            Self::MultilingualE5Base => EmbeddingModel::MultilingualE5Base,
            Self::MultilingualE5Large => EmbeddingModel::MultilingualE5Large,
            Self::SnowflakeArcticEmbedXS => EmbeddingModel::SnowflakeArcticEmbedXS,
            Self::SnowflakeArcticEmbedXSQ => EmbeddingModel::SnowflakeArcticEmbedXSQ,
            Self::SnowflakeArcticEmbedS => EmbeddingModel::SnowflakeArcticEmbedS,
            Self::SnowflakeArcticEmbedSQ => EmbeddingModel::SnowflakeArcticEmbedSQ,
            Self::SnowflakeArcticEmbedM => EmbeddingModel::SnowflakeArcticEmbedM,
            Self::SnowflakeArcticEmbedMQ => EmbeddingModel::SnowflakeArcticEmbedMQ,
            Self::BGEM3 => EmbeddingModel::BGEM3,
        }
    }

    /// Get embedding dimension for this model
    pub fn dimension(&self) -> usize {
        match self {
            Self::MultilingualE5Small
            | Self::SnowflakeArcticEmbedXS
            | Self::SnowflakeArcticEmbedXSQ
            | Self::SnowflakeArcticEmbedS
            | Self::SnowflakeArcticEmbedSQ => 384,
            Self::MultilingualE5Base
            | Self::SnowflakeArcticEmbedM
            | Self::SnowflakeArcticEmbedMQ => 768,
            Self::MultilingualE5Large | Self::BGEM3 => 1024,
        }
    }

    /// Approximate model size in MB
    pub fn size_mb(&self) -> usize {
        match self {
            Self::SnowflakeArcticEmbedXS => 30,
            Self::SnowflakeArcticEmbedXSQ => 15,
            Self::SnowflakeArcticEmbedS => 130,
            Self::SnowflakeArcticEmbedSQ => 65,
            Self::MultilingualE5Small => 235, // O4 optimized
            Self::SnowflakeArcticEmbedM => 400,
            Self::SnowflakeArcticEmbedMQ => 200,
            Self::MultilingualE5Base => 555, // O4 optimized
            Self::MultilingualE5Large => 2200,
            Self::BGEM3 => 2300,
        }
    }

    /// Parse from string (for config file)
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "MultilingualE5Small" => Some(Self::MultilingualE5Small),
            "MultilingualE5Base" => Some(Self::MultilingualE5Base),
            "MultilingualE5Large" => Some(Self::MultilingualE5Large),
            "SnowflakeArcticEmbedXS" => Some(Self::SnowflakeArcticEmbedXS),
            "SnowflakeArcticEmbedXSQ" => Some(Self::SnowflakeArcticEmbedXSQ),
            "SnowflakeArcticEmbedS" => Some(Self::SnowflakeArcticEmbedS),
            "SnowflakeArcticEmbedSQ" => Some(Self::SnowflakeArcticEmbedSQ),
            "SnowflakeArcticEmbedM" => Some(Self::SnowflakeArcticEmbedM),
            "SnowflakeArcticEmbedMQ" => Some(Self::SnowflakeArcticEmbedMQ),
            "BGEM3" => Some(Self::BGEM3),
            _ => None,
        }
    }

    /// Convert to string (for config file)
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MultilingualE5Small => "MultilingualE5Small",
            Self::MultilingualE5Base => "MultilingualE5Base",
            Self::MultilingualE5Large => "MultilingualE5Large",
            Self::SnowflakeArcticEmbedXS => "SnowflakeArcticEmbedXS",
            Self::SnowflakeArcticEmbedXSQ => "SnowflakeArcticEmbedXSQ",
            Self::SnowflakeArcticEmbedS => "SnowflakeArcticEmbedS",
            Self::SnowflakeArcticEmbedSQ => "SnowflakeArcticEmbedSQ",
            Self::SnowflakeArcticEmbedM => "SnowflakeArcticEmbedM",
            Self::SnowflakeArcticEmbedMQ => "SnowflakeArcticEmbedMQ",
            Self::BGEM3 => "BGEM3",
        }
    }

    /// List all available models
    pub fn all() -> &'static [Self] {
        &[
            Self::SnowflakeArcticEmbedXS,
            Self::SnowflakeArcticEmbedXSQ,
            Self::MultilingualE5Small,
            Self::SnowflakeArcticEmbedS,
            Self::SnowflakeArcticEmbedSQ,
            Self::MultilingualE5Base,
            Self::SnowflakeArcticEmbedM,
            Self::SnowflakeArcticEmbedMQ,
            Self::MultilingualE5Large,
            Self::BGEM3,
        ]
    }

    /// Get the HuggingFace model ID
    pub fn model_id(self) -> &'static str {
        match self {
            Self::MultilingualE5Small => "intfloat/multilingual-e5-small",
            Self::MultilingualE5Base => "intfloat/multilingual-e5-base",
            Self::MultilingualE5Large => "intfloat/multilingual-e5-large",
            Self::SnowflakeArcticEmbedXS => "Snowflake/snowflake-arctic-embed-xs",
            Self::SnowflakeArcticEmbedXSQ => "Snowflake/snowflake-arctic-embed-xs",
            Self::SnowflakeArcticEmbedS => "Snowflake/snowflake-arctic-embed-s",
            Self::SnowflakeArcticEmbedSQ => "Snowflake/snowflake-arctic-embed-s",
            Self::SnowflakeArcticEmbedM => "Snowflake/snowflake-arctic-embed-m",
            Self::SnowflakeArcticEmbedMQ => "Snowflake/snowflake-arctic-embed-m",
            Self::BGEM3 => "BAAI/bge-m3",
        }
    }
}

#[cfg(feature = "alcove-full")]
impl std::fmt::Display for EmbeddingModelChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", (*self).as_str())
    }
}

// Feature-gated implementation
// ---------------------------------------------------------------------------

#[cfg(feature = "alcove-full")]
pub struct EmbeddingService {
    /// Current model state
    state: Arc<Mutex<ModelState>>,
    /// Internal configuration with model choice
    internal_config: InternalEmbeddingConfig,
    /// ONNX session (lazy loaded)
    session: Arc<Mutex<Option<TextEmbedding>>>,
    /// Previous model dimension (for detecting changes)
    #[allow(dead_code)]
    previous_dimension: Arc<Mutex<Option<usize>>>,
}

/// Internal embedding configuration (not part of public API)
#[cfg(feature = "alcove-full")]
struct InternalEmbeddingConfig {
    model: EmbeddingModelChoice,
    cache_dir: PathBuf,
    enabled: bool,
}

#[cfg(feature = "alcove-full")]
impl EmbeddingService {
    /// Create a new embedding service
    pub fn new(config: crate::config::EmbeddingConfig) -> Self {
        // Parse model string to EmbeddingModelChoice
        let model_choice = EmbeddingModelChoice::parse(&config.model)
            .unwrap_or_else(|| {
                eprintln!("Warning: Unknown model '{}', using default", config.model);
                EmbeddingModelChoice::default()
            });

        let internal_config = InternalEmbeddingConfig {
            model: model_choice,
            cache_dir: PathBuf::from(&config.cache_dir),
            enabled: config.enabled,
        };

        let initial_state = if internal_config.enabled && Self::is_model_cached(&internal_config) {
            ModelState::Cached
        } else if internal_config.enabled {
            ModelState::NotDownloaded
        } else {
            ModelState::Failed("Embedding disabled".to_string())
        };

        Self {
            state: Arc::new(Mutex::new(initial_state)),
            internal_config,
            session: Arc::new(Mutex::new(None)),
            previous_dimension: Arc::new(Mutex::new(None)),
        }
    }

    /// Check if model is cached locally
    fn is_model_cached(config: &InternalEmbeddingConfig) -> bool {
        // fastembed uses HuggingFace hub cache: models--{user}--{repo}
        let model_id = config.model.model_id();
        let folder_name = format!("models--{}", model_id.replace('/', "--"));
        config.cache_dir.join(folder_name).exists()
    }

    /// Get current model state
    pub fn state(&self) -> ModelState {
        self.state.lock().unwrap().clone()
    }

    /// Get current dimension
    pub fn dimension(&self) -> usize {
        self.internal_config.model.dimension()
    }

    /// Check if dimension changed since last call
    #[allow(dead_code)]
    pub fn dimension_changed(&self) -> bool {
        let current = self.internal_config.model.dimension();
        let mut prev = self.previous_dimension.lock().unwrap();
        if let Some(p) = *prev {
            p != current
        } else {
            *prev = Some(current);
            false
        }
    }

    /// Ensure model is ready (start download if needed)
    pub fn ensure_model(&self) -> Result<(), String> {
        let state = self.state.lock().unwrap().clone();
        match state {
            ModelState::Ready | ModelState::Downloading { .. } => return Ok(()),
            ModelState::Failed(e) => return Err(e),
            _ => {}
        }
        drop(state);

        self.start_download();
        Ok(())
    }

    /// Start background download if not already running
    fn start_download(&self) {
        let mut state = self.state.lock().unwrap();
        if *state != ModelState::NotDownloaded && *state != ModelState::Cached {
            return;
        }

        // For now, we'll do synchronous download in the calling thread
        // A proper implementation would spawn a background thread
        *state = ModelState::Downloading { progress_pct: 0 };
        drop(state);

        let result = self.download_and_load();

        let mut state = self.state.lock().unwrap();
        match result {
            Ok(_) => {
                *state = ModelState::Ready;
            }
            Err(e) => {
                *state = ModelState::Failed(e.to_string());
            }
        }
    }

    /// Download model and load ONNX session
    fn download_and_load(&self) -> Result<()> {
        // Update progress
        {
            let mut state = self.state.lock().unwrap();
            *state = ModelState::Downloading { progress_pct: 10 };
        }

        let options = TextInitOptions::new(self.internal_config.model.as_fastembed_model())
            .with_cache_dir(self.internal_config.cache_dir.clone())
            .with_show_download_progress(true);

        // Update progress
        {
            let mut state = self.state.lock().unwrap();
            *state = ModelState::Downloading { progress_pct: 50 };
        }

        let embedding = TextEmbedding::try_new(options)?;

        // Update progress
        {
            let mut state = self.state.lock().unwrap();
            *state = ModelState::Downloading { progress_pct: 90 };
        }

        // Store session
        {
            let mut session = self.session.lock().unwrap();
            *session = Some(embedding);
        }

        Ok(())
    }

    /// Generate embeddings for texts
    pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        let state = self.state.lock().unwrap().clone();
        if state != ModelState::Ready {
            return Err(format!("Model not ready: {}", state));
        }

        let mut session = self.session.lock().unwrap();
        let session = session
            .as_mut()
            .ok_or_else(|| "Session not loaded".to_string())?;

        let texts_vec: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        session
            .embed(texts_vec, None)
            .map_err(|e| e.to_string())
    }

    /// Remove cached model
    #[allow(dead_code)]
    pub fn remove_cache(&self) -> Result<(), String> {
        let model_dir = self.internal_config.cache_dir.join(self.internal_config.model.as_str());
        if model_dir.exists() {
            std::fs::remove_dir_all(&model_dir)
                .map_err(|e| format!("Failed to remove cache: {}", e))?;
        }

        // Reset state
        let mut state = self.state.lock().unwrap();
        *state = ModelState::NotDownloaded;

        // Clear session
        let mut session = self.session.lock().unwrap();
        *session = None;

        Ok(())
    }

    /// Check if embedding is enabled
    #[allow(dead_code)]
    pub fn is_enabled(&self) -> bool {
        self.internal_config.enabled
    }

    /// Get model name
    pub fn model_name(&self) -> &'static str {
        self.internal_config.model.as_str()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[cfg(feature = "alcove-full")]
    use super::*;

    #[test]
    fn test_model_choice_dimension() {
        #[cfg(feature = "alcove-full")]
        {
            assert_eq!(EmbeddingModelChoice::MultilingualE5Small.dimension(), 384);
            assert_eq!(EmbeddingModelChoice::MultilingualE5Base.dimension(), 768);
            assert_eq!(EmbeddingModelChoice::MultilingualE5Large.dimension(), 1024);
            assert_eq!(EmbeddingModelChoice::SnowflakeArcticEmbedXS.dimension(), 384);
        }
    }

    #[test]
    fn test_model_choice_parse() {
        #[cfg(feature = "alcove-full")]
        {
            assert_eq!(
                EmbeddingModelChoice::parse("MultilingualE5Small"),
                Some(EmbeddingModelChoice::MultilingualE5Small)
            );
            assert_eq!(
                EmbeddingModelChoice::parse("InvalidModel"),
                None
            );
        }
    }

    #[test]
    fn test_model_state_display() {
        #[cfg(feature = "alcove-full")]
        {
            assert_eq!(format!("{}", ModelState::NotDownloaded), "not_downloaded");
            assert_eq!(format!("{}", ModelState::Downloading { progress_pct: 42 }), "downloading (42%)");
            assert_eq!(format!("{}", ModelState::Ready), "ready");
        }
    }

    #[test]
    fn test_model_size() {
        #[cfg(feature = "alcove-full")]
        {
            assert_eq!(EmbeddingModelChoice::SnowflakeArcticEmbedXSQ.size_mb(), 15);
            assert_eq!(EmbeddingModelChoice::MultilingualE5Small.size_mb(), 235);
            assert_eq!(EmbeddingModelChoice::BGEM3.size_mb(), 2300);
        }
    }
}
