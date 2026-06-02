//! REST API route handlers for alcove HTTP server.
//!
//! Each handler wraps an existing `tools::tool_*` function behind a proper
//! REST endpoint with auth/rate-limit checks and `spawn_blocking` for
//! blocking I/O.

#[cfg(feature = "alcove-server")]
use anyhow::Result;
#[cfg(feature = "alcove-server")]
use axum::extract::{ConnectInfo, Path, Query, State};
#[cfg(feature = "alcove-server")]
use axum::http::{HeaderMap, StatusCode};
#[cfg(feature = "alcove-server")]
use axum::response::Json;
#[cfg(feature = "alcove-server")]
use serde::Deserialize;
#[cfg(feature = "alcove-server")]
use serde_json::{Value, json};
#[cfg(feature = "alcove-server")]
use std::net::SocketAddr;
#[cfg(feature = "alcove-server")]
use std::sync::Arc;

#[cfg(feature = "alcove-server")]
use crate::server::{ErrorResponse, ServerState, check_auth};

// ---------------------------------------------------------------------------
// Request query/body types
// ---------------------------------------------------------------------------

#[cfg(feature = "alcove-server")]
#[derive(Debug, Deserialize)]
pub struct ChangesQuery {
    pub auto_rebuild: Option<bool>,
}

#[cfg(feature = "alcove-server")]
#[derive(Debug, Deserialize)]
pub struct LintQuery {
    pub project: Option<String>,
}

#[cfg(feature = "alcove-server")]
#[derive(Debug, Deserialize)]
pub struct VaultSearchQuery {
    pub q: String,
    pub vault: Option<String>,
    #[serde(default = "default_vault_limit")]
    pub limit: usize,
}

#[cfg(feature = "alcove-server")]
fn default_vault_limit() -> usize {
    20
}

#[cfg(feature = "alcove-server")]
#[derive(Debug, Deserialize)]
pub struct BackupBody {
    pub vault_name: Option<String>,
}

#[cfg(feature = "alcove-server")]
#[derive(Debug, Deserialize)]
pub struct DocFileQuery {
    pub project: Option<String>,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
}

#[cfg(feature = "alcove-server")]
#[derive(Debug, Deserialize)]
pub struct InitProjectBody {
    pub project_name: String,
    pub project_path: Option<String>,
    #[serde(default)]
    pub overwrite: bool,
    pub files: Option<Vec<String>>,
}

#[cfg(feature = "alcove-server")]
#[derive(Debug, Deserialize)]
pub struct PromoteBody {
    pub source: String,
    pub project: Option<String>,
    #[serde(default = "default_true")]
    pub copy: bool,
}

#[cfg(feature = "alcove-server")]
fn default_true() -> bool {
    true
}

#[cfg(feature = "alcove-server")]
#[derive(Debug, Deserialize)]
pub struct ConfigureBody {
    pub diagram_format: Option<String>,
    pub core_files: Option<Vec<String>>,
    pub team_files: Option<Vec<String>>,
    pub public_files: Option<Vec<String>>,
}

#[cfg(feature = "alcove-server")]
#[derive(Debug, Deserialize)]
pub struct IndexCodeBody {
    pub source_path: String,
    pub language: Option<String>,
    pub project: Option<String>,
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Validate that `name` is a single normal path component (no traversal).
#[cfg(feature = "alcove-server")]
fn validate_project_name(name: &str) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    use std::path::Component;
    let p = std::path::Path::new(name);
    let components: Vec<_> = p.components().collect();
    if components.len() == 1 && matches!(components[0], Component::Normal(_)) {
        Ok(())
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Project not found".to_string(),
                code: 404,
            }),
        ))
    }
}

