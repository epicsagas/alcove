mod cli;

use std::env;
use std::io::{self, BufRead, Write as _};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use walkdir::WalkDir;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "alcove", version, about = "Project documentation management & MCP server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Interactive setup: docs root, categories, diagram format, agents
    Setup,
    /// Remove skills, config, and legacy files
    Uninstall,
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

impl RpcResponse {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self { jsonrpc: "2.0".into(), id, result: Some(result), error: None }
    }

    fn err(id: Option<Value>, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(RpcError { code, message }),
        }
    }
}

// ---------------------------------------------------------------------------
// MCP tool description (with JSON Schema)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct ToolDescription {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => serve(),
        Some(Commands::Setup) => cli::cmd_setup(),
        Some(Commands::Uninstall) => cli::cmd_uninstall(),
    }
}

// ---------------------------------------------------------------------------
// MCP server — stdio JSON-RPC loop
// ---------------------------------------------------------------------------

fn serve() -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let req: RpcRequest = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let resp = RpcResponse::err(
                    None,
                    -32700,
                    format!("Failed to parse request: {e}"),
                );
                writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
                stdout.flush()?;
                continue;
            }
        };

        let resp = dispatch(req);
        writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
        stdout.flush()?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Method dispatch
// ---------------------------------------------------------------------------

fn dispatch(req: RpcRequest) -> RpcResponse {
    let id = req.id.clone();
    match req.method.as_str() {
        "initialize" => handle_initialize(id),
        "notifications/initialized" => RpcResponse::ok(id, json!({})),
        "tools/list" => handle_tools_list(id),
        "tools/call" => handle_tool_call(id, req.params),
        other => RpcResponse::err(id, -32601, format!("Unknown method: {other}")),
    }
}

// ---------------------------------------------------------------------------
// initialize
// ---------------------------------------------------------------------------

fn handle_initialize(id: Option<Value>) -> RpcResponse {
    RpcResponse::ok(
        id,
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "alcove",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )
}

// ---------------------------------------------------------------------------
// tools/list
// ---------------------------------------------------------------------------

fn handle_tools_list(id: Option<Value>) -> RpcResponse {
    let tools = vec![
        ToolDescription {
            name: "get_project_docs_overview".into(),
            description: concat!(
                "List all documentation files for the current project with sizes and classification. ",
                "Scans both alcove (doc-repo) and project repository. ",
                "Classifications: doc-repo-required (core private), doc-repo-supplementary (internal extras), ",
                "project-repo (public-facing), reference (reports/), unrecognized."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        ToolDescription {
            name: "search_project_docs".into(),
            description: "Case-insensitive keyword search within the current project's documentation. Returns matching lines with file path and line number.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search keyword (case-insensitive)" },
                    "limit": { "type": "number", "description": "Max results to return (default: 20)" }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        },
        ToolDescription {
            name: "get_doc_file".into(),
            description: "Read a specific documentation file by its relative path. Supports offset/limit for large files.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "relative_path": { "type": "string", "description": "File path relative to project docs root (e.g. \"PRD.md\", \"reports/AUDIT.md\")" },
                    "offset": { "type": "number", "description": "Character offset to start reading from" },
                    "limit": { "type": "number", "description": "Max characters to return" }
                },
                "required": ["relative_path"],
                "additionalProperties": false
            }),
        },
        ToolDescription {
            name: "list_projects".into(),
            description: "List all project folders in alcove. Shows which projects have documentation available.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        ToolDescription {
            name: "audit_project".into(),
            description: concat!(
                "Audit project docs across both alcove (doc-repo) and the project repository. ",
                "Scans: 1) alcove for private/internal docs, 2) project repo root + docs/ for public docs. ",
                "Suggests: generating missing public docs from internal content, incorporating project repo materials into alcove. ",
                "NEVER suggests exposing raw internal docs to the project repo. ",
                "IMPORTANT: Present findings to the user and ask which actions to proceed with. Never auto-execute."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        ToolDescription {
            name: "init_project".into(),
            description: concat!(
                "Initialize documentation for a project from the standard template. ",
                "Creates internal docs (PRD, ARCHITECTURE, DECISIONS, etc.) in alcove. ",
                "When project_path is provided, also creates external docs ",
                "(README, CHANGELOG, QUICKSTART) in the project repository. ",
                "Use the 'files' parameter to create only specific documents. ",
                "Without 'files', creates all missing internal required docs."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "project_name": {
                        "type": "string",
                        "description": "Project name (used as folder name in alcove)"
                    },
                    "project_path": {
                        "type": "string",
                        "description": "Absolute path to the project repository. If provided, creates README.md and CHANGELOG.md there."
                    },
                    "overwrite": {
                        "type": "boolean",
                        "description": "Overwrite existing files (default: false)"
                    },
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Specific files to create (e.g. [\"PRD.md\", \"ARCHITECTURE.md\"]). If omitted, creates all Tier 1 docs."
                    }
                },
                "required": ["project_name"],
                "additionalProperties": false
            }),
        },
    ];

    RpcResponse::ok(id, json!({ "tools": tools }))
}

// ---------------------------------------------------------------------------
// tools/call — dispatch to individual tool handlers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ToolCallParams {
    /// MCP spec uses "name"; we also accept "tool_name" for backward compat.
    #[serde(alias = "tool_name")]
    name: String,
    #[serde(default)]
    arguments: Value,
}

