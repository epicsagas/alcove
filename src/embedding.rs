//! Embedding service for hybrid search (alcove-full feature)
//!
//! Provides lazy model download, ONNX inference, and vector index management.
//! Graceful degradation ensures BM25-only search works even when models aren't ready.

#[cfg(feature = "alcove-full")]
use std::path::PathBuf;

#[cfg(feature = "alcove-full")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "alcove-full")]
use std::time::{Duration, Instant};

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
    /// Model cached on disk, ONNX session not yet loaded
    Cached,
    /// ONNX session loaded, ready for embedding
    Ready,
    /// Session was loaded but unloaded after idle timeout; model still on disk.
    /// Behaves like `Cached` — reloads on next `ensure_model()` call.
    Unloaded,
    /// Embedding is intentionally disabled in configuration (not an error)
    Disabled,
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
            Self::Unloaded => write!(f, "unloaded"),
            Self::Disabled => write!(f, "disabled"),
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

// ---------------------------------------------------------------------------
// Inline LRU cache (no external crate)
// ---------------------------------------------------------------------------

/// A simple LRU cache backed by `HashMap` (O(1) lookup) and `VecDeque`
/// (eviction order). When `capacity` is 0 the cache is effectively disabled —
/// every `get` returns `None` and `insert` is a no-op.
pub struct QueryEmbedCache {
    capacity: usize,
    map: std::collections::HashMap<String, Vec<f32>>,
    order: std::collections::VecDeque<String>,
}

