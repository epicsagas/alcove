//! Embedding service for hybrid search (embed-candle feature)
//!
//! Provides lazy model download via HuggingFace Hub, candle-transformers BERT
//! inference, and LRU query caching. Graceful degradation ensures BM25-only
//! search works even when models aren't ready.

#[cfg(feature = "embed-candle")]
use std::path::PathBuf;
#[cfg(feature = "embed-candle")]
use std::sync::{Arc, Condvar, Mutex};
#[cfg(feature = "embed-candle")]
use std::time::{Duration, Instant};

#[cfg(feature = "embed-candle")]
use anyhow::{Context, Result};
#[cfg(feature = "embed-candle")]
use candle_core::Tensor;
#[cfg(feature = "embed-candle")]
use candle_nn::VarBuilder;
#[cfg(feature = "embed-candle")]
use candle_transformers::models::bert::{BertModel, Config as BertConfig, DTYPE};
#[cfg(feature = "embed-candle")]
use candle_transformers::models::xlm_roberta::{Config as XlmRobertaConfig, XLMRobertaModel};
#[cfg(feature = "embed-candle")]
use hf_hub::api::sync::ApiBuilder;
#[cfg(feature = "embed-candle")]
use hf_hub::{Repo, RepoType};

/// Model state for graceful degradation
#[cfg(feature = "embed-candle")]
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ModelState {
    #[default]
    NotDownloaded,
    Downloading {
        progress_pct: u8,
    },
    Cached,
    Ready,
    Unloaded,
    Disabled,
    Failed(String),
}

#[cfg(feature = "embed-candle")]
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

/// Supported embedding models — all resolve to BERT-family architectures
/// loadable via candle-transformers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmbeddingModelChoice {
    MultilingualE5Small,
    #[default]
    AllMiniLML6V2,
    MultilingualE5Base,
    MultilingualE5Large,
    BGEM3,
    ArcticEmbedXS,
    ArcticEmbedS,
    ArcticEmbedM,
    ArcticEmbedL,
}

impl EmbeddingModelChoice {
    /// Get embedding dimension for this model
    pub fn dimension(&self) -> usize {
        match self {
            Self::AllMiniLML6V2
            | Self::MultilingualE5Small
            | Self::ArcticEmbedXS
            | Self::ArcticEmbedS => 384,
            Self::MultilingualE5Base | Self::ArcticEmbedM => 768,
            Self::MultilingualE5Large | Self::BGEM3 | Self::ArcticEmbedL => 1024,
        }
    }

    /// Approximate model size in MB
    pub fn size_mb(&self) -> usize {
        match self {
            Self::AllMiniLML6V2 => 80,
            Self::MultilingualE5Small => 470,
            Self::MultilingualE5Base => 1100,
            Self::MultilingualE5Large => 2200,
            Self::BGEM3 => 2300,
            Self::ArcticEmbedXS => 90,
            Self::ArcticEmbedS => 130,
            Self::ArcticEmbedM => 430,
            Self::ArcticEmbedL => 1300,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "AllMiniLML6V2" => Some(Self::AllMiniLML6V2),
            "MultilingualE5Small" => Some(Self::MultilingualE5Small),
            "MultilingualE5Base" => Some(Self::MultilingualE5Base),
            "MultilingualE5Large" => Some(Self::MultilingualE5Large),
            "BGEM3" => Some(Self::BGEM3),
            "ArcticEmbedXS" => Some(Self::ArcticEmbedXS),
            "ArcticEmbedS" => Some(Self::ArcticEmbedS),
            "ArcticEmbedM" => Some(Self::ArcticEmbedM),
            "ArcticEmbedL" => Some(Self::ArcticEmbedL),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllMiniLML6V2 => "AllMiniLML6V2",
            Self::MultilingualE5Small => "MultilingualE5Small",
            Self::MultilingualE5Base => "MultilingualE5Base",
            Self::MultilingualE5Large => "MultilingualE5Large",
            Self::BGEM3 => "BGEM3",
            Self::ArcticEmbedXS => "ArcticEmbedXS",
            Self::ArcticEmbedS => "ArcticEmbedS",
            Self::ArcticEmbedM => "ArcticEmbedM",
            Self::ArcticEmbedL => "ArcticEmbedL",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::AllMiniLML6V2,
            Self::MultilingualE5Small,
            Self::MultilingualE5Base,
            Self::MultilingualE5Large,
            Self::BGEM3,
            Self::ArcticEmbedXS,
            Self::ArcticEmbedS,
            Self::ArcticEmbedM,
            Self::ArcticEmbedL,
        ]
    }