fn handle_tool_call(id: Option<Value>, params: Value) -> RpcResponse {
    let call: ToolCallParams = match serde_json::from_value(params) {
        Ok(v) => v,
        Err(e) => {
            return RpcResponse::err(id, -32602, format!("Invalid tool call params: {e}"));
        }
    };

    let docs_root = match env::var("DOCS_ROOT") {
        Ok(v) => PathBuf::from(v),
        Err(_) => {
            // Fallback: read docs_root from config.toml
            match load_config().docs_root() {
                Some(p) if p.is_dir() => p,
                _ => {
                    return RpcResponse::err(
                        id,
                        -32000,
                        "DOCS_ROOT environment variable is not set and config.toml has no docs_root.".into(),
                    );
                }
            }
        }
    };

    // Tools that don't require a project context
    match call.name.as_str() {
        "list_projects" => {
            return match tool_list_projects(&docs_root) {
                Ok(v) => RpcResponse::ok(id, mcp_text_result(&v)),
                Err(e) => RpcResponse::err(id, -32002, format!("list_projects failed: {e}")),
            };
        }
        "init_project" => {
            return match tool_init_project(&docs_root, call.arguments) {
                Ok(v) => RpcResponse::ok(id, mcp_text_result(&v)),
                Err(e) => RpcResponse::err(id, -32002, format!("init_project failed: {e}")),
            };
        }
        _ => {}
    }

    // Tools that require project context
    let resolved = match resolve_project(&docs_root) {
        Some(result) => result,
        None => {
            // List available projects to help the agent
            let available: Vec<String> = std::fs::read_dir(&docs_root)
                .ok()
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().is_dir())
                        .filter_map(|e| {
                            let name = e.file_name().to_string_lossy().to_string();
                            if name.starts_with('.') || name.starts_with('_')
                                || name == "mcp" || name == "skills" || name == "scripts"
                            {
                                None
                            } else {
                                Some(name)
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            return RpcResponse::err(
                id,
                -32001,
                format!(
                    "Could not detect project. CWD does not match any project in DOCS_ROOT. \
                     Available projects: [{}]. \
                     Set MCP_PROJECT_NAME env var or run from within a project directory.",
                    available.join(", ")
                ),
            );
        }
    };

    let project_root = docs_root.join(&resolved.name);

    let repo_path = resolved.repo_path.as_deref();

    let result = match call.name.as_str() {
        "get_project_docs_overview" => tool_overview(&project_root, &resolved.name, resolved.detected_via, repo_path),
        "search_project_docs" => tool_search(&project_root, call.arguments, repo_path),
        "get_doc_file" => tool_get_file(&project_root, call.arguments),
        "audit_project" => tool_audit(&project_root, &resolved.name, repo_path),
        other => Err(anyhow::anyhow!("Unknown tool: {other}")),
    };

    match result {
        Ok(v) => RpcResponse::ok(id, mcp_text_result(&v)),
        Err(e) => RpcResponse::err(id, -32002, format!("Tool `{}` failed: {e}", call.name)),
    }
}

/// Wrap a JSON value as MCP text content.
fn mcp_text_result(value: &Value) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(value).unwrap_or_default()
        }]
    })
}

// ---------------------------------------------------------------------------
// Project name resolution
// ---------------------------------------------------------------------------

/// Resolved project information.
struct ResolvedProject {
    name: String,
    detected_via: &'static str,
    /// The project repository path (from CWD), if detected.
    repo_path: Option<PathBuf>,
}

/// Resolve the active project name using this priority:
///   1. MCP_PROJECT_NAME env var (explicit override)
///   2. CWD-based auto-detection (walk up path components, match against DOCS_ROOT)
///
/// Returns ResolvedProject or None.
fn resolve_project(docs_root: &Path) -> Option<ResolvedProject> {
    // 1. Explicit env override — highest priority
    if let Ok(name) = env::var("MCP_PROJECT_NAME") {
        let name = name.trim().to_string();
        if !name.is_empty() && docs_root.join(&name).is_dir() {
            // Still try to detect repo_path from CWD
            let repo_path = detect_repo_path(&name);
            return Some(ResolvedProject { name, detected_via: "env", repo_path });
        }
    }

    // 2. Auto-detect from CWD — walk path components, check against DOCS_ROOT
    if let Ok(cwd) = env::current_dir() {
        let available: Vec<String> = std::fs::read_dir(docs_root)
            .ok()?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') || name.starts_with('_')
                    || name == "mcp" || name == "skills" || name == "scripts"
                {
                    None
                } else {
                    Some(name)
                }
            })
            .collect();

        // Walk CWD components from deepest to shallowest
        let mut path = cwd.as_path();
        loop {
            if let Some(dirname) = path.file_name().and_then(|f| f.to_str())
                && available.iter().any(|p| p == dirname) {
                    // `path` is the directory whose name matched — that's the repo root
                    let repo_path = Some(path.to_path_buf());
                    return Some(ResolvedProject {
                        name: dirname.to_string(),
                        detected_via: "cwd",
                        repo_path,
                    });
                }
            match path.parent() {
                Some(parent) if parent != path => path = parent,
                _ => break,
            }
        }
    }

    None
}

