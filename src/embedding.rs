//! Embedding service for hybrid search (embed feature)
//!
//! Uses llm-kernel for the model catalog (`EmbeddingModel`), LRU cache
//! (`EmbeddingCache`), and metadata lookups. `FastEmbedSession` wraps
//! `fastembed::TextEmbedding` directly for full prefix control — call sites
//! handle query/doc prefixes manually.

#[cfg(feature = "embed")]
use std::path::{Path, PathBuf};
#[cfg(feature = "embed")]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(feature = "embed")]
use std::time::{Duration, Instant};

#[cfg(feature = "embed")]
use anyhow::{Context, Result};
#[cfg(feature = "embed")]
use fastembed::TextEmbedding;
#[cfg(feature = "embed")]
use fastembed::TextInitOptions;
#[cfg(feature = "embed")]
use llm_kernel::embedding::EmbeddingCache;

// Re-export EmbeddingModel from llm-kernel for downstream use.
pub use llm_kernel::embedding::EmbeddingModel;

// ---------------------------------------------------------------------------
// ModelState
// ---------------------------------------------------------------------------

/// Model state for graceful degradation.
///
/// NOTE: `Loading` replaces the former candle `Downloading { progress_pct }`.
/// fastembed's `with_show_download_progress(true)` prints download progress
/// to stderr via hf-hub's built-in progress bar. No programmatic callback
/// is available in the fastembed API.
#[cfg(feature = "embed")]
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ModelState {
    #[default]
    NotLoaded,
    Loading,
    /// Model weights are present in the HuggingFace cache but not loaded into memory.
    Cached,
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
            Self::Cached => write!(f, "cached"),
            Self::Ready => write!(f, "ready"),
            Self::Disabled => write!(f, "disabled"),
            Self::Failed(e) => write!(f, "failed: {}", e),
        }
    }
}

// ---------------------------------------------------------------------------
// Legacy model name parser
// ---------------------------------------------------------------------------

/// Default model used when parsing fails.
const DEFAULT_MODEL: EmbeddingModel = EmbeddingModel::MultilingualE5Small;

/// Parse a model name, handling backward-compatible Arctic aliases.
///
/// New code should use `EmbeddingModel::parse()` directly. This function
/// maps legacy names (e.g., `"ArcticEmbedXS"`) to their current equivalents
/// (e.g., `EmbeddingModel::SnowflakeArcticEmbedXS`).
pub fn parse_legacy_model(name: &str) -> Option<EmbeddingModel> {
    // Try llm-kernel's case-insensitive parse first
    if let Ok(model) = EmbeddingModel::parse(name) {
        return Some(model);
    }
    // Legacy Arctic aliases (old alcove names → Snowflake-prefixed variants)
    let legacy = match name {
        "ArcticEmbedXS" => EmbeddingModel::SnowflakeArcticEmbedXS,
        "ArcticEmbedXSQ" => EmbeddingModel::SnowflakeArcticEmbedXSQ,
        "ArcticEmbedS" => EmbeddingModel::SnowflakeArcticEmbedS,
        "ArcticEmbedSQ" => EmbeddingModel::SnowflakeArcticEmbedSQ,
        "ArcticEmbedM" => EmbeddingModel::SnowflakeArcticEmbedM,
        "ArcticEmbedMQ" => EmbeddingModel::SnowflakeArcticEmbedMQ,
        "ArcticEmbedMLong" => EmbeddingModel::SnowflakeArcticEmbedMLong,
        "ArcticEmbedMLongQ" => EmbeddingModel::SnowflakeArcticEmbedMLongQ,
        "ArcticEmbedL" => EmbeddingModel::SnowflakeArcticEmbedL,
        "ArcticEmbedLQ" => EmbeddingModel::SnowflakeArcticEmbedLQ,
        _ => return None,
    };
    Some(legacy)
}

/// Resolve a model name with fallback to the default.
///
/// Single entry-point for all call sites — avoids duplicating
/// `parse_legacy_model().unwrap_or(DEFAULT_MODEL)` everywhere.
pub fn resolve_model(name: &str) -> EmbeddingModel {
    parse_legacy_model(name).unwrap_or(DEFAULT_MODEL)
}

/// Build the HuggingFace Hub cache folder name for a model.
///
/// Uses `model_code()` (the actual download repo) rather than the canonical
/// model name, matching the folder layout created by `hf-hub`.
#[cfg(feature = "embed")]
fn hf_cache_folder(model: EmbeddingModel) -> String {
    format!("models--{}", model.model_code().replace('/', "--"))
}