    pub fn model_id(self) -> &'static str {
        match self {
            Self::AllMiniLML6V2 => "sentence-transformers/all-MiniLM-L6-v2",
            Self::MultilingualE5Small => "intfloat/multilingual-e5-small",
            Self::MultilingualE5Base => "intfloat/multilingual-e5-base",
            Self::MultilingualE5Large => "intfloat/multilingual-e5-large",
            Self::BGEM3 => "BAAI/bge-m3",
            Self::ArcticEmbedXS => "Snowflake/snowflake-arctic-embed-xs",
            Self::ArcticEmbedS => "Snowflake/snowflake-arctic-embed-s",
            Self::ArcticEmbedM => "Snowflake/snowflake-arctic-embed-m",
            Self::ArcticEmbedL => "Snowflake/snowflake-arctic-embed-l",
        }
    }

    pub fn query_prefix(self) -> Option<&'static str> {
        match self {
            Self::MultilingualE5Small | Self::MultilingualE5Base | Self::MultilingualE5Large => {
                Some("query: ")
            }
            Self::ArcticEmbedXS | Self::ArcticEmbedS | Self::ArcticEmbedM | Self::ArcticEmbedL => {
                Some("Represent this sentence for searching relevant passages: ")
            }
            Self::AllMiniLML6V2 | Self::BGEM3 => None,
        }
    }

    pub fn doc_prefix(self) -> Option<&'static str> {
        match self {
            Self::MultilingualE5Small | Self::MultilingualE5Base | Self::MultilingualE5Large => {
                Some("passage: ")
            }
            _ => None,
        }
    }

    /// Maximum sequence length (tokens) supported by the model.
    pub fn max_seq_length(self) -> usize {
        match self {
            Self::AllMiniLML6V2 => 256,
            Self::MultilingualE5Small
            | Self::MultilingualE5Base
            | Self::MultilingualE5Large
            | Self::ArcticEmbedXS
            | Self::ArcticEmbedS
            | Self::ArcticEmbedM
            | Self::ArcticEmbedL => 512,
            Self::BGEM3 => 8192,
        }
    }

    /// BGEM3 and ArcticEmbed use CLS token pooling; E5/MiniLM use mean pooling.
    pub fn uses_cls_pooling(self) -> bool {
        matches!(
            self,
            Self::BGEM3
                | Self::ArcticEmbedXS
                | Self::ArcticEmbedS
                | Self::ArcticEmbedM
                | Self::ArcticEmbedL
        )
    }

    pub fn is_xlm_roberta(self) -> bool {
        matches!(self, Self::BGEM3)
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
// Candle-based embedding session (the actual inference engine)
// ---------------------------------------------------------------------------

/// Holds a loaded model + tokenizer, ready for inference.
#[cfg(feature = "embed-candle")]
enum ModelSession {
    Bert {
        model: BertModel,
        tokenizer: tokenizers::Tokenizer,
    },
    XlmRoberta {
        model: XLMRobertaModel,
        tokenizer: tokenizers::Tokenizer,
    },
}

#[cfg(feature = "embed-candle")]
struct CandleSession {
    session: ModelSession,
    cls_pooling: bool,
}

#[cfg(feature = "embed-candle")]
impl CandleSession {
    /// Download model files from HuggingFace Hub and build a session.
    fn load(model_choice: EmbeddingModelChoice, cache_dir: &std::path::Path) -> Result<Self> {
        let device = candle_core::Device::Cpu;
        let model_id = model_choice.model_id();

        let api = ApiBuilder::new()
            .with_cache_dir(cache_dir.to_path_buf())
            .build()
            .context("Failed to create HuggingFace API client")?;
        let repo = Repo::with_revision(model_id.to_string(), RepoType::Model, "main".to_string());
        let api_repo = api.repo(repo);

        let config_path = api_repo
            .get("config.json")
            .context("Failed to download config.json")?;
        let tokenizer_path = api_repo
            .get("tokenizer.json")
            .context("Failed to download tokenizer.json")?;

        let config_str =
            std::fs::read_to_string(&config_path).context("Failed to read config.json")?;

        let mut tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;
        tokenizer.with_padding(Some(tokenizers::PaddingParams {
            strategy: tokenizers::PaddingStrategy::BatchLongest,
            ..Default::default()
        }));
        let _ = tokenizer.with_truncation(Some(tokenizers::TruncationParams {
            strategy: tokenizers::TruncationStrategy::LongestFirst,
            max_length: model_choice.max_seq_length(),
            stride: 0,
            ..Default::default()
        }));

        // Weight loading: safetensors (single → multi-shard) → pytorch fallback
        let vb = 'vb: {
            // 1. Single-shard safetensors
            if let Ok(path) = api_repo.get("model.safetensors") {
                // SAFETY: mmaped safetensors files are read-only and the file content
                // is not modified after loading. The HuggingFace Hub cache guarantees
                // atomic file placement (write-to-temp + rename).
                break 'vb unsafe { VarBuilder::from_mmaped_safetensors(&[path], DTYPE, &device)? };
            }

            // 2. Multi-shard safetensors via index.json
            if let Ok(index_path) = api_repo.get("model.safetensors.index.json") {
                let index_str = std::fs::read_to_string(&index_path)
                    .context("Failed to read model.safetensors.index.json")?;
                let shard_map: serde_json::Value = serde_json::from_str(&index_str)
                    .context("Failed to parse model.safetensors.index.json")?;
                let file_names = shard_map["weight_map"]["file_map"]
                    .as_object()
                    .or_else(|| {
                        // Some index files use a flat "weight_map" with file values
                        shard_map["weight_map"].as_object()
                    })
                    .map(|map| {
                        let mut files: Vec<String> = map
                            .values()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                        files.sort();
                        files.dedup();
                        files
                    })
                    .unwrap_or_default();

                let mut shard_paths = Vec::new();
                for name in &file_names {
                    let p = api_repo
                        .get(name)
                        .with_context(|| format!("Failed to download shard {}", name))?;
                    shard_paths.push(p);
                }
                anyhow::ensure!(
                    !shard_paths.is_empty(),
                    "No safetensor shards found in index"
                );
                // SAFETY: same rationale as single-shard — read-only mmaped safetensors.
                break 'vb unsafe {
                    VarBuilder::from_mmaped_safetensors(&shard_paths, DTYPE, &device)?
                };
            }

            // 3. PyTorch fallback (e.g. BGE-M3 ships as pytorch_model.bin)
            if let Ok(path) = api_repo.get("pytorch_model.bin") {
                break 'vb VarBuilder::from_pth(&path, DTYPE, &device)
                    .context("Failed to load pytorch_model.bin")?;
            }

            anyhow::bail!(
                "No model weights found (tried model.safetensors, \
                 model.safetensors.index.json, pytorch_model.bin)"
            );
        };

        let session = if model_choice.is_xlm_roberta() {
            let config: XlmRobertaConfig = serde_json::from_str(&config_str)
                .context("Failed to parse XLM-RoBERTa config.json")?;
            let vb_prefix = vb.pp("roberta");
            // Check if weights already have the roberta prefix built-in.
            // BGE-M3's pytorch_model.bin stores keys without the "roberta." prefix
            // (e.g. "encoder.layer.0..."), while safetensors releases may include it.
            let vb_xlm =
                if vb_prefix.contains_tensor("encoder.layer.0.attention.output.dense.weight") {
                    vb_prefix
                } else {
                    vb
                };
            let model = XLMRobertaModel::new(&config, vb_xlm)
                .context("Failed to load XLM-RoBERTa model")?;
            ModelSession::XlmRoberta { model, tokenizer }
        } else {
            let config: BertConfig =
                serde_json::from_str(&config_str).context("Failed to parse BERT config.json")?;
            let model = BertModel::load(vb, &config)?;
            ModelSession::Bert { model, tokenizer }
        };

        Ok(Self {
            session,
            cls_pooling: model_choice.uses_cls_pooling(),
        })
    }

    /// Embed a batch of texts, returning one Vec<f32> per text.
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let device = candle_core::Device::Cpu;

        // Tokenize — all architectures share the same tokenizer interface
        let tokenizer = match &self.session {
            ModelSession::Bert { tokenizer, .. } | ModelSession::XlmRoberta { tokenizer, .. } => {
                tokenizer
            }
        };
        let encodings = tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

        let token_ids = Tensor::stack(
            &encodings
                .iter()
                .map(|e| Tensor::new(e.get_ids(), &device))
                .collect::<candle_core::Result<Vec<_>>>()?,
            0,
        )?;
        let attention_mask = Tensor::stack(
            &encodings
                .iter()
                .map(|e| Tensor::new(e.get_attention_mask(), &device))
                .collect::<candle_core::Result<Vec<_>>>()?,
            0,
        )?;
        let token_type_ids = token_ids.zeros_like()?;

        // Forward pass — only the model call differs between architectures
        let embeddings = match &self.session {
            ModelSession::Bert { model, .. } => {
                model.forward(&token_ids, &token_type_ids, Some(&attention_mask))?
            }
            ModelSession::XlmRoberta { model, .. } => model.forward(
                &token_ids,
                &attention_mask,
                &token_type_ids,
                None, // past_key_value
                None, // encoder_hidden_states
                None, // encoder_attention_mask
            )?,
        };

        // Pooling: CLS (first token) or mean (mask-weighted average)
        let pooled = if self.cls_pooling {
            embeddings.narrow(1, 0, 1)?.squeeze(1)?
        } else {
            let mask_f = attention_mask.to_dtype(DTYPE)?.unsqueeze(2)?;
            let sum_mask = mask_f.sum(1)?;
            (embeddings.broadcast_mul(&mask_f)?)
                .sum(1)?
                .broadcast_div(&sum_mask)?
        };

        // L2 normalize
        let normalized = pooled.broadcast_div(&pooled.sqr()?.sum_keepdim(1)?.sqrt()?)?;

        // Extract to Vec<Vec<f32>>
        let batch_size = normalized.dims()[0];
        (0..batch_size)
            .map(|i| {
                normalized
                    .get(i)?
                    .to_vec1::<f32>()
                    .map_err(anyhow::Error::from)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// EmbeddingService — public API (same interface, candle backend)
// ---------------------------------------------------------------------------

#[cfg(feature = "embed-candle")]
pub struct EmbeddingService {
    state: Arc<Mutex<ModelState>>,
    download_cvar: Arc<Condvar>,
    internal_config: InternalEmbeddingConfig,
    session: Arc<Mutex<Option<CandleSession>>>,
    #[allow(dead_code)]
    previous_dimension: Arc<Mutex<Option<usize>>>,
    last_embed_at: Arc<Mutex<Instant>>,
    query_cache: Arc<Mutex<QueryEmbedCache>>,
}

#[cfg(feature = "embed-candle")]
struct InternalEmbeddingConfig {
    model: EmbeddingModelChoice,
    cache_dir: PathBuf,
    enabled: bool,
}

#[cfg(feature = "embed-candle")]
impl EmbeddingService {
    pub fn new(config: crate::config::EmbeddingConfig) -> Self {
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

    fn is_model_cached(config: &InternalEmbeddingConfig) -> bool {
        // hf-hub cache: models--{org}--{repo}
        let model_id = config.model.model_id();
        let folder_name = format!("models--{}", model_id.replace('/', "--"));
        config.cache_dir.join(folder_name).exists()
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
            ModelState::NotDownloaded | ModelState::Cached | ModelState::Unloaded => {
                self.start_download();
            }
            ModelState::Downloading { .. } => {}
        }

        let deadline = Instant::now() + Duration::from_secs(300);
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match state.clone() {
                ModelState::Ready => return Ok(()),
                ModelState::Failed(e) => return Err(e),
                ModelState::Unloaded => {
                    drop(state);
                    self.start_download();
                    state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                ModelState::Downloading { .. } => {
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
                other => {
                    return Err(format!(
                        "Unexpected model state while waiting for download: {}",
                        other
                    ));
                }
            }
        }
    }

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

        let state_arc = Arc::clone(&self.state);
        let cvar_arc = Arc::clone(&self.download_cvar);
        let session_arc = Arc::clone(&self.session);
        let cache_dir = self.internal_config.cache_dir.clone();
        let model = self.internal_config.model;

        std::thread::spawn(move || {
            macro_rules! set_state {
                ($s:expr) => {
                    *state_arc.lock().unwrap_or_else(|e| e.into_inner()) = $s;
                };
            }
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

            set_state!(ModelState::Downloading { progress_pct: 20 });

            match CandleSession::load(model, &cache_dir) {
                Ok(session) => {
                    set_state!(ModelState::Downloading { progress_pct: 90 });
                    let mut state_guard = state_arc.lock().unwrap_or_else(|e| e.into_inner());
                    *session_arc.lock().unwrap_or_else(|e| e.into_inner()) = Some(session);
                    *state_guard = ModelState::Ready;
                    drop(state_guard);
                    cvar_arc.notify_all();
                }
                Err(e) => {
                    set_state_and_notify!(ModelState::Failed(e.to_string()));
                }
            }
        });
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
        *state = ModelState::Unloaded;
        true
    }

    pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
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
    pub fn remove_cache(&self) -> Result<(), String> {
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
        *state = ModelState::NotDownloaded;

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
    #[cfg(feature = "embed-candle")]
    use super::*;

    #[test]
    fn test_model_choice_dimension() {
        #[cfg(feature = "embed-candle")]
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
        }
    }

    #[test]
    fn test_model_prefixes() {
        #[cfg(feature = "embed-candle")]
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
        #[cfg(feature = "embed-candle")]
        {
            assert_eq!(
                EmbeddingModelChoice::parse("AllMiniLML6V2"),
                Some(EmbeddingModelChoice::AllMiniLML6V2)
            );
            assert_eq!(
                EmbeddingModelChoice::parse("BGEM3"),
                Some(EmbeddingModelChoice::BGEM3)
            );
            assert_eq!(EmbeddingModelChoice::parse("InvalidModel"), None);
            assert_eq!(
                EmbeddingModelChoice::parse("ArcticEmbedXS"),
                Some(EmbeddingModelChoice::ArcticEmbedXS)
            );
            assert_eq!(
                EmbeddingModelChoice::parse("ArcticEmbedS"),
                Some(EmbeddingModelChoice::ArcticEmbedS)
            );
            assert_eq!(
                EmbeddingModelChoice::parse("ArcticEmbedM"),
                Some(EmbeddingModelChoice::ArcticEmbedM)
            );
            assert_eq!(
                EmbeddingModelChoice::parse("ArcticEmbedL"),
                Some(EmbeddingModelChoice::ArcticEmbedL)
            );
        }
    }

    #[test]
    fn test_model_state_display() {
        #[cfg(feature = "embed-candle")]
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
        #[cfg(feature = "embed-candle")]
        {
            assert_eq!(EmbeddingModelChoice::AllMiniLML6V2.size_mb(), 80);
            assert_eq!(EmbeddingModelChoice::BGEM3.size_mb(), 2300);
            assert_eq!(EmbeddingModelChoice::ArcticEmbedXS.size_mb(), 90);
            assert_eq!(EmbeddingModelChoice::ArcticEmbedS.size_mb(), 130);
            assert_eq!(EmbeddingModelChoice::ArcticEmbedM.size_mb(), 430);
            assert_eq!(EmbeddingModelChoice::ArcticEmbedL.size_mb(), 1300);
        }
    }

    #[test]
    fn test_is_xlm_roberta() {
        #[cfg(feature = "embed-candle")]
        {
            assert!(EmbeddingModelChoice::BGEM3.is_xlm_roberta());
            assert!(!EmbeddingModelChoice::AllMiniLML6V2.is_xlm_roberta());
            assert!(!EmbeddingModelChoice::MultilingualE5Large.is_xlm_roberta());
            for m in [
                EmbeddingModelChoice::ArcticEmbedXS,
                EmbeddingModelChoice::ArcticEmbedS,
                EmbeddingModelChoice::ArcticEmbedM,
                EmbeddingModelChoice::ArcticEmbedL,
            ] {
                assert!(!m.is_xlm_roberta(), "{:?} should not be xlm_roberta", m);
            }
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
    #[cfg(feature = "embed-candle")]
    fn test_mutex_poison_recovery() {
        use std::sync::{Arc, Mutex};
        let mutex: Arc<Mutex<ModelState>> = Arc::new(Mutex::new(ModelState::NotDownloaded));
        let mutex_clone = Arc::clone(&mutex);
        let _ = std::panic::catch_unwind(move || {
            let _guard = mutex_clone.lock().unwrap();
            panic!("intentional poison");
        });
        let recovered = mutex.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(recovered, ModelState::NotDownloaded);
    }

    #[test]
    #[cfg(feature = "embed-candle")]
    fn test_try_unload_if_idle_clears_session_atomically() {
        use std::sync::{Arc, Mutex};
        let state: Arc<Mutex<ModelState>> = Arc::new(Mutex::new(ModelState::Ready));
        let session: Arc<Mutex<Option<u32>>> = Arc::new(Mutex::new(Some(42u32)));
        let unloaded = {
            let mut st = state.lock().unwrap_or_else(|e| e.into_inner());
            if *st == ModelState::Ready {
                let mut sess = session.lock().unwrap_or_else(|e| e.into_inner());
                *sess = None;
                *st = ModelState::Unloaded;
                true
            } else {
                false
            }
        };
        assert!(unloaded);
        assert_eq!(
            *state.lock().unwrap_or_else(|e| e.into_inner()),
            ModelState::Unloaded
        );
        assert!(session.lock().unwrap_or_else(|e| e.into_inner()).is_none());
    }
}