/// Try to find the project repo path from CWD for env-based detection.
fn detect_repo_path(project_name: &str) -> Option<PathBuf> {
    let cwd = env::current_dir().ok()?;
    let mut path = cwd.as_path();
    loop {
        if let Some(dirname) = path.file_name().and_then(|f| f.to_str())
            && dirname == project_name {
                return Some(path.to_path_buf());
            }
        match path.parent() {
            Some(parent) if parent != path => path = parent,
            _ => break,
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tool: get_project_docs_overview
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Document tier classification
//
//   doc-repo    = alcove가 관리하는 문서 저장소 — 비공개, 팀 내부용
//   project-repo = 프로젝트 레포 — GitHub 공유, 외부 기여자 대면
// ---------------------------------------------------------------------------

/// Doc-repo required docs — core project docs stored in alcove.
pub const DOC_REPO_REQUIRED: &[&str] = &[
    "PRD.md",
    "ARCHITECTURE.md",
    "PROGRESS.md",
    "DECISIONS.md",
    "CONVENTIONS.md",
    "SECRETS_MAP.md",
    "DEBT.md",
];

/// Doc-repo supplementary docs — recognized extras in alcove.
/// Dev-team-oriented docs that don't need public exposure.
pub const DOC_REPO_SUPPLEMENTARY: &[&str] = &[
    // Dev environment & onboarding
    "ENV_SETUP.md",
    "ONBOARDING.md",
    // Data model & specs
    "DATA_MODEL.md",
    "SCHEMA.md",
    // Operations (internal runbooks)
    "DEPLOYMENT.md",
    "RUNBOOK.md",
    "PLAYBOOK.md",
    "MONITORING.md",
    "INFRASTRUCTURE.md",
    "RELEASE.md",
    "RELEASE_PROCESS.md",
    "MIGRATION.md",
    "UPGRADING.md",
    // Quality & testing (internal)
    "TESTING.md",
    "BENCHMARK.md",
    "PERFORMANCE.md",
    "STYLE_GUIDE.md",
    // Internal reference
    "GLOSSARY.md",
    "TROUBLESHOOTING.md",
];

/// Project-repo docs — typically found in the project repository (GitHub).
/// Used to classify files when scanning the project repo, NOT to suggest
/// moving alcove files outward.
pub const PROJECT_REPO_FILES: &[&str] = &[
    // GitHub community health files
    "README.md",
    "CHANGELOG.md",
    "CONTRIBUTING.md",
    "SECURITY.md",
    "CODE_OF_CONDUCT.md",
    "LICENSE.md",
    "SUPPORT.md",
    "AUTHORS.md",
    "CONTRIBUTORS.md",
    "CODEOWNERS",
    // User-facing guides
    "QUICKSTART.md",
    "INSTALL.md",
    "API.md",
    "FAQ.md",
];

// ---------------------------------------------------------------------------
// Dynamic config from ~/.config/alcove/config.toml
// Falls back to hardcoded defaults above when config is absent.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
struct CategoryConfig {
    #[serde(default)]
    files: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct DiagramConfig {
    #[serde(default = "default_diagram_format")]
    format: String,
}

fn default_diagram_format() -> String {
    "mermaid".into()
}

#[derive(Debug, Deserialize, Clone)]
pub struct DocConfig {
    #[serde(default)]
    docs_root: Option<String>,
    #[serde(default)]
    core: Option<CategoryConfig>,
    #[serde(default)]
    team: Option<CategoryConfig>,
    #[serde(default)]
    public: Option<CategoryConfig>,
    #[serde(default)]
    diagram: Option<DiagramConfig>,
}

impl DocConfig {
    pub fn core_files(&self) -> Vec<String> {
        self.core.as_ref().map_or_else(
            || DOC_REPO_REQUIRED.iter().map(|s| s.to_string()).collect(),
            |c| c.files.clone(),
        )
    }

    pub fn team_files(&self) -> Vec<String> {
        self.team.as_ref().map_or_else(
            || DOC_REPO_SUPPLEMENTARY.iter().map(|s| s.to_string()).collect(),
            |c| c.files.clone(),
        )
    }

    pub fn public_files(&self) -> Vec<String> {
        self.public.as_ref().map_or_else(
            || PROJECT_REPO_FILES.iter().map(|s| s.to_string()).collect(),
            |c| c.files.clone(),
        )
    }

    pub fn diagram_format(&self) -> String {
        self.diagram
            .as_ref()
            .map_or_else(default_diagram_format, |d| d.format.clone())
    }

    fn docs_root(&self) -> Option<PathBuf> {
        self.docs_root.as_ref().map(PathBuf::from)
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(home) = env::var("HOME") {
        PathBuf::from(home).join(".config/alcove/config.toml")
    } else {
        PathBuf::from("/nonexistent")
    }
}

pub fn load_config() -> &'static DocConfig {
    static CONFIG: OnceLock<DocConfig> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let path = config_path();
        if path.exists()
            && let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(cfg) = toml::from_str::<DocConfig>(&content) {
                    return cfg;
                }
        // Fallback: empty config → all methods return hardcoded defaults
        DocConfig { docs_root: None, core: None, team: None, public: None, diagram: None }
    })
}

fn classify_tier(relative_path: &str) -> &'static str {
    let filename = Path::new(relative_path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");
    let lower = filename.to_lowercase();

    let cfg = load_config();

    if cfg.core_files().iter().any(|f| f.to_lowercase() == lower) {
        "doc-repo-required"
    } else if relative_path.starts_with("reports/") || relative_path.starts_with("reports\\") {
        "reference"
    } else if cfg.team_files().iter().any(|f| f.to_lowercase() == lower) {
        "doc-repo-supplementary"
    } else if cfg.public_files().iter().any(|f| f.to_lowercase() == lower) {
        "project-repo"
    } else {
        "unrecognized"
    }
}

fn tool_overview(project_root: &Path, project_name: &str, detected_via: &str, repo_path: Option<&Path>) -> Result<Value> {
    let mut bridge_files = Vec::new();

    for entry in WalkDir::new(project_root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        if !is_doc_file(path) {
            continue;
        }

        let rel = path
            .strip_prefix(project_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        let meta = entry.metadata()?;
        bridge_files.push(json!({
            "path": rel,
            "size_bytes": meta.len(),
            "tier": classify_tier(&rel),
        }));
    }

    // Scan project repo (root + docs/) if available
    let mut repo_files = Vec::new();
    if let Some(rp) = repo_path {
        // Root-level doc files
        for entry in std::fs::read_dir(rp).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_file() && is_doc_file(&path) {
                let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                repo_files.push(json!({
                    "path": filename,
                    "size_bytes": size,
                    "tier": classify_tier(filename),
                }));
            }
        }
        // docs/ subfolder
        let docs_dir = rp.join("docs");
        if docs_dir.is_dir() {
            for entry in WalkDir::new(&docs_dir)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_file())
            {
                let path = entry.path();
                if !is_doc_file(path) {
                    continue;
                }
                let rel = path.strip_prefix(rp).unwrap_or(path).to_string_lossy().to_string();
                let size = entry.metadata().ok().map(|m| m.len()).unwrap_or(0);
                repo_files.push(json!({
                    "path": rel,
                    "size_bytes": size,
                    "tier": classify_tier(&rel),
                }));
            }
        }
    }

    let total = bridge_files.len() + repo_files.len();

    let mut result = json!({
        "project_name": project_name,
        "detected_via": detected_via,
        "total_files": total,
        "doc_repo": {
            "path": project_root.to_string_lossy(),
            "files": bridge_files,
            "count": bridge_files.len(),
        },
        "diagram_format": load_config().diagram_format(),
        "hint": "Start with PRD.md (what/why), ARCHITECTURE.md (how), PROGRESS.md (status)",
        "diagram_hint": format!("Use {} syntax when creating or updating diagrams in docs.", load_config().diagram_format()),
    });

    if let Some(rp) = repo_path {
        result["project_repo"] = json!({
            "path": rp.to_string_lossy(),
            "files": repo_files,
            "count": repo_files.len(),
        });
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Tool: search_project_docs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SearchArgs {
    query: String,
    #[serde(default = "default_search_limit")]
    limit: usize,
}

fn default_search_limit() -> usize {
    20
}

fn search_dir_for_query(
    dir: &Path,
    base: &Path,
    query_lower: &str,
    source: &str,
    limit: usize,
    matches: &mut Vec<Value>,
) {
    if !dir.exists() || matches.len() >= limit {
        return;
    }
    for entry in WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        if matches.len() >= limit {
            return;
        }
        let path = entry.path();
        if !is_doc_file(path) {
            continue;
        }
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let rel = path
            .strip_prefix(base)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        for (idx, line) in content.lines().enumerate() {
            if line.to_lowercase().contains(query_lower) {
                matches.push(json!({
                    "file": rel,
                    "line": idx + 1,
                    "snippet": line.trim(),
                    "source": source,
                }));
                if matches.len() >= limit {
                    return;
                }
            }
        }
    }
}

fn tool_search(project_root: &Path, args_value: Value, repo_path: Option<&Path>) -> Result<Value> {
    let args: SearchArgs = serde_json::from_value(args_value)
        .context("search_project_docs requires { query, limit? }")?;

    let query_lower = args.query.to_lowercase();
    let mut matches = Vec::new();

    // 1. Search alcove folder
    search_dir_for_query(project_root, project_root, &query_lower, "alcove", args.limit, &mut matches);

    // 2. Search project repo (root-level + docs/) if available
    if let Some(rp) = repo_path {
        // Root-level docs (depth 1 only, not recursive)
        for entry in std::fs::read_dir(rp).into_iter().flatten().flatten() {
            if matches.len() >= args.limit {
                break;
            }
            let path = entry.path();
            if !path.is_file() || !is_doc_file(&path) {
                continue;
            }
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
            for (idx, line) in content.lines().enumerate() {
                if line.to_lowercase().contains(&query_lower) {
                    matches.push(json!({
                        "file": filename,
                        "line": idx + 1,
                        "snippet": line.trim(),
                        "source": "project-repo",
                    }));
                    if matches.len() >= args.limit {
                        break;
                    }
                }
            }
        }
        // docs/ subfolder (recursive)
        search_dir_for_query(&rp.join("docs"), rp, &query_lower, "project-repo", args.limit, &mut matches);
    }

    let truncated = matches.len() >= args.limit;
    Ok(json!({ "query": args.query, "matches": matches, "truncated": truncated }))
}

// ---------------------------------------------------------------------------
// Tool: get_doc_file
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GetFileArgs {
    relative_path: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

fn tool_get_file(project_root: &Path, args_value: Value) -> Result<Value> {
    let args: GetFileArgs = serde_json::from_value(args_value)
        .context("get_doc_file requires { relative_path, offset?, limit? }")?;

    let safe_rel = Path::new(&args.relative_path);
    if safe_rel
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        anyhow::bail!("Path traversal is not allowed");
    }

    let full_path = project_root.join(safe_rel);
    if !full_path.exists() || !full_path.is_file() {
        anyhow::bail!("File does not exist: {}", args.relative_path);
    }
    if !is_doc_file(&full_path) {
        anyhow::bail!("File type not allowed: {}", args.relative_path);
    }

    let content = std::fs::read_to_string(&full_path)?;
    let sliced = slice_content(&content, args.offset, args.limit);

    Ok(json!({
        "path": args.relative_path,
        "content": sliced,
        "total_chars": content.len(),
    }))
}

// ---------------------------------------------------------------------------
// Tool: list_projects
// ---------------------------------------------------------------------------

fn tool_list_projects(docs_root: &Path) -> Result<Value> {
    let mut projects = Vec::new();

    let entries = std::fs::read_dir(docs_root)
        .context("Failed to read DOCS_ROOT directory")?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // Skip hidden dirs, _template, mcp, skills
        if name.starts_with('.') || name.starts_with('_') || name == "mcp" || name == "skills" {
            continue;
        }

        // Count doc files
        let doc_count = WalkDir::new(&path)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file() && is_doc_file(e.path()))
            .count();

        // Check which internal required files exist
        let core_files = load_config().core_files();
        let internal_present: Vec<String> = core_files
            .iter()
            .filter(|f| path.join(f).exists())
            .cloned()
            .collect();

        let internal_missing: Vec<String> = core_files
            .iter()
            .filter(|f| !path.join(f).exists())
            .cloned()
            .collect();

        projects.push(json!({
            "name": name,
            "total_docs": doc_count,
            "internal_required_present": internal_present,
            "internal_required_missing": internal_missing,
        }));
    }

    Ok(json!({ "projects": projects }))
}

