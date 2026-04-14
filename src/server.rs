//! HTTP server mode for external RAG access (alcove-server feature)
//!
//! Provides a REST API for search and document access.
//! Usage: `alcove serve --port 8080 [--token secret]`

#[cfg(feature = "alcove-server")]
use anyhow::Result;
#[cfg(feature = "alcove-server")]
use axum::{
    extract::{Query, State},
    http::{HeaderMap, Method, StatusCode, header},
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
use serde_json::Value;
#[cfg(feature = "alcove-server")]
use tower_http::cors::{AllowOrigin, CorsLayer};

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
    pub docs_root_configured: bool,
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
    pub token: Option<String>,
    /// Shared embedding service — initialised once at startup, reused per request.
    #[cfg(feature = "alcove-full")]
    pub embedding_service: Option<Arc<crate::embedding::EmbeddingService>>,
}

// ---------------------------------------------------------------------------
// Auth helpers
// ---------------------------------------------------------------------------

/// Constant-time string comparison to prevent timing attacks on bearer token auth.
/// Runs the full XOR loop regardless of length to avoid a length oracle.
#[cfg(feature = "alcove-server")]
fn constant_time_eq_str(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let len = a.len().max(b.len());
    let mut diff: u8 = (a.len() != b.len()) as u8;
    for i in 0..len {
        let x = if i < a.len() { a[i] } else { 0 };
        let y = if i < b.len() { b[i] } else { 0 };
        diff |= x ^ y;
    }
    diff == 0
}

/// Validates CORS origin against an allowlist of localhost origins.
/// Prevents `starts_with` bypass (e.g. `http://localhost.evil.com`).
#[cfg(feature = "alcove-server")]
fn is_allowed_origin(origin: &[u8]) -> bool {
    let s = std::str::from_utf8(origin).unwrap_or("");
    s == "http://localhost"
        || (s.starts_with("http://localhost:") && s[17..].parse::<u16>().is_ok())
        || (s.starts_with("http://127.0.0.1:") && s[17..].parse::<u16>().is_ok())
}

/// Check Bearer token authentication. Returns `Err` with 401 if the token is
/// set and the request does not supply the correct `Authorization: Bearer <token>`.
#[cfg(feature = "alcove-server")]
fn check_auth(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let Some(expected) = state.token.as_deref() else {
        return Ok(()); // no token configured → open access
    };
    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if constant_time_eq_str(provided, expected) {
        Ok(())
    } else {
        Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "Unauthorized".to_string(),
                code: 401,
            }),
        ))
    }
}

// ---------------------------------------------------------------------------
// Shared search logic
// ---------------------------------------------------------------------------

#[cfg(feature = "alcove-server")]
async fn handle_search(
    state: Arc<ServerState>,
    headers: HeaderMap,
    req: SearchRequest,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;

    if req.q.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Query cannot be empty".to_string(),
                code: 400,
            }),
        ));
    }

    if req.q.len() > 8192 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Query too long (max 8192 bytes)".to_string(),
                code: 400,
            }),
        ));
    }

    let docs_root = state.docs_root.clone();
    let project_filter_owned = req.project.clone();
    let q = req.q.clone();
    let limit = req.limit.clamp(1, 200);
    let use_hybrid = req.mode == "hybrid"
        || (req.mode == "auto" && cfg!(feature = "alcove-full"));

    // Try hybrid search first if available
    #[cfg(feature = "alcove-full")]
    if use_hybrid
        && let Some(service_arc) = state.embedding_service.clone()
            && crate::index::index_exists(&docs_root) {
                let docs_root2 = docs_root.clone();
                let q2 = q.clone();
                let pf = project_filter_owned.clone();

                let result = tokio::task::spawn_blocking(move || {
                    crate::index::search_hybrid(
                        &docs_root2,
                        &q2,
                        &service_arc,
                        limit,
                        pf.as_deref(),
                    )
                })
                .await
                .map_err(|_| (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "Internal search error".to_string(),
                        code: 500,
                    }),
                ))?;

                match result {
                    Ok(json) => {
                        let results: Vec<SearchResult> = json["matches"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|m| {
                                        Some(SearchResult {
                                            project: m["project"].as_str()?.to_string(),
                                            file: m["file"].as_str()?.to_string(),
                                            line: m["line_start"].as_u64()?,
                                            snippet: m["snippet"]
                                                .as_str()
                                                .unwrap_or("")
                                                .to_string(),
                                            score: m["score"].as_f64().unwrap_or(0.0),
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
                    Err(err) => {
                        eprintln!("[alcove] hybrid search error, falling back to BM25: {err}");
                    }
                }
            }

    // Fall back to BM25 search
    if crate::index::index_exists(&docs_root) {
        let docs_root2 = docs_root.clone();
        let q2 = q.clone();
        let pf = project_filter_owned.clone();

        let result = tokio::task::spawn_blocking(move || {
            crate::index::search_indexed(&docs_root2, &q2, limit, pf.as_deref())
        })
        .await
        .map_err(|_| (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Internal search error".to_string(),
                code: 500,
            }),
        ))?;

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
                                score: m["score"].as_f64().unwrap_or(0.0),
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

    Err((
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            error: "Search index not available. Run 'alcove index' first.".to_string(),
            code: 503,
        }),
    ))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

#[cfg(feature = "alcove-server")]
async fn health(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Result<Json<HealthResponse>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    let docs_root = state.docs_root.clone();
    let projects = tokio::task::spawn_blocking(move || {
        std::fs::read_dir(&docs_root)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| e.path().is_dir())
                    .count()
            })
            .unwrap_or(0)
    })
    .await
    .unwrap_or(0);

    Ok(Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        docs_root_configured: true,
        projects,
    }))
}