/// Convert a `Result<Value>` from tool functions into an HTTP response.
#[cfg(feature = "alcove-server")]
fn map_tool_result(
    result: Result<Value>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    match result {
        Ok(v) => Ok(Json(v)),
        Err(e) => {
            let msg = e.to_string();
            let status = if msg.contains("not found") || msg.contains("not exist") {
                StatusCode::NOT_FOUND
            } else if msg.contains("required") || msg.contains("invalid") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            Err((
                status,
                Json(ErrorResponse {
                    error: msg,
                    code: status.as_u16(),
                }),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Global handlers
// ---------------------------------------------------------------------------

/// GET /projects
#[cfg(feature = "alcove-server")]
pub async fn get_list_projects(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    let docs_root = state.docs_root.clone();
    let result = tokio::task::spawn_blocking(move || crate::tools::tool_list_projects(&docs_root))
        .await
        .unwrap_or_else(|e| {
            eprintln!("[alcove] list_projects task failed: {e}");
            Err(anyhow::anyhow!("Internal server error"))
        });
    map_tool_result(result)
}

/// POST /projects
#[cfg(feature = "alcove-server")]
pub async fn post_init_project(
    State(state): State<Arc<ServerState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<InitProjectBody>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    if !state.search_rate_limiter.check(peer.ip()) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(ErrorResponse {
                error: "Rate limit exceeded".to_string(),
                code: 429,
            }),
        ));
    }

    let docs_root = state.docs_root.clone();
    let docs_root_for_reindex = docs_root.clone();
    let args = json!({
        "project_name": body.project_name,
        "project_path": body.project_path,
        "overwrite": body.overwrite,
        "files": body.files,
    });

    let result =
        tokio::task::spawn_blocking(move || crate::tools::tool_init_project(&docs_root, args))
            .await
            .unwrap_or_else(|e| {
                eprintln!("[alcove] init_project task failed: {e}");
                Err(anyhow::anyhow!("Internal server error"))
            });

    // Trigger background reindex after init
    if result.is_ok() {
        std::thread::spawn(move || {
            let _ = crate::index::build_index(&docs_root_for_reindex);
        });
    }

    map_tool_result(result)
}

/// POST /rebuild
#[cfg(feature = "alcove-server")]
pub async fn post_rebuild(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    let docs_root = state.docs_root.clone();
    std::thread::spawn(move || {
        let _ = crate::index::build_index(&docs_root);
    });
    Ok(Json(json!({
        "status": "started",
        "message": "Index build started in background."
    })))
}

/// GET /changes
#[cfg(feature = "alcove-server")]
pub async fn get_changes(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<ChangesQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    let docs_root = state.docs_root.clone();
    let args = json!({ "auto_rebuild": query.auto_rebuild.unwrap_or(false) });
    let result =
        tokio::task::spawn_blocking(move || crate::tools::tool_check_doc_changes(&docs_root, args))
            .await
            .unwrap_or_else(|e| {
                eprintln!("[alcove] check_doc_changes task failed: {e}");
                Err(anyhow::anyhow!("Internal server error"))
            });
    map_tool_result(result)
}

/// GET /lint
#[cfg(feature = "alcove-server")]
pub async fn get_lint(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Query(query): Query<LintQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    let docs_root = state.docs_root.clone();
    let args = json!({ "project": query.project });
    let result =
        tokio::task::spawn_blocking(move || crate::tools::tool_lint_project(&docs_root, args))
            .await
            .unwrap_or_else(|e| {
                eprintln!("[alcove] lint task failed: {e}");
                Err(anyhow::anyhow!("Internal server error"))
            });
    map_tool_result(result)
}

/// GET /vaults/search
#[cfg(feature = "alcove-server")]
pub async fn get_vault_search(
    State(state): State<Arc<ServerState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<VaultSearchQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    if !state.search_rate_limiter.check(peer.ip()) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(ErrorResponse {
                error: "Rate limit exceeded".to_string(),
                code: 429,
            }),
        ));
    }

    let vault_name = query.vault.as_deref().unwrap_or("*");
    let limit = query.limit.clamp(1, 200);
    let search_all = vault_name == "*" || vault_name.is_empty();

    if search_all {
        let q = query.q.clone();
        let result = tokio::task::spawn_blocking(move || {
            let vaults = crate::vault::list_vaults()?;
            use rayon::prelude::*;
            let vault_results: Vec<_> = vaults
                .par_iter()
                .filter_map(|vault| crate::index::search_vault(&vault.path, &q, limit).ok())
                .collect();
            let mut all_matches: Vec<Value> = Vec::new();
            for result in vault_results {
                if let Some(matches) = result["matches"].as_array() {
                    all_matches.extend(matches.iter().cloned());
                }
            }
            all_matches.sort_by(|a, b| {
                let sa = a["score"].as_f64().unwrap_or(0.0);
                let sb = b["score"].as_f64().unwrap_or(0.0);
                sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
            });
            all_matches.truncate(limit);
            Ok(json!({ "query": q, "scope": "all_vaults", "matches": all_matches }))
        })
        .await
        .unwrap_or_else(|e| {
            eprintln!("[alcove] vault search task failed: {e}");
            Err(anyhow::anyhow!("Internal server error"))
        });
        return map_tool_result(result);
    }

    // Single vault search
    validate_project_name(vault_name)?;
    let vault_path = crate::vault::vaults_root().join(vault_name);
    if !vault_path.is_dir() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Vault not found".to_string(),
                code: 404,
            }),
        ));
    }

    let q = query.q.clone();
    let result =
        tokio::task::spawn_blocking(move || crate::index::search_vault(&vault_path, &q, limit))
            .await
            .unwrap_or_else(|e| {
                eprintln!("[alcove] vault search task failed: {e}");
                Err(anyhow::anyhow!("Internal server error"))
            });
    map_tool_result(result)
}

