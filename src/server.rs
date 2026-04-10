//! HTTP server mode for external RAG access (alcove-server feature)
//!
//! Provides a REST API for search and document access.
//! Usage: `alcove serve --port 8080 [--token secret]`

#[cfg(feature = "alcove-server")]
use anyhow::Result;
#[cfg(feature = "alcove-server")]
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
#[cfg(feature = "alcove-server")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "alcove-server")]
use std::net::SocketAddr;
#[cfg(feature = "alcove-server")]
use std::sync::Arc;
#[cfg(feature = "alcove-server")]
use tower_http::cors::{Any, CorsLayer};

// ---------------------------------------------------------------------------
// Request/Response types
// ---------------------------------------------------------------------------

#[cfg(feature = "alcove-server")]
#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    /// Search query
    pub q: String,
    /// Max results (default: 20)
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Project filter (optional)
    pub project: Option<String>,
    /// Search mode: auto, hybrid, bm25, grep
    #[serde(default = "default_mode")]
    pub mode: String,
}

#[cfg(feature = "alcove-server")]
fn default_limit() -> usize {
    20
}

#[cfg(feature = "alcove-server")]
fn default_mode() -> String {
    "auto".to_string()
}

#[cfg(feature = "alcove-server")]
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub project: String,
    pub file: String,
    pub line: u64,
    pub snippet: String,
    pub score: f64,
    pub source: String,
}

#[cfg(feature = "alcove-server")]
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub mode: String,
    pub truncated: bool,
}

#[cfg(feature = "alcove-server")]
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub docs_root: String,
    pub projects: usize,
}

#[cfg(feature = "alcove-server")]
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: u16,
}

// ---------------------------------------------------------------------------
// Server state
// ---------------------------------------------------------------------------

#[cfg(feature = "alcove-server")]
#[derive(Clone)]
pub struct ServerState {
    pub docs_root: std::path::PathBuf,
    #[allow(dead_code)]
    pub token: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[cfg(feature = "alcove-server")]
async fn health(State(state): State<Arc<ServerState>>) -> Json<HealthResponse> {
    let projects = std::fs::read_dir(&state.docs_root)
        .map(|entries| entries.filter_map(Result::ok).filter(|e| e.path().is_dir()).count())
        .unwrap_or(0);

    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        docs_root: state.docs_root.to_string_lossy().to_string(),
        projects,
    })
}

#[cfg(feature = "alcove-server")]
async fn search(
    State(state): State<Arc<ServerState>>,
    Query(req): Query<SearchRequest>,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate query
    if req.q.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Query cannot be empty".to_string(),
                code: 400,
            }),
        ));
    }

    let docs_root = &state.docs_root;
    let project_filter = req.project.as_deref();

    // Determine search mode
    let use_hybrid = req.mode == "hybrid"
        || (req.mode == "auto" && cfg!(feature = "alcove-full"));

    // Try hybrid search first if available
    #[cfg(feature = "alcove-full")]
    if use_hybrid {
        use crate::embedding::{EmbeddingModelChoice, EmbeddingService};
        use crate::config::load_config;

        if crate::index::index_exists(docs_root) {
            let cfg = load_config();
            let emb_cfg = cfg.embedding_config_with_defaults();

            if emb_cfg.enabled {
                let model = EmbeddingModelChoice::parse(&emb_cfg.model).unwrap_or_default();
                let cache_dir = std::path::PathBuf::from(
                    emb_cfg.cache_dir.starts_with('~')
                        .then(|| std::env::var("HOME").ok())
                        .flatten()
                        .map(|h| emb_cfg.cache_dir.replacen('~', &h, 1))
                        .unwrap_or_else(|| emb_cfg.cache_dir.clone())
                );

                let service = EmbeddingService::new(crate::config::EmbeddingConfig {
                    model: model.as_str().to_string(),
                    auto_download: emb_cfg.auto_download,
                    cache_dir: cache_dir.to_string_lossy().into_owned(),
                    enabled: true,
                });

                if service.state() == crate::embedding::ModelState::Ready {
                    let result = crate::index::search_hybrid(
                        docs_root,
                        &req.q,
                        &service,
                        req.limit,
                        project_filter,
                    );

                    if let Ok(json) = result {
                        let results: Vec<SearchResult> = json["matches"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|m| {
                                        Some(SearchResult {
                                            project: m["project"].as_str()?.to_string(),
                                            file: m["file"].as_str()?.to_string(),
                                            line: m["line_start"].as_u64()?,
                                            snippet: m["snippet"].as_str().unwrap_or("").to_string(),
                                            score: m["score"].as_f64()? * 100.0, // Scale to 0-100
                                            source: "hybrid".to_string(),
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();

                        return Ok(Json(SearchResponse {
                            query: req.q,
                            results,
                            mode: "hybrid".to_string(),
                            truncated: json["truncated"].as_bool().unwrap_or(false),
                        }));
                    }
                }
            }
        }
    }

    // Fall back to BM25 search
    if crate::index::index_exists(docs_root) {
        let result = crate::index::search_indexed(docs_root, &req.q, req.limit, project_filter);

        if let Ok(json) = result {
            let results: Vec<SearchResult> = json["matches"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            Some(SearchResult {
                                project: m["project"].as_str()?.to_string(),
                                file: m["file"].as_str()?.to_string(),
                                line: m["line_start"].as_u64()?,
                                snippet: m["snippet"].as_str().unwrap_or("").to_string(),
                                score: m["score"].as_f64()? * 100.0,
                                source: "bm25".to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            return Ok(Json(SearchResponse {
                query: req.q,
                results,
                mode: "bm25".to_string(),
                truncated: json["truncated"].as_bool().unwrap_or(false),
            }));
        }
    }

    // No index available
    Err((
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            error: "Search index not available. Run 'alcove index' first.".to_string(),
            code: 503,
        }),
    ))
}

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

#[cfg(feature = "alcove-server")]
pub async fn run_server(docs_root: std::path::PathBuf, host: &str, port: u16, token: Option<String>) -> Result<()> {
    let state = Arc::new(ServerState { docs_root, token });

    let app = Router::new()
        .route("/health", get(health))
        .route("/search", get(search))
        .route("/v1/search", post(search)) // OpenAI-compatible endpoint
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state);

    // Parse the bind host; fail clearly if invalid.
    let ip: std::net::IpAddr = host.parse().map_err(|e| {
        anyhow::anyhow!("Invalid server host '{}': {}", host, e)
    })?;
    let addr = SocketAddr::from((ip, port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    println!(
        "  {} Alcove RAG server running on http://{}",
        console::style("✓").green(),
        addr
    );
    println!("  {} Endpoints:", console::style("→").dim());
    println!("      GET  /health  - Health check");
    println!("      GET  /search  - Search (q, limit, project, mode params)");
    println!("      POST /v1/search - OpenAI-compatible search");
    println!();

    axum::serve(listener, app).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Stub for non-alcove-server builds
// ---------------------------------------------------------------------------

#[cfg(not(feature = "alcove-server"))]
pub async fn run_server(
    _docs_root: std::path::PathBuf,
    _host: &str,
    _port: u16,
    _token: Option<String>,
) -> anyhow::Result<()> {
    anyhow::bail!(
        "HTTP server requires 'alcove-server' feature. Install with: cargo install alcove --features alcove-server"
    )
}