/// GET /search — query params
#[cfg(feature = "alcove-server")]
async fn get_search(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(req): Query<SearchRequest>,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    handle_search(state, headers, req).await
}

/// POST /v1/search — JSON body (OpenAI-compatible)
#[cfg(feature = "alcove-server")]
async fn post_search(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<SearchRequest>,
) -> Result<Json<SearchResponse>, (StatusCode, Json<ErrorResponse>)> {
    handle_search(state, headers, req).await
}

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

#[cfg(feature = "alcove-server")]
pub async fn run_server(
    docs_root: std::path::PathBuf,
    host: &str,
    port: u16,
    token: Option<String>,
) -> Result<()> {
    if token.is_none() {
        eprintln!(
            "  {} Alcove server running without authentication — anyone on the network can query it.",
            console::style("WARNING").yellow().bold()
        );
    }

    #[cfg(feature = "alcove-full")]
    let embedding_service = {
        use crate::embedding::{EmbeddingModelChoice, EmbeddingService};
        use crate::config::load_config;

        let cfg = load_config();
        let emb_cfg = cfg.embedding_config_with_defaults();

        if emb_cfg.enabled {
            let model = EmbeddingModelChoice::parse(&emb_cfg.model).unwrap_or_default();
            let cache_dir = std::path::PathBuf::from(
                emb_cfg
                    .cache_dir
                    .starts_with('~')
                    .then(|| std::env::var("HOME").ok())
                    .flatten()
                    .map(|h| emb_cfg.cache_dir.replacen('~', &h, 1))
                    .unwrap_or_else(|| emb_cfg.cache_dir.clone()),
            );
            Some(Arc::new(EmbeddingService::new(crate::config::EmbeddingConfig {
                model: model.as_str().to_string(),
                auto_download: emb_cfg.auto_download,
                cache_dir: cache_dir.to_string_lossy().into_owned(),
                enabled: true,
                query_cache_size: emb_cfg.query_cache_size,
            })))
        } else {
            None
        }
    };

    let state = Arc::new(ServerState {
        docs_root,
        token,
        #[cfg(feature = "alcove-full")]
        embedding_service,
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/search", get(get_search))
        .route("/v1/search", post(post_search))
        .route("/mcp", post(mcp_dispatch))
        .layer(axum::extract::DefaultBodyLimit::max(1_048_576))
        .layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::predicate(|origin, _| {
                    is_allowed_origin(origin.as_bytes())
                }))
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]),
        )
        .with_state(state);

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
    println!("      GET  /health     - Health check");
    println!("      GET  /search     - Search (q, limit, project, mode params)");
    println!("      POST /v1/search  - OpenAI-compatible search (JSON body)");
    println!("      POST /mcp        - MCP JSON-RPC dispatch (proxy target)");
    println!();

    axum::serve(listener, app).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// MCP JSON-RPC dispatch via HTTP — proxy target for stdio thin clients
// ---------------------------------------------------------------------------

#[cfg(feature = "alcove-server")]
async fn mcp_dispatch(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    body: String,
) -> (StatusCode, Json<Value>) {
    // Auth check (reuse existing token validation)
    if let Some(ref expected) = state.token {
        let provided = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        match provided {
            Some(t) if constant_time_eq_str(t, expected) => {}
            _ => {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({"error": "Unauthorized"})),
                );
            }
        }
    }

    let req: crate::mcp::RpcRequest = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            let resp = crate::mcp::RpcResponse::err(
                None,
                -32700,
                format!("Failed to parse JSON-RPC request: {e}"),
            );
            return (
                StatusCode::OK,
                Json(serde_json::to_value(&resp).unwrap_or_default()),
            );
        }
    };

    let req_id = req.id.clone();
    let result = tokio::task::spawn_blocking(move || crate::mcp::dispatch(req))
        .await
        .unwrap_or_else(|e| {
            eprintln!("[alcove] mcp dispatch task panicked: {e}");
            Some(crate::mcp::RpcResponse::err(
                req_id,
                -32603,
                "Internal server error".to_string(),
            ))
        });
    match result {
        Some(resp) => (
            StatusCode::OK,
            Json(serde_json::to_value(&resp).unwrap_or_default()),
        ),
        None => (
            StatusCode::NO_CONTENT,
            Json(serde_json::json!(null)),
        ),
    }
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