/// GET /vaults
#[cfg(feature = "alcove-server")]
pub async fn get_list_vaults(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    let result = tokio::task::spawn_blocking(crate::vault::list_vaults)
        .await
        .unwrap_or_else(|e| {
            eprintln!("[alcove] list_vaults task failed: {e}");
            Err(anyhow::anyhow!("Internal server error"))
        });

    match result {
        Ok(vaults) => {
            let arr: Vec<Value> = vaults
                .into_iter()
                .map(|v| {
                    json!({
                        "name": v.name,
                        "doc_count": v.doc_count,
                        "is_link": v.is_link,
                        "path": v.path.to_string_lossy(),
                    })
                })
                .collect();
            Ok(Json(json!(arr)))
        }
        Err(e) => map_tool_result(Err(e)),
    }
}

/// POST /vaults/backup
#[cfg(feature = "alcove-server")]
pub async fn post_backup_vault(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    body: Option<axum::Json<BackupBody>>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    let args = match body {
        Some(axum::Json(b)) => json!({ "vault_name": b.vault_name }),
        None => json!({}),
    };
    let result = tokio::task::spawn_blocking(move || crate::tools::tool_backup_vault(args))
        .await
        .unwrap_or_else(|e| {
            eprintln!("[alcove] backup_vault task failed: {e}");
            Err(anyhow::anyhow!("Internal server error"))
        });
    map_tool_result(result)
}

/// POST /promote
#[cfg(feature = "alcove-server")]
pub async fn post_promote(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<PromoteBody>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    let docs_root = state.docs_root.clone();
    let args = json!({
        "source": body.source,
        "project": body.project,
        "copy": body.copy,
    });
    let result =
        tokio::task::spawn_blocking(move || crate::tools::tool_promote_document(&docs_root, args))
            .await
            .unwrap_or_else(|e| {
                eprintln!("[alcove] promote task failed: {e}");
                Err(anyhow::anyhow!("Internal server error"))
            });
    map_tool_result(result)
}

/// POST /index-code
#[cfg(feature = "alcove-server")]
pub async fn post_index_code(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<IndexCodeBody>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    let docs_root = state.docs_root.clone();
    let project_name = body.project.unwrap_or_else(|| {
        crate::tools::resolve_project(&docs_root)
            .map(|r| r.name)
            .unwrap_or_default()
    });
    let args = json!({
        "source_path": body.source_path,
        "language": body.language,
    });
    let result = tokio::task::spawn_blocking(move || {
        crate::tools::tool_index_code_structure(&docs_root, &project_name, args)
    })
    .await
    .unwrap_or_else(|e| {
        eprintln!("[alcove] index_code task failed: {e}");
        Err(anyhow::anyhow!("Internal server error"))
    });
    map_tool_result(result)
}

// ---------------------------------------------------------------------------
// Project-scoped handlers
// ---------------------------------------------------------------------------