// ---------------------------------------------------------------------------
// Tool: init_project
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct InitProjectArgs {
    project_name: String,
    #[serde(default)]
    project_path: Option<String>,
    #[serde(default)]
    overwrite: Option<bool>,
    /// When set, only create these specific files instead of all Tier 1 docs.
    #[serde(default)]
    files: Option<Vec<String>>,
}

fn tool_init_project(docs_root: &Path, args_value: Value) -> Result<Value> {
    let args: InitProjectArgs = serde_json::from_value(args_value)
        .context("init_project requires { project_name, project_path?, overwrite? }")?;

    // Validate project name
    let name = args.project_name.trim();
    if name.is_empty()
        || name.contains("..")
        || name.contains('/')
        || name.contains('\\')
        || name.starts_with('.')
        || name.starts_with('_')
    {
        anyhow::bail!(
            "Invalid project name: `{name}`. Must not contain path separators, start with . or _, or be empty."
        );
    }

    let template_root = docs_root.join("_template");
    if !template_root.exists() {
        anyhow::bail!(
            "Template directory not found. Expected at: {}",
            template_root.display()
        );
    }

    let project_root = docs_root.join(name);
    let overwrite = args.overwrite.unwrap_or(false);

    // --- Internal: Copy template to alcove ---
    std::fs::create_dir_all(project_root.join("reports"))?;

    let mut bridge_created = Vec::new();
    let mut bridge_skipped = Vec::new();

    // If `files` is specified, only create those specific files
    let file_filter: Option<Vec<String>> = args
        .files
        .map(|f| f.iter().map(|s| s.trim().to_string()).collect());

    for entry in WalkDir::new(&template_root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let rel = entry
            .path()
            .strip_prefix(&template_root)
            .unwrap_or(entry.path());

        let filename = rel
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("");

        // Skip template README (usage guide) and .gitkeep
        if rel == Path::new("README.md") || filename == ".gitkeep" {
            continue;
        }

        // If file_filter is set, skip files not in the list
        if let Some(ref filter) = file_filter {
            let rel_str = rel.to_string_lossy();
            if !filter.iter().any(|f| f == filename || f == rel_str.as_ref()) {
                continue;
            }
        }

        let dest = project_root.join(rel);

        if dest.exists() && !overwrite {
            bridge_skipped.push(rel.to_string_lossy().to_string());
            continue;
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = std::fs::read_to_string(entry.path())?;
        let content = content.replace("ProjectName", name);
        std::fs::write(&dest, content)?;

        bridge_created.push(rel.to_string_lossy().to_string());
    }

    // --- External: Create user-facing docs in project repository ---
    let mut repo_created = Vec::new();
    let mut repo_skipped = Vec::new();
    let mut repo_path_used = String::new();

    if let Some(ref project_path) = args.project_path {
        let project_dir = PathBuf::from(project_path);
        if !project_dir.exists() || !project_dir.is_dir() {
            anyhow::bail!(
                "project_path does not exist or is not a directory: {project_path}"
            );
        }

        repo_path_used = project_path.clone();

        // Helper: create a file in the repo if it doesn't exist (or overwrite)
        let mut create_repo_file = |filename: &str, content: String| -> Result<()> {
            // If file_filter is set, skip files not in the list
            if let Some(ref filter) = file_filter
                && !filter.iter().any(|f| f == filename) {
                    return Ok(());
                }
            let dest = project_dir.join(filename);
            if dest.exists() && !overwrite {
                repo_skipped.push(filename.to_string());
            } else {
                std::fs::write(&dest, content)?;
                repo_created.push(filename.to_string());
            }
            Ok(())
        };

        // README.md
        create_repo_file(
            "README.md",
            format!(
                r#"# {name}

> TODO: Brief project description

## Quick Start

```bash
# TODO: Quick start steps
```

## Installation

```bash
# TODO: Installation steps
```

## Usage

```bash
# TODO: Usage examples
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development guidelines.

## License

TODO: Choose a license
"#,
            ),
        )?;

        // CHANGELOG.md
        create_repo_file(
            "CHANGELOG.md",
            format!(
                r#"# Changelog

All notable changes to {name} will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial project setup
"#,
            ),
        )?;

        // QUICKSTART.md
        create_repo_file(
            "QUICKSTART.md",
            format!(
                r#"# {name} — Quick Start

Get up and running in under 5 minutes.

## Prerequisites

- TODO: List required tools and versions

## Steps

```bash
# 1. Clone the repository
git clone <repo-url>
cd {name}

# 2. Install dependencies
# TODO: install command

# 3. Configure environment
# TODO: env setup

# 4. Run
# TODO: run command
```

## Verify

```bash
# TODO: verification command
```

## Next Steps

- Read the full [README](README.md) for detailed usage
- Check [CONTRIBUTING.md](CONTRIBUTING.md) for development setup
"#,
            ),
        )?;
    }

    Ok(json!({
        "project_name": name,
        "internal_docs": {
            "location": "alcove (private)",
            "path": project_root.to_string_lossy(),
            "created": bridge_created,
            "skipped": bridge_skipped,
        },
        "external_docs": {
            "location": "project repo (public)",
            "path": repo_path_used,
            "created": repo_created,
            "skipped": repo_skipped,
        },
        "next_steps": [
            format!("Edit {name}/PRD.md — define requirements and goals"),
            format!("Edit {name}/ARCHITECTURE.md — define tech stack and structure"),
            format!("Edit {name}/CONVENTIONS.md — define coding rules"),
            format!("Edit {name}/SECRETS_MAP.md — map environment variables"),
        ]
    }))
}

// ---------------------------------------------------------------------------
// Tool: audit_project
// ---------------------------------------------------------------------------

fn tool_audit(project_root: &Path, project_name: &str, repo_path: Option<&Path>) -> Result<Value> {
    let mut tier1_status = Vec::new();
    let core_files = load_config().core_files();

    for file in &core_files {
        let path = project_root.join(file);
        let exists = path.exists();
        let size = if exists {
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        let has_placeholder = if exists {
            std::fs::read_to_string(&path)
                .map(|c| c.contains("ProjectName"))
                .unwrap_or(false)
        } else {
            false
        };

        let status = if !exists {
            "missing"
        } else if has_placeholder {
            "template-unfilled"
        } else if size < 100 {
            "minimal"
        } else {
            "populated"
        };

        tier1_status.push(json!({
            "file": file,
            "exists": exists,
            "size_bytes": size,
            "status": status,
        }));
    }

    // Categorize non-required files by tier classification
    let mut supplementary_files = Vec::new();
    // No separate external_files — everything in alcove is internal by design.
    let mut unrecognized_files = Vec::new();

    for entry in WalkDir::new(project_root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        if !is_doc_file(path) {
            continue;
        }

        let rel = path
            .strip_prefix(project_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        let filename = Path::new(&rel)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("");

        // Skip internal required files and reports/
        if core_files.iter().any(|f| f == filename) || rel.starts_with("reports/") {
            continue;
        }

        let meta = entry.metadata()?;
        let tier = classify_tier(&rel);

        match tier {
            "doc-repo-supplementary" | "project-repo" => {
                // Everything in alcove is managed internally.
                supplementary_files.push(json!({
                    "path": rel,
                    "size_bytes": meta.len(),
                }));
            }
            _ => {
                let hint = suggest_categorization(filename);
                unrecognized_files.push(json!({
                    "path": rel,
                    "size_bytes": meta.len(),
                    "hint": hint,
                }));
            }
        }
    }

    // --- Scan project repository (root + docs/) for doc files ---
    let mut repo_docs = Vec::new();
    let mut repo_path_str = String::new();

    if let Some(rp) = repo_path {
        repo_path_str = rp.to_string_lossy().to_string();

        // Root-level docs only (depth 1)
        for entry in std::fs::read_dir(rp).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_file() && is_doc_file(&path) {
                let filename = path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or("");
                let rel = filename.to_string();
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let tier = classify_tier(&rel);
                repo_docs.push(json!({
                    "path": rel,
                    "filename": filename,
                    "size_bytes": size,
                    "tier": tier,
                }));
            }
        }

        // docs/ subfolder (recursive) — paths shown as docs/...
        let docs_dir = rp.join("docs");
        if docs_dir.is_dir() {
            for entry in WalkDir::new(&docs_dir)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_file())
            {
                let path = entry.path();
                if !is_doc_file(path) {
                    continue;
                }
                let rel = path
                    .strip_prefix(rp)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();
                let filename = path
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or("");
                let size = entry.metadata().ok().map(|m| m.len()).unwrap_or(0);
                let tier = classify_tier(&rel);
                repo_docs.push(json!({
                    "path": rel,
                    "filename": filename,
                    "size_bytes": size,
                    "tier": tier,
                }));
            }
        }
    }

    let missing_files: Vec<String> = tier1_status
        .iter()
        .filter(|s| s["status"] == "missing")
        .filter_map(|s| s["file"].as_str().map(String::from))
        .collect();
    let unfilled_files: Vec<String> = tier1_status
        .iter()
        .filter(|s| s["status"] == "template-unfilled")
        .filter_map(|s| s["file"].as_str().map(String::from))
        .collect();
    let minimal_files: Vec<String> = tier1_status
        .iter()
        .filter(|s| s["status"] == "minimal")
        .filter_map(|s| s["file"].as_str().map(String::from))
        .collect();
    let populated = core_files.len() - missing_files.len() - unfilled_files.len() - minimal_files.len();

    // Build structured suggested_actions for the AI agent
    let mut suggested_actions: Vec<Value> = Vec::new();

    if !missing_files.is_empty() {
        suggested_actions.push(json!({
            "action": "create_missing_internal",
            "description": format!("Create {} missing internal doc(s): {}", missing_files.len(), missing_files.join(", ")),
            "tool": "init_project",
            "params": {
                "project_name": project_name,
                "files": missing_files,
            }
        }));
    }

    if !unfilled_files.is_empty() {
        suggested_actions.push(json!({
            "action": "fill_templates",
            "description": format!(
                "Fill {} template doc(s) with actual project content: {}",
                unfilled_files.len(),
                unfilled_files.join(", ")
            ),
            "tool": "agent_edit",
            "params": {
                "files": unfilled_files,
            },
            "note": "Read each file, analyze the project source, and replace placeholder content."
        }));
    }

    if !minimal_files.is_empty() {
        suggested_actions.push(json!({
            "action": "expand_minimal",
            "description": format!(
                "Expand {} minimal doc(s) with more detail: {}",
                minimal_files.len(),
                minimal_files.join(", ")
            ),
            "tool": "agent_edit",
            "params": {
                "files": minimal_files,
            },
            "note": "These files exist but have very little content (<100 bytes)."
        }));
    }

    if !unrecognized_files.is_empty() {
        suggested_actions.push(json!({
            "action": "review_unrecognized",
            "description": format!(
                "{} unrecognized file(s) in alcove",
                unrecognized_files.len()
            ),
            "files": &unrecognized_files,
            "note": "Present the list to the user for review."
        }));
    }

    // --- Cross-repo suggestions ---
    if repo_path.is_some() {
        let repo_filenames: Vec<String> = repo_docs.iter()
            .filter_map(|d| d["filename"].as_str().map(String::from))
            .collect();

        // 1) Project repo에 없는 공개 문서 → alcove 기반으로 생성 제안
        let public_files = load_config().public_files();
        let missing_public: Vec<&str> = public_files.iter()
            .filter(|f| !repo_filenames.iter().any(|r| r.to_lowercase() == f.to_lowercase()))
            .map(|s| s.as_str())
            .collect();
        if !missing_public.is_empty() {
            suggested_actions.push(json!({
                "action": "generate_public_docs",
                "description": format!(
                    "Project repo is missing {} public doc(s): {}",
                    missing_public.len(),
                    missing_public.join(", ")
                ),
                "note": "Can generate these from alcove content, formatted for public consumption. Internal details will NOT be exposed."
            }));
        }

        // 2) Project repo에 내부 문서와 동일한 파일이 있으면 → 내용 비교 후 병합/제거 제안
        let mut duplicate_in_repo = Vec::new();
        for doc in &repo_docs {
            let fname = doc["filename"].as_str().unwrap_or("");
            let fname_lower = fname.to_lowercase();
            if core_files.iter().any(|f| f.to_lowercase() == fname_lower) {
                // 내부 필수 문서와 동일한 파일이 project repo에도 있음
                duplicate_in_repo.push(fname.to_string());
            }
        }
        if !duplicate_in_repo.is_empty() {
            suggested_actions.push(json!({
                "action": "resolve_exposed_internal_docs",
                "severity": "warning",
                "description": format!(
                    "Project repo contains {} internal doc(s) that should only exist in alcove: {}",
                    duplicate_in_repo.len(),
                    duplicate_in_repo.join(", ")
                ),
                "note": concat!(
                    "These are private docs (PRD, ARCHITECTURE, etc.) exposed in the public repo. ",
                    "BEFORE removing from project repo, compare content with alcove version. ",
                    "If the project repo version has additional content, merge it into the alcove version first, ",
                    "then remove from the project repo."
                )
            }));
        }

        // 3) Project repo에 분석/보고서 성격 파일이 있으면 → alcove reports/로 가져오기 제안
        let mut repo_reports = Vec::new();
        let mut repo_incorporable = Vec::new();
        for doc in &repo_docs {
            let fname = doc["filename"].as_str().unwrap_or("");
            let fname_lower = fname.to_lowercase();
            let tier = doc["tier"].as_str().unwrap_or("");

            // 이미 공개 문서로 분류된 건 건너뜀
            if tier == "project-repo" {
                continue;
            }
            // 이미 위에서 중복으로 처리된 건 건너뜀
            if duplicate_in_repo.iter().any(|d| d.to_lowercase() == fname_lower) {
                continue;
            }

            let is_report = fname_lower.contains("audit")
                || fname_lower.contains("benchmark")
                || fname_lower.contains("analysis")
                || fname_lower.contains("competitive")
                || fname_lower.contains("comprehensive")
                || fname_lower.contains("feasibility")
                || fname_lower.contains("report")
                || fname_lower.contains("session");

            if is_report {
                repo_reports.push(doc["path"].as_str().unwrap_or(fname).to_string());
            } else if tier == "doc-repo-supplementary" || tier == "unrecognized" {
                repo_incorporable.push(doc["path"].as_str().unwrap_or(fname).to_string());
            }
        }

        if !repo_reports.is_empty() {
            suggested_actions.push(json!({
                "action": "move_reports_to_doc_repo",
                "description": format!(
                    "Project repo has {} analysis/report file(s) that belong in alcove reports/: {}",
                    repo_reports.len(),
                    repo_reports.join(", ")
                ),
                "note": "Analysis, benchmark, and audit documents are internal reference material. Move to alcove reports/ folder, not the public project repo."
            }));
        }

        if !repo_incorporable.is_empty() {
            suggested_actions.push(json!({
                "action": "incorporate_to_doc_repo",
                "description": format!(
                    "Project repo has {} file(s) that could be restructured into alcove: {}",
                    repo_incorporable.len(),
                    repo_incorporable.join(", ")
                ),
                "note": "These project repo files contain reference material that could be restructured and incorporated into alcove internal docs."
            }));
        }
    }

    if suggested_actions.is_empty() {
        suggested_actions.push(json!({
            "action": "none",
            "description": "All docs present and well-organized. No action needed."
        }));
    }

    let mut result = json!({
        "project_name": project_name,
        "doc_repo": {
            "path": project_root.to_string_lossy(),
            "required": tier1_status,
            "supplementary": supplementary_files,
            "reference": "reports/",
            "unrecognized": unrecognized_files,
        },
        "summary": {
            "doc_repo_required_total": core_files.len(),
            "doc_repo_required_populated": populated,
            "doc_repo_required_missing": missing_files.len(),
            "doc_repo_required_unfilled": unfilled_files.len(),
            "doc_repo_required_minimal": minimal_files.len(),
            "doc_repo_supplementary_count": supplementary_files.len(),
            "unrecognized_count": unrecognized_files.len(),
            "repo_docs_count": repo_docs.len(),
        },
        "diagram_format": load_config().diagram_format(),
        "suggested_actions": suggested_actions,
        "agent_instruction": concat!(
            "Present the audit findings to the user. Ask which actions to proceed with. ",
            "RULES: ",
            "1) alcove → project repo: Generate public docs (README, etc.) DERIVED from internal content. NEVER copy internal docs as-is. ",
            "2) project repo → alcove: Restructure/incorporate reference materials into internal docs. ",
            "3) NEVER expose raw internal docs (PRD, ARCHITECTURE, SECRETS_MAP, etc.) to the project repo. ",
            "4) Analysis/report/benchmark/audit files belong in alcove reports/, NOT in the project repo. ",
            "5) Before removing a file from project repo that also exists in alcove, DIFF the two versions. ",
            "   If the project repo version has additional content, MERGE it into alcove first, then remove from project repo. ",
            "6) NEVER move or delete files without explicit user confirmation."
        ),
    });

    // Add project_repo section if we detected the repo path
    if repo_path.is_some() {
        result["project_repo"] = json!({
            "path": repo_path_str,
            "docs": repo_docs,
            "scanned": ["root-level docs", "docs/ folder"],
        });
    } else {
        result["project_repo"] = json!({
            "status": "not_detected",
            "note": "Could not detect project repo path from CWD. Run from within the project directory for full audit.",
        });
    }

    Ok(result)
}

/// Categorization hint for unrecognized files in alcove.
/// Suggests which internal doc this file's content relates to.
fn suggest_categorization(filename: &str) -> &'static str {
    let lower = filename.to_lowercase();

    if lower.contains("product") || lower.contains("requirement")
        || lower.contains("spec") || lower.contains("summary") {
        return "Related to PRD.md";
    }
    if lower.contains("design") || lower.contains("orchestration")
        || lower.contains("implementation") {
        return "Related to ARCHITECTURE.md";
    }
    if lower.contains("plan") || lower.contains("roadmap") || lower.contains("todo") {
        return "Related to PROGRESS.md";
    }
    if lower.contains("feasibility") || lower.contains("adr") || lower.contains("decision") {
        return "Related to DECISIONS.md";
    }
    if lower.contains("coding_standard") || lower.contains("code_style") {
        return "Related to CONVENTIONS.md";
    }
    if lower.contains("tech_debt") || lower.contains("technical_debt") {
        return "Related to DEBT.md";
    }
    if lower.contains("env_var") || lower.contains("secrets") {
        return "Related to SECRETS_MAP.md";
    }
    if lower.contains("audit") || lower.contains("benchmark")
        || lower.contains("analysis") || lower.contains("competitive")
        || lower.contains("comprehensive") || lower.contains("session")
        || lower.contains("report") {
        return "Candidate for reports/ folder";
    }

    "Uncategorized — ask user"
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if a file is a documentation file.
/// Allows .md, .txt, .rst unconditionally.
/// For .yml/.yaml/.json, only allows known doc spec files (e.g. OPENAPI).
fn is_doc_file(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "md" | "txt" | "rst" => true,
        "yml" | "yaml" | "json" => {
            let filename = path
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("")
                .to_lowercase();
            filename.starts_with("openapi") || filename.starts_with("swagger")
        }
        _ => false,
    }
}

fn slice_content(content: &str, offset: Option<usize>, limit: Option<usize>) -> String {
    let start = offset.unwrap_or(0);
    let chars: Vec<char> = content.chars().collect();
    let total = chars.len();

    if start >= total {
        return String::new();
    }

    let end = limit
        .map(|l| (start + l).min(total))
        .unwrap_or(total);

    chars[start..end].iter().collect()
}
