use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::{is_blocked_system_path, is_reserved_dir_name, load_config};
use crate::telemetry::{FailureClass, ResultSizeBucket, Telemetry, Tool};
use crate::tools;

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct RpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl RpcResponse {
    pub fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Option<Value>, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(RpcError { code, message }),
        }
    }
}

// ---------------------------------------------------------------------------
// MCP tool description
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ToolDescription {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub fn dispatch(req: RpcRequest) -> Option<RpcResponse> {
    match req.method.as_str() {
        "initialize" => Some(handle_initialize(req.id)),
        "notifications/initialized" | "initialized" => None,
        "tools/list" => Some(handle_tools_list(req.id)),
        "tools/call" => Some(handle_tool_call(req.id, req.params)),
        _ => Some(RpcResponse::err(
            req.id,
            -32601,
            format!("Method not found: {}", req.method),
        )),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

fn handle_initialize(id: Option<Value>) -> RpcResponse {
    RpcResponse::ok(
        id,
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": { "listChanged": false }
            },
            "serverInfo": {
                "name": "alcove",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )
}

fn handle_tools_list(id: Option<Value>) -> RpcResponse {
    let tools: Vec<ToolDescription> = vec![
        ToolDescription {
            name: "get_project_docs_overview".into(),
            description: concat!(
                "List all documentation files for the current project with file sizes and classification labels.\n",
                "\n",
                "Call this tool first when the user asks what docs exist, wants a summary of project documentation, ",
                "or before deciding which files to read. It is read-only and has no side effects.\n",
                "\n",
                "Scans two locations: the alcove doc-repo (private/internal docs) and the project repository root + docs/ (public-facing docs).\n",
                "\n",
                "Classification labels:\n",
                "- doc-repo-required: core internal docs required by policy (e.g. PRD, ARCHITECTURE)\n",
                "- doc-repo-supplementary: optional internal extras\n",
                "- project-repo: public-facing docs in the project repo (e.g. README, CHANGELOG)\n",
                "- reference: reports and reference materials\n",
                "- unrecognized: files not matching any known category\n",
                "\n",
                "Returns an empty list if no docs exist yet. Use init_project to create initial docs."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDescription {
            name: "search_project_docs".into(),
            description: concat!(
                "Search documentation files for a keyword or phrase. ",
                "Automatically uses BM25 ranked search when index is available, ",
                "falls back to grep (substring match) otherwise.\n",
                "\n",
                "scope=\"project\" (default): current project only, based on CWD.\n",
                "scope=\"global\": search across ALL projects in the doc repository.\n",
                "\n",
                "Use global scope when the user:\n",
                "- does not specify a project, or says 'all projects', 'everywhere', 'across projects'\n",
                "- references previously saved notes, knowledge, or past decisions\n",
                "- wants to compare how different projects handle the same topic\n",
                "- uses words like 'find everywhere', 'search everything', 'all docs'\n",
                "- asks in Korean: '전체', '모든 프로젝트', '다른 프로젝트에서는'\n",
                "\n",
                "Use project scope (default) when the user asks about the current project context."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["project", "global"],
                        "description": "Search scope: 'project' (default, current project only) or 'global' (all projects). Omit or set to 'project' for current project."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max results (default: 20)"
                    },
                    "mode": {
                        "type": "string",
                        "enum": ["grep"],
                        "description": "Override search mode. Options: \"grep\" (regex-only search, skips BM25 index). Omit for default hybrid search."
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDescription {
            name: "get_doc_file".into(),
            description: concat!(
                "Read the full content of a specific documentation file by its relative path.\n",
                "\n",
                "Use this tool when you know the exact file to read — typically after get_project_docs_overview ",
                "or search_project_docs has identified the relevant file. It is read-only and has no side effects.\n",
                "\n",
                "For large files, use offset and limit to read in chunks and avoid exceeding context limits. ",
                "offset is a character (not line) position. Omit both to read the entire file.\n",
                "\n",
                "Returns an error if the file does not exist or the path is outside the doc root."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "relative_path": {
                        "type": "string",
                        "description": "Path relative to the project doc root (e.g. \"PRD.md\" or \"reports/weekly.md\")"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Character offset to start reading from (default: 0). Use for paginating large files."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max characters to return (default: entire file). Use together with offset to read in chunks."
                    }
                },
                "required": ["relative_path"]
            }),
        },
        ToolDescription {
            name: "list_projects".into(),
            description: concat!(
                "List all projects that have documentation stored in the alcove doc-repo.\n",
                "\n",
                "Use this tool when:\n",
                "- The user asks which projects are available or tracked in alcove\n",
                "- You need to verify a project exists before calling get_project_docs_overview or search_project_docs\n",
                "- The user wants to switch project context or compare projects\n",
                "- Before using scope=\"global\" in search_project_docs to understand what will be searched\n",
                "\n",
                "It is read-only and has no side effects. Does not require any parameters.\n",
                "\n",
                "Returns an array of project names derived from subdirectory names in the alcove doc-repo. ",
                "Returns an empty array if no projects have been initialized yet — use init_project to create one. ",
                "Project names are case-sensitive and match the directory names exactly."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDescription {
            name: "audit_project".into(),
            description: concat!(
                "Audit documentation health across both the alcove doc-repo (private/internal) and the project repository (public-facing).\n",
                "\n",
                "Use this tool when the user wants to know what docs are missing, outdated, or misplaced — ",
                "for example: 'audit my docs', 'what docs am I missing?', 'check my documentation health'.\n",
                "\n",
                "Scans two locations:\n",
                "1. alcove doc-repo: checks for missing required internal docs\n",
                "2. project repo root + docs/: checks for missing public-facing docs\n",
                "\n",
                "Suggests actions such as generating missing public docs from internal content, or incorporating ",
                "project repo materials into alcove. NEVER suggests exposing raw internal docs to the project repo.\n",
                "\n",
                "IMPORTANT: This tool only reports findings. Always present the results to the user and ask ",
                "which actions to proceed with before calling init_project or configure_project."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDescription {
            name: "configure_project".into(),
            description: concat!(
                "Create or update per-project settings in alcove.toml. ",
                "Each project can override global defaults for: diagram format, ",
                "required core docs, team docs, and public docs. ",
                "Only the fields you specify are changed; unmentioned settings are preserved. ",
                "Run init_project first if the project does not yet exist."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "project_name": {
                        "type": "string",
                        "description": "Name of the project to configure"
                    },
                    "diagram_format": {
                        "type": "string",
                        "description": "Diagram syntax to use in this project's docs (e.g. \"mermaid\", \"plantuml\")"
                    },
                    "core_files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Required internal docs for this project (overrides global core list)"
                    },
                    "team_files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Supplementary team docs recognized for this project"
                    },
                    "public_files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Public-facing docs recognized for this project"
                    }
                },
                "required": ["project_name"]
            }),
        },
        ToolDescription {
            name: "init_project".into(),
            description: concat!(
                "Initialize documentation for a new project from alcove templates. ",
                "Creates internal docs (PRD, Architecture, etc.) in the alcove doc-repo. ",
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
                        "description": "Name of the project to initialize docs for"
                    },
                    "project_path": {
                        "type": "string",
                        "description": "Absolute path to the project repository (for creating external docs like README)"
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
                "required": ["project_name"]
            }),
        },
        ToolDescription {
            name: "validate_docs".into(),
            description: concat!(
                "Validate the current project's documentation against the team policy defined in policy.toml.\n",
                "\n",
                "Use this tool when the user asks to check doc quality, run a policy check, or verify docs before a release. ",
                "It is read-only and does not modify any files.\n",
                "\n",
                "Checks performed:\n",
                "- Required files exist\n",
                "- Template placeholders (e.g. TODO, FIXME) have been filled in\n",
                "- Required section headings are present\n",
                "- Lists meet minimum item counts defined in policy\n",
                "\n",
                "Returns a pass/warn/fail status per file with specific details about each violation. ",
                "If no policy.toml exists, returns a message indicating policy is not configured. ",
                "Use configure_project or init_project to set up policy."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDescription {
            name: "rebuild_index".into(),
            description: concat!(
                "Trigger an incremental index update in the background. ",
                "Returns immediately — the agent is not blocked while indexing runs. ",
                "Run this after adding or updating documents. ",
                "Search results will reflect the new documents once indexing completes."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDescription {
            name: "check_doc_changes".into(),
            description: concat!(
                "Check which documentation files have been added, modified, or deleted since the last index build.\n",
                "\n",
                "Use this tool before search_project_docs when you want to ensure the index is up to date, ",
                "or when the user asks whether docs have changed recently. It is safe to call at any time — ",
                "without auto_rebuild it is read-only and has no side effects.\n",
                "\n",
                "Compares current file timestamps against the stored index metadata. ",
                "Returns a list of changed files grouped by status: added, modified, deleted.\n",
                "\n",
                "Set auto_rebuild=true to automatically trigger rebuild_index if any changes are detected, ",
                "avoiding a separate tool call. If no index exists yet, reports all files as new."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "auto_rebuild": {
                        "type": "boolean",
                        "description": "Automatically rebuild the index if changes are detected (default: false)"
                    }
                },
                "required": []
            }),
        },
        ToolDescription {
            name: "lint_project".into(),
            description: concat!(
                "Lint project documentation for semantic issues: broken links, orphaned files, ",
                "stale markers (WIP/TODO/FIXME/DRAFT/DEPRECATED), and stale year references.\n",
                "\n",
                "Use this tool when the user asks to check doc quality beyond policy compliance, ",
                "find broken internal links, locate TODO/WIP content, or audit doc hygiene.\n",
                "\n",
                "Checks:\n",
                "- broken-link (warning): wikilinks [[target]] or markdown links [text](path) that resolve to no file\n",
                "- orphan (info): files not linked from any other document (index/readme/moc excluded)\n",
                "- stale-marker (warning): files containing WIP, TODO, FIXME, DRAFT, DEPRECATED, DO NOT USE, OUTDATED\n",
                "- stale-date (info): files mentioning a year that is 2+ years in the past\n",
                "\n",
                "Optionally filter by project name. If omitted, scans all projects."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "project": {
                        "type": "string",
                        "description": "Project name to lint (omit for all projects)"
                    }
                },
                "required": []
            }),
        },
        ToolDescription {
            name: "search_vault".into(),
            description: "Search knowledge base vaults for a query. Use this to find information in research notes, reference materials, and curated knowledge bases — separate from project documentation.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    },
                    "vault": {
                        "type": "string",
                        "description": "Vault name to search. Omit or use '*' to search all vaults."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max results (default: 20)"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDescription {
            name: "list_vaults".into(),
            description: "List all knowledge base vaults with their document counts.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDescription {
            name: "promote_document".into(),
            description: concat!(
                "Promote a document from an external vault (e.g. Obsidian) into the alcove doc-repo.\n",
                "\n",
                "Use this tool when the user wants to import, migrate, or copy a file from ",
                "outside alcove into the appropriate project directory.\n",
                "\n",
                "If 'project' is not specified, the target project is auto-detected by matching ",
                "the file name and content keywords against known project directory names. ",
                "Falls back to the 'inbox/' directory if no match is found.\n",
                "\n",
                "By default, the file is copied (safe). Set copy=false to move it instead."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "Absolute path to the source file to promote"
                    },
                    "project": {
                        "type": "string",
                        "description": "Target project name (auto-detected if omitted)"
                    },
                    "copy": {
                        "type": "boolean",
                        "description": "Copy the file (true, default) or move it (false)"
                    }
                },
                "required": ["source"]
            }),
        },
    ];

    RpcResponse::ok(id, json!({ "tools": tools }))
}

#[derive(Debug, Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

fn tool_enum(name: &str) -> Option<Tool> {
    match name {
        "audit_project"           => Some(Tool::AuditProject),
        "check_doc_changes"       => Some(Tool::CheckDocChanges),
        "configure_project"       => Some(Tool::ConfigureProject),
        "get_doc_file"            => Some(Tool::GetDocFile),
        "get_project_docs_overview" => Some(Tool::GetProjectDocsOverview),
        "init_project"            => Some(Tool::InitProject),
        "lint_project"            => Some(Tool::LintProject),
        "list_projects"           => Some(Tool::ListProjects),
        "list_vaults"             => Some(Tool::ListVaults),
        "promote_document"        => Some(Tool::PromoteDocument),
        "rebuild_index"           => Some(Tool::RebuildIndex),
        "search_project_docs"     => Some(Tool::SearchProjectDocs),
        "search_vault"            => Some(Tool::SearchVault),
        "validate_docs"           => Some(Tool::ValidateDocs),
        _                         => None,
    }
}

fn result_size(v: &Value) -> ResultSizeBucket {
    let n = ["matches", "projects", "issues", "suggested_actions"]
        .iter()
        .find_map(|key| v.get(key).and_then(|m| m.as_array()).map(|a| a.len()))
        .or_else(|| v.as_array().map(|a| a.len()))
        .unwrap_or(1);
    ResultSizeBucket::from_count(n)
}

fn handle_tool_call(id: Option<Value>, params: Value) -> RpcResponse {
    let call: ToolCallParams = match serde_json::from_value(params) {
        Ok(c) => c,
        Err(e) => return RpcResponse::err(id, -32602, format!("Invalid tool call params: {e}")),
    };

    let tel = Telemetry::init();
    let tool_variant = tool_enum(&call.name);
    let t0 = Instant::now();
    if let Some(t) = tool_variant {
        tel.track_tool_called(t);
    }

    // Wrap result emission to fire completion/failure events before returning.
    macro_rules! ok {
        ($v:expr) => {{
            let v: Value = $v;
            if let Some(t) = tool_variant {
                tel.track_tool_completed(t, t0.elapsed().as_millis() as u64, result_size(&v));
            }
            RpcResponse::ok(id.clone(), mcp_text_result(&v))
        }};
    }
    macro_rules! err {
        ($code:expr, $msg:expr) => {{
            if let Some(t) = tool_variant {
                tel.track_tool_failed(t, FailureClass::Unknown);
            }
            RpcResponse::err(id.clone(), $code, $msg)
        }};
    }

    let docs_root = match std::env::var("DOCS_ROOT") {
        Ok(v) => {
            let path = PathBuf::from(&v);
            if is_blocked_system_path(&path) {
                return err!(-32000, "DOCS_ROOT points to a restricted system directory.".into());
            }
            path
        }
        Err(_) => match load_config().docs_root() {
            Some(p) if p.is_dir() => p,
            _ => {
                return err!(-32000, "DOCS_ROOT environment variable is not set and config.toml has no docs_root.".into());
            }
        },
    };

    // lint_project and promote_document operate on docs_root directly
    if call.name == "lint_project" {
        return match tools::tool_lint_project(&docs_root, call.arguments) {
            Ok(v) => ok!(v),
            Err(e) => err!(-32002, format!("Tool `{}` failed: {e}", call.name)),
        };
    }
    if call.name == "promote_document" {
        return match tools::tool_promote_document(&docs_root, call.arguments) {
            Ok(v) => ok!(v),
            Err(e) => err!(-32002, format!("Tool `{}` failed: {e}", call.name)),
        };
    }

    // list_vaults and search_vault operate on vault storage, not project docs
    if call.name == "list_vaults" {
        return match crate::vault::list_vaults() {
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
                ok!(json!(arr))
            }
            Err(e) => err!(-32002, format!("Tool `{}` failed: {e}", call.name)),
        };
    }
    if call.name == "search_vault" {
        let query = call
            .arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if query.is_empty() {
            return err!(-32602, "Query must not be empty".to_string());
        }
        if query.len() > 8192 {
            return err!(-32002, "Query too long (max 8192 bytes)".to_string());
        }
        let vault_name = call
            .arguments
            .get("vault")
            .and_then(|v| v.as_str())
            .unwrap_or("*");
        let limit = call
            .arguments
            .get("limit")
            .and_then(Value::as_u64)
            .map(|v| usize::try_from(v).unwrap_or(usize::MAX).clamp(1, 200))
            .unwrap_or(20);

        let search_all = vault_name == "*" || vault_name.is_empty();

        if search_all {
            let vaults = match crate::vault::list_vaults() {
                Ok(v) => v,
                Err(e) => return err!(-32002, format!("Tool `{}` failed: {e}", call.name)),
            };
            use rayon::prelude::*;
            let vault_results: Vec<_> = vaults
                .par_iter()
                .filter_map(|vault| crate::index::search_vault(&vault.path, query, limit).ok())
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
            return ok!(json!({ "query": query, "scope": "all_vaults", "matches": all_matches }));
        } else {
            {
                use std::path::Component;
                let p = std::path::Path::new(vault_name);
                let components: Vec<_> = p.components().collect();
                if components.len() != 1 || !matches!(components[0], Component::Normal(_)) {
                    return err!(-32002, format!("Invalid vault name: '{vault_name}'"));
                }
            }
            let vault_path = crate::vault::vaults_root().join(vault_name);
            if !vault_path.is_dir() {
                return err!(-32002, format!("Vault '{}' does not exist", vault_name));
            }
            return match crate::index::search_vault(&vault_path, query, limit) {
                Ok(v) => ok!(v),
                Err(e) => err!(-32002, format!("Tool `{}` failed: {e}", call.name)),
            };
        }
    }

    // list_projects and init_project don't need a resolved project
    if call.name == "list_projects" {
        return match tools::tool_list_projects(&docs_root) {
            Ok(v) => ok!(v),
            Err(e) => err!(-32002, format!("Tool `{}` failed: {e}", call.name)),
        };
    }
    if call.name == "init_project" {
        return match tools::tool_init_project(&docs_root, call.arguments) {
            Ok(v) => {
                let _ = crate::index::build_index(&docs_root);
                ok!(v)
            }
            Err(e) => err!(-32002, format!("Tool `{}` failed: {e}", call.name)),
        };
    }
    if call.name == "rebuild_index" {
        let docs_root_clone = docs_root.clone();
        std::thread::spawn(move || {
            let _ = crate::index::build_index(&docs_root_clone);
        });
        return ok!(json!({
            "status": "started",
            "message": "Index build started in background. Search will use the updated index once complete."
        }));
    }
    if call.name == "check_doc_changes" {
        return match tools::tool_check_doc_changes(&docs_root, call.arguments) {
            Ok(v) => ok!(v),
            Err(e) => err!(-32002, format!("Tool `{}` failed: {e}", call.name)),
        };
    }

    // Search: auto mode selection — ranked (BM25) if index available, grep fallback
    if call.name == "search_project_docs" {
        let scope = call
            .arguments
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("project");
        let mode_override = call.arguments.get("mode").and_then(|v| v.as_str());

        let is_global = scope == "global";
        let limit = call
            .arguments
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map(|v| usize::try_from(v).unwrap_or(usize::MAX).clamp(1, 200))
            .unwrap_or(20);
        let query = call
            .arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let force_grep = mode_override == Some("grep");

        if !force_grep {
            let index_dir = docs_root.join(".alcove").join("index");
            if index_dir.exists() || crate::index::ensure_index_fresh(&docs_root) {
                let project_filter = if is_global {
                    None
                } else {
                    tools::resolve_project(&docs_root).map(|r| r.name)
                };
                if let Ok(v) = crate::index::search_indexed(
                    &docs_root,
                    query,
                    limit,
                    project_filter.as_deref(),
                ) {
                    let matches = v["matches"].as_array();
                    if matches.is_some_and(|m| !m.is_empty()) {
                        return ok!(v);
                    }
                }
            }
        }

        if is_global {
            return match tools::tool_search_global(&docs_root, call.arguments) {
                Ok(v) => ok!(v),
                Err(e) => err!(-32002, format!("Tool `{}` failed: {e}", call.name)),
            };
        }
    }

    // All other tools require a resolved project
    let resolved = match tools::resolve_project(&docs_root) {
        Some(r) => r,
        None => {
            let available: Vec<String> = std::fs::read_dir(&docs_root)
                .ok()
                .map(|rd| {
                    rd.filter_map(std::result::Result::ok)
                        .filter(|e| e.path().is_dir())
                        .filter_map(|e| {
                            let name = e.file_name().to_string_lossy().to_string();
                            if is_reserved_dir_name(&name) {
                                None
                            } else {
                                Some(name)
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            return err!(
                -32001,
                format!(
                    "Could not detect project. CWD does not match any project in DOCS_ROOT. \
                     Available projects: [{}]. \
                     Set MCP_PROJECT_NAME env var or run from within a project directory.",
                    available.join(", ")
                )
            );
        }
    };

    let project_root = docs_root.join(&resolved.name);
    let repo_path = resolved.repo_path.as_deref();

    let result = match call.name.as_str() {
        "get_project_docs_overview" => tools::tool_overview(
            &project_root,
            &resolved.name,
            resolved.detected_via,
            repo_path,
        ),
        "search_project_docs" => tools::tool_search(&project_root, call.arguments, repo_path),
        "get_doc_file" => tools::tool_get_file(&project_root, call.arguments),
        "audit_project" => tools::tool_audit(&project_root, &resolved.name, repo_path),
        "configure_project" => {
            let rp = repo_path
                .map(|p| p.to_path_buf())
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| project_root.to_path_buf());
            tools::tool_configure_project(&rp, call.arguments)
        }
        "validate_docs" => {
            let source = crate::policy::policy_source(&docs_root, &resolved.name);
            let (pol, results) = crate::policy::validate(&docs_root, &resolved.name, repo_path);
            Ok(crate::policy::validation_to_json(&pol, &results, source))
        }
        other => Err(anyhow::anyhow!("Unknown tool: {other}")),
    };

    match result {
        Ok(v) => ok!(v),
        Err(e) => err!(-32002, format!("Tool `{}` failed: {e}", call.name)),
    }
}

/// Wrap a JSON value as MCP text content.
pub fn mcp_text_result(value: &Value) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(value).unwrap_or_default()
        }]
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn rpc_ok_response() {
        let resp = RpcResponse::ok(Some(json!(1)), json!({"status": "ok"}));
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(json!(1)));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn rpc_err_response() {
        let resp = RpcResponse::err(Some(json!(2)), -32600, "Invalid".into());
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "Invalid");
    }

    #[test]
    fn mcp_text_result_wraps_json() {
        let val = json!({"key": "value"});
        let result = mcp_text_result(&val);
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        let text = content[0]["text"].as_str().unwrap();
        assert!(text.contains("\"key\""));
        assert!(text.contains("\"value\""));
    }

    fn make_req(method: &str, params: Value) -> RpcRequest {
        RpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(json!(1)),
            method: method.into(),
            params,
        }
    }

    #[test]
    fn dispatch_initialize() {
        let resp = dispatch(make_req("initialize", json!({}))).unwrap();
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "alcove");
    }

    #[test]
    fn dispatch_initialized_notification() {
        let resp = dispatch(make_req("notifications/initialized", json!({})));
        assert!(
            resp.is_none(),
            "notifications should not produce a response"
        );
    }

    #[test]
    fn dispatch_tools_list() {
        let resp = dispatch(make_req("tools/list", json!({}))).unwrap();
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"get_project_docs_overview"));
        assert!(names.contains(&"search_project_docs"));
        assert!(names.contains(&"get_doc_file"));
        assert!(names.contains(&"list_projects"));
        assert!(names.contains(&"audit_project"));
        assert!(names.contains(&"init_project"));
        assert!(names.contains(&"configure_project"));
        assert!(names.contains(&"rebuild_index"));
        assert!(names.contains(&"check_doc_changes"));
        assert!(names.contains(&"search_vault"));
        assert!(names.contains(&"list_vaults"));
    }

    #[test]
    fn dispatch_tools_list_has_schemas() {
        let resp = dispatch(make_req("tools/list", json!({}))).unwrap();
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        for tool in &tools {
            assert!(
                tool["inputSchema"].is_object(),
                "tool {} missing schema",
                tool["name"]
            );
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn dispatch_unknown_method() {
        let resp = dispatch(make_req("nonexistent/method", json!({}))).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[test]
    fn dispatch_tool_call_invalid_params() {
        let resp = dispatch(make_req("tools/call", json!("not an object"))).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[test]
    fn rpc_response_serialization() {
        let resp = RpcResponse::ok(Some(json!(42)), json!({"done": true}));
        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(serialized.contains("\"jsonrpc\":\"2.0\""));
        assert!(serialized.contains("\"id\":42"));
        assert!(!serialized.contains("\"error\""));
    }

    #[test]
    fn rpc_err_omits_result() {
        let resp = RpcResponse::err(None, -32000, "fail".into());
        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(!serialized.contains("\"result\""));
        assert!(!serialized.contains("\"id\""));
        assert!(serialized.contains("\"error\""));
    }

    // -----------------------------------------------------------------------
    // Additional edge-case tests
    // -----------------------------------------------------------------------

    #[test]
    fn dispatch_initialized_without_notifications_prefix() {
        // "initialized" (without "notifications/" prefix) should also return None
        let resp = dispatch(make_req("initialized", json!({})));
        assert!(
            resp.is_none(),
            "bare 'initialized' should not produce a response"
        );
    }

    #[test]
    #[serial]
    fn dispatch_tool_call_unknown_tool_with_docs_root() {
        // Unknown tools (other than list_projects / init_project) require
        // project resolution first. With an empty DOCS_ROOT, resolution fails
        // with -32001 before reaching the unknown-tool branch.
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: test is single-threaded; no other thread reads DOCS_ROOT concurrently.
        unsafe { std::env::set_var("DOCS_ROOT", tmp.path().as_os_str()) };

        let req = make_req(
            "tools/call",
            json!({"name": "totally_nonexistent_tool", "arguments": {}}),
        );
        let resp = dispatch(req).unwrap();

        // SAFETY: test is single-threaded; restoring env to previous state.
        unsafe { std::env::remove_var("DOCS_ROOT") };

        assert!(resp.error.is_some(), "unknown tool should produce an error");
        let err = resp.error.unwrap();
        // Project resolution fails before the unknown tool check
        assert_eq!(err.code, -32001);
        assert!(
            err.message.contains("Could not detect project"),
            "should get project resolution error, got: {}",
            err.message,
        );
    }

    #[test]
    fn handle_tools_list_contains_validate_docs() {
        let resp = handle_tools_list(Some(json!(1)));
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();

        let validate = tools.iter().find(|t| t["name"] == "validate_docs");
        assert!(validate.is_some(), "validate_docs tool must be present");

        let validate = validate.unwrap();
        let schema = &validate["inputSchema"];
        assert_eq!(schema["type"], "object");
        assert!(
            schema["properties"].is_object(),
            "validate_docs schema should have properties object"
        );
        assert!(
            schema["required"].is_array(),
            "validate_docs schema should have required array"
        );
    }

    #[test]
    fn mcp_text_result_with_empty_string() {
        let val = json!("");
        let result = mcp_text_result(&val);
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        // Pretty-printed empty string is just `""`
        let text = content[0]["text"].as_str().unwrap();
        assert_eq!(text, "\"\"");
    }

    #[test]
    fn mcp_text_result_with_null() {
        let val = json!(null);
        let result = mcp_text_result(&val);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert_eq!(text, "null");
    }

    #[test]
    fn mcp_text_result_with_array() {
        let val = json!(["alpha", "beta", "gamma"]);
        let result = mcp_text_result(&val);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("alpha"));
        assert!(text.contains("beta"));
        assert!(text.contains("gamma"));
        // Verify it round-trips back to the same array
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 3);
    }

    #[test]
    fn rpc_request_deserialization_missing_optional_fields() {
        // id and params are optional / have defaults
        let json_str = r#"{"jsonrpc": "2.0", "method": "initialize"}"#;
        let req: RpcRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.method, "initialize");
        assert!(req.id.is_none(), "id should be None when absent");
        assert!(
            req.params.is_null(),
            "params should default to null when absent"
        );
    }

    #[test]
    fn rpc_request_deserialization_with_all_fields() {
        let json_str =
            r#"{"jsonrpc": "2.0", "id": 42, "method": "tools/list", "params": {"foo": "bar"}}"#;
        let req: RpcRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(req.id, Some(json!(42)));
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.params["foo"], "bar");
    }

    #[test]
    fn rpc_response_ok_with_none_id_skips_id_in_json() {
        let resp = RpcResponse::ok(None, json!("hello"));
        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(
            !serialized.contains("\"id\""),
            "id should be skipped when None"
        );
        assert!(serialized.contains("\"result\""));
    }

    #[test]
    fn tool_description_serialization_renames_input_schema() {
        let td = ToolDescription {
            name: "test_tool".into(),
            description: "A test tool".into(),
            input_schema: json!({"type": "object", "properties": {}}),
        };
        let serialized = serde_json::to_value(&td).unwrap();
        // The field must appear as "inputSchema", not "input_schema"
        assert!(
            serialized.get("inputSchema").is_some(),
            "field should be serialized as inputSchema"
        );
        assert!(
            serialized.get("input_schema").is_none(),
            "field should NOT appear as input_schema"
        );
        assert_eq!(serialized["inputSchema"]["type"], "object");
    }

    #[test]
    #[serial]
    fn dispatch_list_projects_with_valid_docs_root() {
        let tmp = tempfile::tempdir().unwrap();
        // Create a fake project directory inside the temp DOCS_ROOT
        std::fs::create_dir(tmp.path().join("my_project")).unwrap();
        // SAFETY: test is single-threaded; no other thread reads DOCS_ROOT concurrently.
        unsafe { std::env::set_var("DOCS_ROOT", tmp.path().as_os_str()) };

        let req = make_req(
            "tools/call",
            json!({"name": "list_projects", "arguments": {}}),
        );
        let resp = dispatch(req).unwrap();

        // SAFETY: test is single-threaded; restoring env to previous state.
        unsafe { std::env::remove_var("DOCS_ROOT") };

        assert!(
            resp.error.is_none(),
            "list_projects should succeed: {:?}",
            resp.error
        );
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("my_project"),
            "list_projects output should contain the created project directory"
        );
    }

    #[test]
    #[serial]
    fn dispatch_rebuild_index() {
        let tmp = tempfile::tempdir().unwrap();
        // Create a project with a doc
        let proj = tmp.path().join("indexproj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("PRD.md"), "# PRD\n\nIndex test content.").unwrap();

        unsafe { std::env::set_var("DOCS_ROOT", tmp.path().as_os_str()) };
        let req = make_req(
            "tools/call",
            json!({"name": "rebuild_index", "arguments": {}}),
        );
        let resp = dispatch(req).unwrap();
        unsafe { std::env::remove_var("DOCS_ROOT") };

        assert!(resp.error.is_none(), "rebuild_index should succeed");
        let text = resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            text.contains("ok") || text.contains("skipped") || text.contains("started"),
            "result should contain status ok, skipped, or started, got: {text}"
        );
    }

    #[test]
    #[serial]
    fn dispatch_search_global_grep() {
        let tmp = tempfile::tempdir().unwrap();
        let p1 = tmp.path().join("alpha");
        std::fs::create_dir_all(&p1).unwrap();
        std::fs::write(p1.join("PRD.md"), "# Alpha PRD\n\nUnique marker xyzzy.").unwrap();
        let p2 = tmp.path().join("beta");
        std::fs::create_dir_all(&p2).unwrap();
        std::fs::write(
            p2.join("ARCH.md"),
            "# Beta Arch\n\nAnother xyzzy reference.",
        )
        .unwrap();

        unsafe { std::env::set_var("DOCS_ROOT", tmp.path().as_os_str()) };
        let req = make_req(
            "tools/call",
            json!({
                "name": "search_project_docs",
                "arguments": {"query": "xyzzy", "scope": "global"}
            }),
        );
        let resp = dispatch(req).unwrap();
        unsafe { std::env::remove_var("DOCS_ROOT") };

        assert!(resp.error.is_none(), "global grep search should succeed");
        let text = resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(text.contains("alpha"), "should find in alpha project");
        assert!(text.contains("beta"), "should find in beta project");
    }

    #[test]
    #[serial]
    fn dispatch_search_ranked_fallback_to_grep() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("falltest");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("DOC.md"), "# Test\n\nFallback marker plugh.").unwrap();

        // No index built — ranked should fallback to global grep
        unsafe { std::env::set_var("DOCS_ROOT", tmp.path().as_os_str()) };
        let req = make_req(
            "tools/call",
            json!({
                "name": "search_project_docs",
                "arguments": {"query": "plugh", "scope": "global", "mode": "ranked"}
            }),
        );
        let resp = dispatch(req).unwrap();
        unsafe { std::env::remove_var("DOCS_ROOT") };

        assert!(
            resp.error.is_none(),
            "ranked search should fallback, not error"
        );
        let text = resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            text.contains("plugh"),
            "fallback grep should find the marker"
        );
    }

    #[test]
    #[serial]
    fn dispatch_search_ranked_with_index() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("ranked");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("NOTES.md"),
            "# Notes\n\nBM25 scoring test document.",
        )
        .unwrap();

        // Build index first (use inner fn to avoid global lock in parallel tests)
        crate::index::build_index_unlocked(tmp.path()).unwrap();

        unsafe { std::env::set_var("DOCS_ROOT", tmp.path().as_os_str()) };
        let req = make_req(
            "tools/call",
            json!({
                "name": "search_project_docs",
                "arguments": {"query": "scoring", "scope": "global", "mode": "ranked"}
            }),
        );
        let resp = dispatch(req).unwrap();
        unsafe { std::env::remove_var("DOCS_ROOT") };

        assert!(
            resp.error.is_none(),
            "ranked search with index should succeed"
        );
        let text = resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(text.contains("ranked"), "should have ranked mode in result");
        assert!(text.contains("score"), "should have score in result");
    }

    #[test]
    fn search_schema_has_scope_and_mode() {
        let resp = handle_tools_list(Some(json!(1)));
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        let search = tools
            .iter()
            .find(|t| t["name"] == "search_project_docs")
            .unwrap();
        let props = &search["inputSchema"]["properties"];
        assert!(props["scope"].is_object(), "scope param should exist");
        assert!(
            props["mode"].is_object(),
            "mode param should be documented in schema"
        );
        // Check scope enum values
        let scope_enum = props["scope"]["enum"].as_array().unwrap();
        assert!(scope_enum.contains(&json!("project")));
        assert!(scope_enum.contains(&json!("global")));
        // Check mode enum value
        let mode_enum = props["mode"]["enum"].as_array().unwrap();
        assert!(mode_enum.contains(&json!("grep")));
    }

    #[test]
    #[serial]
    fn dispatch_search_auto_uses_ranked_when_index_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("autoproj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("DOC.md"), "# Doc\n\nAuto mode test content here.").unwrap();

        // Build index
        crate::index::build_index_unlocked(tmp.path()).unwrap();

        unsafe { std::env::set_var("DOCS_ROOT", tmp.path().as_os_str()) };
        // No mode param — should auto-select ranked
        let req = make_req(
            "tools/call",
            json!({
                "name": "search_project_docs",
                "arguments": {"query": "Auto mode", "scope": "global"}
            }),
        );
        let resp = dispatch(req).unwrap();
        unsafe { std::env::remove_var("DOCS_ROOT") };

        assert!(resp.error.is_none(), "auto search should succeed");
        let text = resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            text.contains("ranked"),
            "auto mode with index should use ranked: {text}"
        );
        assert!(text.contains("score"), "ranked results should have scores");
    }

    #[test]
    #[serial]
    fn dispatch_search_auto_falls_back_to_grep_no_index() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("grepproj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("DOC.md"), "# Doc\n\nFallback grep marker xyzzy.").unwrap();

        // No index built — auto should fallback to grep
        unsafe { std::env::set_var("DOCS_ROOT", tmp.path().as_os_str()) };
        let req = make_req(
            "tools/call",
            json!({
                "name": "search_project_docs",
                "arguments": {"query": "xyzzy", "scope": "global"}
            }),
        );
        let resp = dispatch(req).unwrap();
        unsafe { std::env::remove_var("DOCS_ROOT") };

        assert!(resp.error.is_none(), "grep fallback should succeed");
        let text = resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            text.contains("xyzzy"),
            "grep fallback should find the marker"
        );
    }

    #[test]
    #[serial]
    fn dispatch_search_force_grep_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("forceproj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("DOC.md"), "# Doc\n\nForce grep marker plugh.").unwrap();

        // Build index
        crate::index::build_index_unlocked(tmp.path()).unwrap();

        unsafe { std::env::set_var("DOCS_ROOT", tmp.path().as_os_str()) };
        // Explicitly force grep mode via documented "mode" parameter
        let req = make_req(
            "tools/call",
            json!({
                "name": "search_project_docs",
                "arguments": {"query": "plugh", "scope": "global", "mode": "grep"}
            }),
        );
        let resp = dispatch(req).unwrap();
        unsafe { std::env::remove_var("DOCS_ROOT") };

        assert!(resp.error.is_none(), "forced grep should succeed");
        let text = resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(text.contains("plugh"), "grep should find the marker");
        // Should NOT contain "score" (grep doesn't have scores)
        assert!(
            !text.contains("score"),
            "forced grep should not have scores: {text}"
        );
    }

    #[test]
    #[serial]
    fn dispatch_check_doc_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("changeproj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("PRD.md"), "# PRD\n\nChange detection test.").unwrap();

        unsafe { std::env::set_var("DOCS_ROOT", tmp.path().as_os_str()) };
        let req = make_req(
            "tools/call",
            json!({"name": "check_doc_changes", "arguments": {}}),
        );
        let resp = dispatch(req).unwrap();
        unsafe { std::env::remove_var("DOCS_ROOT") };

        assert!(resp.error.is_none(), "check_doc_changes should succeed");
        let text = resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        // No index exists, so all files are "added"
        assert!(text.contains("added"), "should report added files");
        assert!(text.contains("PRD.md"), "should list PRD.md");
    }

    #[test]
    #[serial]
    fn dispatch_check_doc_changes_with_auto_rebuild() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("rebuildproj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("DOC.md"), "# Doc\n\nAuto rebuild test.").unwrap();

        unsafe { std::env::set_var("DOCS_ROOT", tmp.path().as_os_str()) };
        let req = make_req(
            "tools/call",
            json!({"name": "check_doc_changes", "arguments": {"auto_rebuild": true}}),
        );
        let resp = dispatch(req).unwrap();
        unsafe { std::env::remove_var("DOCS_ROOT") };

        assert!(resp.error.is_none(), "auto_rebuild should succeed");
        let text = resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(text.contains("rebuild"), "should contain rebuild result");
    }

    #[test]
    #[serial]
    fn dispatch_search_vault_returns_results() {
        let tmp = tempfile::tempdir().unwrap();
        // Set ALCOVE_HOME so vaults_root() points to our temp dir
        unsafe { std::env::set_var("ALCOVE_HOME", tmp.path().as_os_str()) };

        let vaults_dir = tmp.path().join("vaults");
        let vault_path = vaults_dir.join("testvault");
        std::fs::create_dir_all(&vault_path).unwrap();
        std::fs::write(
            vault_path.join("notes.md"),
            "# Research Notes\n\nImportant findings about quantum computing.",
        )
        .unwrap();

        // Build a vault index so search_vault can find results
        let _ = crate::index::build_vault_index(&vault_path);

        // Also need DOCS_ROOT set for dispatch to proceed past the config check
        let docs_tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("DOCS_ROOT", docs_tmp.path().as_os_str()) };

        let req = make_req(
            "tools/call",
            json!({
                "name": "search_vault",
                "arguments": {"query": "quantum", "vault": "testvault"}
            }),
        );
        let resp = dispatch(req).unwrap();

        unsafe { std::env::remove_var("DOCS_ROOT") };
        unsafe { std::env::remove_var("ALCOVE_HOME") };

        // The tool should succeed (even if no index exists yet, it returns an error message)
        if let Some(ref err) = resp.error {
            // Acceptable: vault index not built yet
            assert!(
                err.message.contains("index") || err.message.contains("Vault"),
                "unexpected error: {}",
                err.message
            );
        } else {
            let text = resp.result.unwrap()["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string();
            assert!(
                text.contains("quantum") || text.contains("matches"),
                "search_vault should return results or matches key: {text}"
            );
        }
    }

    #[test]
    #[serial]
    fn dispatch_list_vaults() {
        let tmp = tempfile::tempdir().unwrap();
        // Set ALCOVE_HOME so vaults_root() points to our temp dir
        unsafe { std::env::set_var("ALCOVE_HOME", tmp.path().as_os_str()) };

        let vaults_dir = tmp.path().join("vaults");
        std::fs::create_dir_all(&vaults_dir).unwrap();

        // Create two vaults
        let v1 = vaults_dir.join("alpha");
        std::fs::create_dir_all(&v1).unwrap();
        std::fs::write(v1.join("doc.md"), "# Alpha doc").unwrap();

        let v2 = vaults_dir.join("beta");
        std::fs::create_dir_all(&v2).unwrap();
        std::fs::write(v2.join("a.md"), "# A").unwrap();
        std::fs::write(v2.join("b.md"), "# B").unwrap();

        // DOCS_ROOT needed for dispatch
        let docs_tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("DOCS_ROOT", docs_tmp.path().as_os_str()) };

        let req = make_req(
            "tools/call",
            json!({"name": "list_vaults", "arguments": {}}),
        );
        let resp = dispatch(req).unwrap();

        unsafe { std::env::remove_var("DOCS_ROOT") };
        unsafe { std::env::remove_var("ALCOVE_HOME") };

        assert!(
            resp.error.is_none(),
            "list_vaults should succeed: {:?}",
            resp.error
        );
        let text = resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(text.contains("alpha"), "should list alpha vault");
        assert!(text.contains("beta"), "should list beta vault");
        assert!(text.contains("doc_count"), "should contain doc_count field");
    }
}