// ---------------------------------------------------------------------------
// FastEmbed session (ONNX Runtime inference engine)
// ---------------------------------------------------------------------------

/// Holds a loaded fastembed `TextEmbedding`, ready for inference.
///
/// Wraps `TextEmbedding` directly (not via llm-kernel's `FastembedProvider`)
/// to retain full prefix control — call sites handle query/doc prefixes.
#[cfg(feature = "embed")]
struct FastEmbedSession {
    model: TextEmbedding,
}

#[cfg(feature = "embed")]
impl FastEmbedSession {
    /// Download model (if needed) via fastembed's HuggingFace Hub integration
    /// and build an ONNX Runtime session.
    fn load(model: EmbeddingModel, cache_dir: &Path) -> Result<Self> {
        #[allow(unused_mut)]
        let mut opts = TextInitOptions::new(model.as_fastembed())
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
        // fastembed's embed() already L2-normalizes
        self.model.embed(texts, None)
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
    query_cache: Arc<Mutex<EmbeddingCache>>,
}

#[cfg(feature = "embed")]
struct InternalEmbeddingConfig {
    model: EmbeddingModel,
    cache_dir: PathBuf,
    enabled: bool,
}

#[cfg(feature = "embed")]
impl EmbeddingService {
    pub fn new(config: crate::config::EmbeddingConfig) -> Self {
        let model_choice = match parse_legacy_model(&config.model) {
            Some(m) => m,
            None => {
                let default = DEFAULT_MODEL;
                eprintln!(
                    "Warning: Unknown model '{}', using {} ({}d). \
                     Run 'alcove index' to rebuild the vector index if you changed models.",
                    config.model,
                    default.as_str(),
                    default.dimension()
                );
                default
            }
        };

        let enabled = config.enabled;
        let cache_dir = PathBuf::from(&config.cache_dir);
        let internal_config = InternalEmbeddingConfig {
            model: model_choice,
            cache_dir: cache_dir.clone(),
            enabled,
        };

        let initial_state = if !enabled {
            ModelState::Disabled
        } else if llm_kernel::embedding::is_model_cached(model_choice, &cache_dir) {
            ModelState::Cached
        } else {
            ModelState::NotLoaded
        };

        Self {
            state: Arc::new(Mutex::new(initial_state)),
            download_cvar: Arc::new(Condvar::new()),
            internal_config,
            session: Arc::new(Mutex::new(None)),
            previous_dimension: Arc::new(Mutex::new(None)),
            last_embed_at: Arc::new(Mutex::new(Instant::now())),
            query_cache: Arc::new(Mutex::new(EmbeddingCache::new(config.query_cache_size))),
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
            ModelState::NotLoaded | ModelState::Cached | ModelState::Loading => {
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
                ModelState::NotLoaded | ModelState::Cached | ModelState::Loading => {
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
            if *state != ModelState::NotLoaded && *state != ModelState::Cached {
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

                    match FastEmbedSession::load(model, &cache_dir) {
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
        // Session unloaded but cache remains on disk — next call will load from cache.
        *state = ModelState::Cached;
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
        let folder_name = hf_cache_folder(self.internal_config.model);
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

    pub fn model_choice(&self) -> EmbeddingModel {
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
            assert_eq!(EmbeddingModel::AllMiniLML6V2.dimension(), 384);
            assert_eq!(EmbeddingModel::MultilingualE5Small.dimension(), 384);
            assert_eq!(EmbeddingModel::MultilingualE5Base.dimension(), 768);
            assert_eq!(EmbeddingModel::MultilingualE5Large.dimension(), 1024);
            assert_eq!(EmbeddingModel::BGEM3.dimension(), 1024);
            assert_eq!(EmbeddingModel::SnowflakeArcticEmbedXS.dimension(), 384);
            assert_eq!(EmbeddingModel::SnowflakeArcticEmbedS.dimension(), 384);
            assert_eq!(EmbeddingModel::SnowflakeArcticEmbedM.dimension(), 768);
            assert_eq!(EmbeddingModel::SnowflakeArcticEmbedL.dimension(), 1024);
            assert_eq!(EmbeddingModel::BGESmallZHV15.dimension(), 512);
            assert_eq!(EmbeddingModel::SnowflakeArcticEmbedMLong.dimension(), 768);
        }
    }

    #[test]
    fn test_model_max_seq_length() {
        #[cfg(feature = "embed")]
        {
            assert_eq!(EmbeddingModel::AllMiniLML6V2.max_seq_length(), 256);
            assert_eq!(EmbeddingModel::AllMpnetBaseV2.max_seq_length(), 384);
            assert_eq!(EmbeddingModel::SnowflakeArcticEmbedXS.max_seq_length(), 512);
            assert_eq!(EmbeddingModel::SnowflakeArcticEmbedS.max_seq_length(), 512);
            assert_eq!(EmbeddingModel::SnowflakeArcticEmbedM.max_seq_length(), 512);
            assert_eq!(EmbeddingModel::SnowflakeArcticEmbedL.max_seq_length(), 512);
            assert_eq!(
                EmbeddingModel::SnowflakeArcticEmbedMLong.max_seq_length(),
                8192
            );
            assert_eq!(EmbeddingModel::BGEM3.max_seq_length(), 8192);
            assert_eq!(EmbeddingModel::NomicEmbedTextV15.max_seq_length(), 8192);
        }
    }

    #[test]
    fn test_model_prefixes() {
        #[cfg(feature = "embed")]
        {
            assert_eq!(EmbeddingModel::AllMiniLML6V2.query_prefix(), None);
            assert_eq!(EmbeddingModel::AllMiniLML6V2.doc_prefix(), None);

            for m in [
                EmbeddingModel::MultilingualE5Small,
                EmbeddingModel::MultilingualE5Base,
                EmbeddingModel::MultilingualE5Large,
            ] {
                assert_eq!(m.query_prefix(), Some("query: "), "{:?} query prefix", m);
                assert_eq!(m.doc_prefix(), Some("passage: "), "{:?} doc prefix", m);
            }

            assert_eq!(EmbeddingModel::BGEM3.query_prefix(), None);
            assert_eq!(EmbeddingModel::BGEM3.doc_prefix(), None);

            assert_eq!(
                EmbeddingModel::NomicEmbedTextV15.query_prefix(),
                Some("search_query: ")
            );
            assert_eq!(
                EmbeddingModel::NomicEmbedTextV15.doc_prefix(),
                Some("search_document: ")
            );

            let arctic_query_prefix = "Represent this sentence for searching relevant passages: ";
            for m in [
                EmbeddingModel::SnowflakeArcticEmbedXS,
                EmbeddingModel::SnowflakeArcticEmbedS,
                EmbeddingModel::SnowflakeArcticEmbedM,
                EmbeddingModel::SnowflakeArcticEmbedL,
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
    fn test_parse_legacy_model() {
        #[cfg(feature = "embed")]
        {
            // New names work directly
            assert_eq!(
                parse_legacy_model("AllMiniLML6V2"),
                Some(EmbeddingModel::AllMiniLML6V2)
            );
            assert_eq!(parse_legacy_model("BGEM3"), Some(EmbeddingModel::BGEM3));
            assert_eq!(
                parse_legacy_model("SnowflakeArcticEmbedXS"),
                Some(EmbeddingModel::SnowflakeArcticEmbedXS)
            );
            // Legacy Arctic aliases
            assert_eq!(
                parse_legacy_model("ArcticEmbedXS"),
                Some(EmbeddingModel::SnowflakeArcticEmbedXS)
            );
            assert_eq!(
                parse_legacy_model("ArcticEmbedL"),
                Some(EmbeddingModel::SnowflakeArcticEmbedL)
            );
            assert_eq!(
                parse_legacy_model("ArcticEmbedMLong"),
                Some(EmbeddingModel::SnowflakeArcticEmbedMLong)
            );
            // Unknown
            assert_eq!(parse_legacy_model("InvalidModel"), None);
        }
    }

    #[test]
    fn test_model_state_display() {
        #[cfg(feature = "embed")]
        {
            assert_eq!(format!("{}", ModelState::NotLoaded), "not_loaded");
            assert_eq!(format!("{}", ModelState::Loading), "loading");
            assert_eq!(format!("{}", ModelState::Cached), "cached");
            assert_eq!(format!("{}", ModelState::Ready), "ready");
            assert_eq!(
                format!("{}", ModelState::Failed("oops".to_string())),
                "failed: oops"
            );
        }
    }

    #[test]
    #[cfg(feature = "embed")]
    fn test_embedding_cache_hit_skips_inference() {
        let mut cache = EmbeddingCache::new(2);
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
    #[cfg(feature = "embed")]
    fn test_embedding_cache_lru_order() {
        let mut cache = EmbeddingCache::new(2);
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