/// GET /projects/{name}/docs
#[cfg(feature = "alcove-server")]
pub async fn get_project_docs(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    validate_project_name(&name)?;

    let docs_root = state.docs_root.clone();
    let project_root = docs_root.join(&name);
    if !project_root.is_dir() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Project not found".to_string(),
                code: 404,
            }),
        ));
    }

    let name_clone = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        let repo_path = std::env::current_dir().ok();
        crate::tools::tool_overview(&project_root, &name_clone, "rest-api", repo_path.as_deref())
    })
    .await
    .unwrap_or_else(|e| {
        eprintln!("[alcove] project_docs task failed: {e}");
        Err(anyhow::anyhow!("Internal server error"))
    });
    map_tool_result(result)
}

/// GET /docs/{*path}
#[cfg(feature = "alcove-server")]
pub async fn get_doc_file(
    State(srv): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(parts): Path<Vec<String>>,
    Query(query): Query<DocFileQuery>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&srv, &headers)?;

    let project_name = query.project.unwrap_or_else(|| {
        crate::tools::resolve_project(&srv.docs_root)
            .map(|r| r.name)
            .unwrap_or_default()
    });
    if project_name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "project query parameter required when project cannot be auto-detected"
                    .to_string(),
                code: 400,
            }),
        ));
    }
    validate_project_name(&project_name)?;

    let project_root = srv.docs_root.join(&project_name);
    if !project_root.is_dir() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Project not found".to_string(),
                code: 404,
            }),
        ));
    }

    let relative_path = parts.join("/");
    let args = json!({
        "relative_path": relative_path,
        "offset": query.offset,
        "limit": query.limit,
    });

    let result =
        tokio::task::spawn_blocking(move || crate::tools::tool_get_file(&project_root, args))
            .await
            .unwrap_or_else(|e| {
                eprintln!("[alcove] get_doc_file task failed: {e}");
                Err(anyhow::anyhow!("Internal server error"))
            });
    map_tool_result(result)
}

/// GET /projects/{name}/audit
#[cfg(feature = "alcove-server")]
pub async fn get_audit(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    validate_project_name(&name)?;

    let docs_root = state.docs_root.clone();
    let project_root = docs_root.join(&name);
    if !project_root.is_dir() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Project not found".to_string(),
                code: 404,
            }),
        ));
    }

    let name_clone = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        let repo_path = std::env::current_dir().ok();
        crate::tools::tool_audit(&project_root, &name_clone, repo_path.as_deref())
    })
    .await
    .unwrap_or_else(|e| {
        eprintln!("[alcove] audit task failed: {e}");
        Err(anyhow::anyhow!("Internal server error"))
    });
    map_tool_result(result)
}

/// GET /projects/{name}/validate
#[cfg(feature = "alcove-server")]
pub async fn get_validate(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    validate_project_name(&name)?;

    let docs_root = state.docs_root.clone();
    let name_clone = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        let source = crate::policy::policy_source(&docs_root, &name_clone);
        let repo_path = std::env::current_dir().ok();
        let (pol, results) = crate::policy::validate(&docs_root, &name_clone, repo_path.as_deref());
        Ok(crate::policy::validation_to_json(&pol, &results, source))
    })
    .await
    .unwrap_or_else(|e| {
        eprintln!("[alcove] validate task failed: {e}");
        Err(anyhow::anyhow!("Internal server error"))
    });
    map_tool_result(result)
}

/// PUT /projects/{name}/config
#[cfg(feature = "alcove-server")]
pub async fn put_configure(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
    axum::Json(body): axum::Json<ConfigureBody>,
) -> Result<Json<Value>, (StatusCode, Json<ErrorResponse>)> {
    check_auth(&state, &headers)?;
    validate_project_name(&name)?;

    let repo_path = std::env::current_dir().unwrap_or_else(|_| state.docs_root.clone());
    let args = json!({
        "project_name": name,
        "diagram_format": body.diagram_format,
        "core_files": body.core_files,
        "team_files": body.team_files,
        "public_files": body.public_files,
    });

    let result =
        tokio::task::spawn_blocking(move || crate::tools::tool_configure_project(&repo_path, args))
            .await
            .unwrap_or_else(|e| {
                eprintln!("[alcove] configure task failed: {e}");
                Err(anyhow::anyhow!("Internal server error"))
            });
    map_tool_result(result)
}