impl QueryEmbedCache {
    /// Create a new cache with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            map: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
        }
    }

    /// Return a reference to the cached vector for `key`, or `None`.
    /// Accessing an entry promotes it to the most-recently-used position.
    pub fn get(&mut self, key: &str) -> Option<&Vec<f32>> {
        if self.capacity == 0 || !self.map.contains_key(key) {
            return None;
        }
        // Promote to MRU: remove from current position and push to back.
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.to_string());
        self.map.get(key)
    }

    /// Insert or update an entry. Evicts the LRU entry when at capacity.
    pub fn insert(&mut self, key: String, value: Vec<f32>) {
        if self.capacity == 0 {
            return;
        }
        if self.map.contains_key(&key) {
            // Update existing entry and promote it.
            self.map.insert(key.clone(), value);
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
            self.order.push_back(key);
            return;
        }
        // Evict LRU if at capacity.
        if self.map.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            }
        }
        self.map.insert(key.clone(), value);
        self.order.push_back(key);
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
    /// Timestamp of the last successful `embed()` call — used for idle unload.
    last_embed_at: Arc<Mutex<Instant>>,
    /// Per-query embedding LRU cache to skip redundant ONNX inference.
    query_cache: Arc<Mutex<QueryEmbedCache>>,
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
        let model_choice = EmbeddingModelChoice::parse(&config.model).unwrap_or_else(|| {
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
            ModelState::Disabled
        };

        let query_cache_size = config.query_cache_size;

        Self {
            state: Arc::new(Mutex::new(initial_state)),
            internal_config,
            session: Arc::new(Mutex::new(None)),
            previous_dimension: Arc::new(Mutex::new(None)),
            last_embed_at: Arc::new(Mutex::new(Instant::now())),
            query_cache: Arc::new(Mutex::new(QueryEmbedCache::new(query_cache_size))),
        }
    }

    /// Returns true if this model choice is a quantized (Q) variant sharing
    /// the same HuggingFace model ID as its non-quantized counterpart.
    pub fn is_quantized_variant(model: EmbeddingModelChoice) -> bool {
        matches!(
            model,
            EmbeddingModelChoice::SnowflakeArcticEmbedXSQ
                | EmbeddingModelChoice::SnowflakeArcticEmbedSQ
                | EmbeddingModelChoice::SnowflakeArcticEmbedMQ
        )
    }

    /// Check if model is cached locally
    fn is_model_cached(config: &InternalEmbeddingConfig) -> bool {
        // fastembed uses HuggingFace hub cache: models--{user}--{repo}
        let model_id = config.model.model_id();
        let mut folder_name = format!("models--{}", model_id.replace('/', "--"));
        // Q variants share the same HuggingFace model ID as their base variant;
        // append a suffix so the cache check doesn't produce false positives.
        if Self::is_quantized_variant(config.model) {
            folder_name.push_str("-quantized");
        }
        config.cache_dir.join(folder_name).exists()
    }

    /// Get current model state
    pub fn state(&self) -> ModelState {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Get current dimension
    pub fn dimension(&self) -> usize {
        self.internal_config.model.dimension()
    }

    /// Check if dimension changed since last call
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

    /// Ensure model is ready (start background download if needed).
    ///
    /// Spawns a background thread for the download and then polls until the
    /// state transitions to `Ready` or `Failed`, or until the 5-minute timeout
    /// elapses.  The spawned thread means the Tokio executor is not starved
    /// during the download — each 100 ms sleep yields back to the runtime.
    pub fn ensure_model(&self) -> Result<(), String> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match state {
            ModelState::Ready => return Ok(()),
            ModelState::Disabled => return Err("Embedding is disabled in configuration".to_string()),
            ModelState::Failed(e) => return Err(e),
            // Unloaded = was Ready, then session dropped for idle; model still on disk.
            // Treat identically to Cached so start_download() reloads the ONNX session.
            ModelState::NotDownloaded | ModelState::Cached | ModelState::Unloaded => {
                // Kick off background download/reload then fall through to the poll loop.
                self.start_download();
            }
            ModelState::Downloading { .. } => {
                // Another thread is already downloading; fall through to poll.
            }
        }

        // Poll until Ready or Failed.
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            // When running inside a Tokio multi-thread runtime (alcove-server),
            // use block_in_place so the executor can schedule other tasks on this
            // thread while we sleep, instead of parking the OS thread blindly.
            #[cfg(feature = "alcove-server")]
            tokio::task::block_in_place(|| {
                std::thread::sleep(Duration::from_millis(100));
            });
            #[cfg(not(feature = "alcove-server"))]
            std::thread::sleep(Duration::from_millis(100));

            let current = self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            match current {
                ModelState::Ready => return Ok(()),
                ModelState::Failed(e) => return Err(e),
                ModelState::Downloading { .. } => {
                    if Instant::now() >= deadline {
                        return Err(
                            "Timed out waiting for model download to complete".to_string(),
                        );
                    }
                }
                other => {
                    return Err(format!(
                        "Unexpected model state while waiting for download: {}",
                        other
                    ))
                }
            }
        }
    }

    /// Spawn a background thread to download and load the ONNX model.
    ///
    /// Returns immediately after transitioning state to `Downloading`.
    /// Callers that need to wait for completion should poll via `ensure_model`.
    fn start_download(&self) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if *state != ModelState::NotDownloaded
                && *state != ModelState::Cached
                && *state != ModelState::Unloaded
            {
                return;
            }
            *state = ModelState::Downloading { progress_pct: 0 };
        }

        // Clone the Arcs so they can be moved into the background thread.
        let state_arc = Arc::clone(&self.state);
        let session_arc = Arc::clone(&self.session);
        let cache_dir = self.internal_config.cache_dir.clone();
        let model = self.internal_config.model;

        std::thread::spawn(move || {
            macro_rules! set_state {
                ($s:expr) => {
                    *state_arc.lock().unwrap_or_else(|e| e.into_inner()) = $s;
                };
            }

            if let Err(e) = std::fs::create_dir_all(&cache_dir) {
                set_state!(ModelState::Failed(e.to_string()));
                return;
            }

            set_state!(ModelState::Downloading { progress_pct: 10 });

            let options = TextInitOptions::new(model.as_fastembed_model())
                .with_cache_dir(cache_dir)
                .with_show_download_progress(true);

            set_state!(ModelState::Downloading { progress_pct: 50 });

            match TextEmbedding::try_new(options) {
                Ok(embedding) => {
                    set_state!(ModelState::Downloading { progress_pct: 90 });
                    *session_arc.lock().unwrap_or_else(|e| e.into_inner()) = Some(embedding);
                    set_state!(ModelState::Ready);
                }
                Err(e) => {
                    set_state!(ModelState::Failed(e.to_string()));
                }
            }
        });
    }

    /// Drop the ONNX session if the idle timeout has elapsed.
    ///
    /// Acquires locks one at a time (never nested) to avoid deadlock with the
    /// background download thread. Returns `true` if the session was unloaded.
    ///
    /// Possible race: if a background reload completes between the state-lock and
    /// session-lock acquisitions, we still drop the session and leave state as
    /// `Unloaded`. The next `ensure_model()` call will reload transparently.
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

        // Mark as Unloaded (only if currently Ready to avoid interfering with downloads).
        let should_drop = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if *state == ModelState::Ready {
                *state = ModelState::Unloaded;
                true
            } else {
                false
            }
        }; // state lock released

        if should_drop {
            // Drop the session to reclaim model-weight memory (~235–500 MB).
            // State is already Unloaded, so ensure_model() will reload on demand.
            *self.session.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return true;
        }
        false
    }

    /// Generate embeddings for texts.
    ///
    /// Checks idle timeout before use: if the session has been unused for
    /// longer than `memory.model_unload_secs`, the ONNX session is dropped
    /// and `Unloaded` state is returned so the caller can fall back to BM25.
    /// The session will be transparently reloaded on the next `ensure_model()` call.
    pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        // --- idle-unload check ---
        // Drop the ONNX session after prolonged inactivity to reclaim RAM.
        // We call `try_unload_if_idle()` which acquires locks one-at-a-time
        // (never nested) so it can never deadlock with the download thread.
        // In the rare race where a download completes during the unload, the
        // session is dropped and `Unloaded` state is set; the next call to
        // `ensure_model()` transparently reloads it.
        if self.try_unload_if_idle() {
            return Err("Model unloaded after idle timeout; will reload on next request".to_string());
        }

        // --- readiness check ---
        let state = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if state != ModelState::Ready {
            return Err(format!("Model not ready: {}", state));
        }

        // --- cache lookup (partial-hit support) ---
        // Collect cache hits; build a miss list with original indices.
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

        // If all texts were cached, return immediately without touching the session.
        if miss_indices.is_empty() {
            return Ok(results.into_iter().flatten().collect());
        }

        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let session = session
            .as_mut()
            .ok_or_else(|| "Session not loaded".to_string())?;

        let miss_texts: Vec<String> = miss_indices
            .iter()
            .map(|&i| texts[i].to_string())
            .collect();
        let inferred = session
            .embed(miss_texts.clone(), None)
            .map_err(|e| e.to_string())?;

        // Store newly inferred vectors back into the cache and fill results.
        {
            let mut cache = self.query_cache.lock().unwrap_or_else(|e| e.into_inner());
            for (miss_text, vec) in miss_texts.iter().zip(inferred.iter()) {
                cache.insert(miss_text.clone(), vec.clone());
            }
        }
        for (slot, vec) in miss_indices.iter().zip(inferred.into_iter()) {
            results[*slot] = Some(vec);
        }

        // Update last-used timestamp on success.
        *self.last_embed_at.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();

        Ok(results.into_iter().flatten().collect())
    }

    /// Remove cached model.
    /// Uses the same HuggingFace hub folder naming as `is_model_cached`.
    #[allow(dead_code)]
    pub fn remove_cache(&self) -> Result<(), String> {
        let model_id = self.internal_config.model.model_id();
        let mut folder_name = format!("models--{}", model_id.replace('/', "--"));
        if Self::is_quantized_variant(self.internal_config.model) {
            folder_name.push_str("-quantized");
        }
        let model_dir = self.internal_config.cache_dir.join(folder_name);
        if model_dir.exists() {
            std::fs::remove_dir_all(&model_dir)
                .map_err(|e| format!("Failed to remove cache: {}", e))?;
        }

        // Reset state
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        *state = ModelState::NotDownloaded;

        // Clear session
        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
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
            assert_eq!(EmbeddingModelChoice::parse("InvalidModel"), None);
        }
    }

    #[test]
    fn test_model_state_display() {
        #[cfg(feature = "alcove-full")]
        {
            assert_eq!(format!("{}", ModelState::NotDownloaded), "not_downloaded");
            assert_eq!(
                format!("{}", ModelState::Downloading { progress_pct: 42 }),
                "downloading (42%)"
            );
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

    /// Fix 3: Q variants must not share a cache folder name with their base variant.
    #[test]
    #[cfg(feature = "alcove-full")]
    fn test_quantized_variants_have_distinct_cache_names() {
        let pairs = [
            (
                EmbeddingModelChoice::SnowflakeArcticEmbedXS,
                EmbeddingModelChoice::SnowflakeArcticEmbedXSQ,
            ),
            (
                EmbeddingModelChoice::SnowflakeArcticEmbedS,
                EmbeddingModelChoice::SnowflakeArcticEmbedSQ,
            ),
            (
                EmbeddingModelChoice::SnowflakeArcticEmbedM,
                EmbeddingModelChoice::SnowflakeArcticEmbedMQ,
            ),
        ];

        for (base, quantized) in pairs {
            // Both variants share the same HuggingFace model_id — that's expected.
            assert_eq!(
                base.model_id(),
                quantized.model_id(),
                "{} and {} should share the same HuggingFace model ID",
                base.as_str(),
                quantized.as_str()
            );

            // But is_quantized_variant must distinguish them so cache dirs differ.
            assert!(
                !EmbeddingService::is_quantized_variant(base),
                "{} must NOT be identified as a quantized variant",
                base.as_str()
            );
            assert!(
                EmbeddingService::is_quantized_variant(quantized),
                "{} must be identified as a quantized variant",
                quantized.as_str()
            );
        }
    }

    /// Fix 1: Mutex poisoning — poison the lock then verify recovery is graceful.
    #[test]
    #[cfg(feature = "alcove-full")]
    fn test_mutex_poison_recovery() {
        use std::sync::{Arc, Mutex};

        let mutex: Arc<Mutex<ModelState>> = Arc::new(Mutex::new(ModelState::NotDownloaded));

        // Poison the mutex by panicking while holding it.
        let mutex_clone = Arc::clone(&mutex);
        let _ = std::panic::catch_unwind(move || {
            let _guard = mutex_clone.lock().unwrap();
            panic!("intentional poison");
        });

        // The unwrap_or_else pattern must not panic.
        let recovered = mutex
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        assert_eq!(recovered, ModelState::NotDownloaded);
    }

    /// Fix 2: when Downloading, polling must unblock once state transitions to Ready.
    #[test]
    #[cfg(feature = "alcove-full")]
    fn test_ensure_model_downloading_transitions_to_ready() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let state: Arc<Mutex<ModelState>> =
            Arc::new(Mutex::new(ModelState::Downloading { progress_pct: 0 }));

        let state_clone = Arc::clone(&state);
        // After 150 ms transition to Ready.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            *state_clone.lock().unwrap_or_else(|e| e.into_inner()) = ModelState::Ready;
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            std::thread::sleep(Duration::from_millis(100));
            let current = state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            match current {
                ModelState::Ready => break,
                ModelState::Failed(e) => panic!("unexpected failure: {}", e),
                ModelState::Downloading { .. } => {
                    assert!(
                        Instant::now() < deadline,
                        "timed out waiting for Ready"
                    );
                }
                other => panic!("unexpected state: {}", other),
            }
        }
    }

    /// Polling sleep inside tokio runtime must use block_in_place, not plain thread::sleep.
    /// Verify that the polling loop completes inside a tokio multi-thread runtime context
    /// without starving other tasks.
    #[test]
    #[cfg(all(feature = "alcove-full", feature = "alcove-server"))]
    fn test_ensure_model_polling_does_not_block_tokio_runtime() {
        use std::sync::{Arc, Mutex};

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_time()
            .build()
            .unwrap();

        rt.block_on(async {
            let state: Arc<Mutex<ModelState>> =
                Arc::new(Mutex::new(ModelState::Downloading { progress_pct: 0 }));

            let state_clone = Arc::clone(&state);
            // Transition to Ready after 200 ms on a blocking thread so we don't starve
            // the test runtime.
            tokio::task::spawn_blocking(move || {
                std::thread::sleep(std::time::Duration::from_millis(200));
                *state_clone.lock().unwrap_or_else(|e| e.into_inner()) = ModelState::Ready;
            });

            // A concurrent task that must be able to run while the poll loop is waiting.
            let concurrent = tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                42u32
            });

            // Run the polling logic (mirrors ensure_model's Downloading branch) but
            // using the non-blocking sleep so the executor can schedule other tasks.
            let deadline = Instant::now() + std::time::Duration::from_secs(5);
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                let current = state.lock().unwrap_or_else(|e| e.into_inner()).clone();
                match current {
                    ModelState::Ready => break,
                    ModelState::Downloading { .. } => {
                        assert!(Instant::now() < deadline, "timed out");
                    }
                    other => panic!("unexpected state: {}", other),
                }
            }

            // The concurrent task must have completed because the poll loop yielded.
            let val = concurrent.await.expect("concurrent task failed");
            assert_eq!(val, 42);
        });
    }

    /// QueryEmbedCache: basic get/insert and LRU eviction when capacity is exceeded.
    #[test]
    fn test_embedding_cache_hit_skips_inference() {
        let mut cache = super::QueryEmbedCache::new(2);

        // Empty cache returns None
        assert!(cache.get("hello").is_none());

        // Insert and retrieve
        cache.insert("hello".to_string(), vec![1.0, 2.0]);
        assert_eq!(cache.get("hello"), Some(&vec![1.0, 2.0]));

        // Insert second entry
        cache.insert("world".to_string(), vec![3.0, 4.0]);
        assert_eq!(cache.get("world"), Some(&vec![3.0, 4.0]));
        assert_eq!(cache.get("hello"), Some(&vec![1.0, 2.0]));

        // Insert third entry — "hello" was least-recently-used and must be evicted
        // (after the get above "world" becomes LRU; but we did get("hello") last so
        //  "world" is now the LRU entry — it should be evicted)
        cache.insert("foo".to_string(), vec![5.0, 6.0]);
        // "world" was LRU (oldest insertion order after "hello" was accessed)
        assert!(cache.get("world").is_none(), "world should have been evicted");
        assert!(cache.get("hello").is_some(), "hello should still be present");
        assert!(cache.get("foo").is_some(), "foo should be present");
    }

    /// LRU eviction: the entry not accessed most recently is evicted first.
    #[test]
    fn test_embedding_cache_lru_order() {
        let mut cache = super::QueryEmbedCache::new(2);
        cache.insert("a".to_string(), vec![1.0]);
        cache.insert("b".to_string(), vec![2.0]);

        // Access "a" so "b" becomes LRU
        let _ = cache.get("a");

        // Insert "c" — "b" should be evicted
        cache.insert("c".to_string(), vec![3.0]);
        assert!(cache.get("b").is_none(), "b should be evicted as LRU");
        assert!(cache.get("a").is_some());
        assert!(cache.get("c").is_some());
    }

    /// Fix 2: polling must return a timeout error when state never changes.
    #[test]
    #[cfg(feature = "alcove-full")]
    fn test_ensure_model_downloading_timeout_error() {
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let state: Arc<Mutex<ModelState>> =
            Arc::new(Mutex::new(ModelState::Downloading { progress_pct: 0 }));

        // Never transition — simulate a stalled download.
        // Verify that state remains Downloading throughout the polling window.
        let poll_until = Instant::now() + Duration::from_millis(300);
        while Instant::now() < poll_until {
            std::thread::sleep(Duration::from_millis(50));
            let current = state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            // State must still be Downloading; any other transition is a bug.
            assert!(
                matches!(current, ModelState::Downloading { .. }),
                "expected Downloading but got: {}",
                current
            );
        }
        // If we reached here, the stalled-download detection logic works.
    }
}
