use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::load_config;
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
                "List all documentation files for the current project with sizes and classification. ",
                "Scans both alcove (doc-repo) and project repository. ",
                "Classifications: doc-repo-required (core private), doc-repo-supplementary (internal extras), ",
                "project-repo (public-facing), reference (reports/), unrecognized."
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
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDescription {
            name: "get_doc_file".into(),
            description: "Read a specific documentation file by its relative path. Supports offset/limit for large files.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "relative_path": {
                        "type": "string",
                        "description": "Path relative to the project doc root (e.g. \"PRD.md\" or \"reports/weekly.md\")"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Character offset to start reading from (default: 0)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max characters to return (default: all)"
                    }
                },
                "required": ["relative_path"]
            }),
        },
        ToolDescription {
            name: "list_projects".into(),
            description: "List all projects that have documentation in alcove.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
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
                "Validate project docs against team policy (policy.toml). ",
                "Checks: required files exist, template placeholders filled, ",
                "required sections present, minimum list items. ",
                "Returns pass/warn/fail status per file with details."
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
                "Check which documentation files have been added, modified, or deleted ",
                "since the last index build. Compares current file timestamps against ",
                "the index metadata. Optionally triggers an index rebuild if changes are detected."
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
    ];

    RpcResponse::ok(id, json!({ "tools": tools }))
}

#[derive(Debug, Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

fn handle_tool_call(id: Option<Value>, params: Value) -> RpcResponse {
    let call: ToolCallParams = match serde_json::from_value(params) {
        Ok(c) => c,
        Err(e) => return RpcResponse::err(id, -32602, format!("Invalid tool call params: {e}")),
    };

    let docs_root = match std::env::var("DOCS_ROOT") {
        Ok(v) => PathBuf::from(v),
        Err(_) => match load_config().docs_root() {
            Some(p) if p.is_dir() => p,
            _ => {
                return RpcResponse::err(
                    id,
                    -32000,
                    "DOCS_ROOT environment variable is not set and config.toml has no docs_root."
                        .into(),
                );
            }
        },
    };

    // list_projects and init_project don't need a resolved project
    if call.name == "list_projects" {
        return match tools::tool_list_projects(&docs_root) {
            Ok(v) => RpcResponse::ok(id, mcp_text_result(&v)),
            Err(e) => RpcResponse::err(id, -32002, format!("Tool `{}` failed: {e}", call.name)),
        };
    }
    if call.name == "init_project" {
        return match tools::tool_init_project(&docs_root, call.arguments) {
            Ok(v) => {
                // Auto-rebuild index after creating new project docs
                let _ = crate::index::build_index(&docs_root);
                RpcResponse::ok(id, mcp_text_result(&v))
            }
            Err(e) => RpcResponse::err(id, -32002, format!("Tool `{}` failed: {e}", call.name)),
        };
    }
    if call.name == "rebuild_index" {
        // Fire-and-forget: spawn background thread so the agent isn't blocked.
        // The lock mechanism prevents concurrent builds.
        let docs_root_clone = docs_root.clone();
        std::thread::spawn(move || {
            let _ = crate::index::build_index(&docs_root_clone);
        });
        return RpcResponse::ok(id, mcp_text_result(&json!({
            "status": "started",
            "message": "Index build started in background. Search will use the updated index once complete."
        })));
    }
    if call.name == "check_doc_changes" {
        return match tools::tool_check_doc_changes(&docs_root, call.arguments) {
            Ok(v) => RpcResponse::ok(id, mcp_text_result(&v)),
            Err(e) => RpcResponse::err(id, -32002, format!("Tool `{}` failed: {e}", call.name)),
        };
    }

    // Search: auto mode selection — ranked (BM25) if index available, grep fallback
    if call.name == "search_project_docs" {
        let scope = call
            .arguments
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("project");
        // Accept mode as hidden override (not in schema, but still honored if passed)
        let mode_override = call.arguments.get("mode").and_then(|v| v.as_str());

        let is_global = scope == "global";
        let limit = call
            .arguments
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .map(|v| usize::try_from(v).unwrap_or(usize::MAX))
            .unwrap_or(20);
        let query = call
            .arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let force_grep = mode_override == Some("grep");

        // Try ranked search (unless explicitly forced to grep)
        if !force_grep {
            let index_dir = docs_root.join(".alcove").join("index");
            if index_dir.exists() || crate::index::ensure_index_fresh(&docs_root) {
                let project_filter = if is_global {
                    None
                } else {
                    tools::resolve_project(&docs_root).map(|r| r.name)
                };
                match crate::index::search_indexed(
                    &docs_root,
                    query,
                    limit,
                    project_filter.as_deref(),
                ) {
                    Ok(v) => {
                        // If ranked returned results, use them
                        let matches = v["matches"].as_array();
                        if matches.is_some_and(|m| !m.is_empty()) {
                            return RpcResponse::ok(id, mcp_text_result(&v));
                        }
                        // Ranked returned 0 results → fall through to grep
                    }
                    Err(_) => {
                        // Index error → fall through to grep
                    }
                }
            }
        }

        // Grep fallback (or forced grep mode)
        if is_global {
            return match tools::tool_search_global(&docs_root, call.arguments) {
                Ok(v) => RpcResponse::ok(id, mcp_text_result(&v)),
                Err(e) => RpcResponse::err(id, -32002, format!("Tool `{}` failed: {e}", call.name)),
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
                            if name.starts_with('.') || name.starts_with('_') {
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
        Ok(v) => RpcResponse::ok(id, mcp_text_result(&v)),
        Err(e) => RpcResponse::err(id, -32002, format!("Tool `{}` failed: {e}", call.name)),
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
            text.contains("ok") || text.contains("skipped"),
            "result should contain status ok or skipped, got: {text}"
        );
    }

    #[test]
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
    fn search_schema_has_scope_but_no_mode() {
        let resp = handle_tools_list(Some(json!(1)));
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        let search = tools
            .iter()
            .find(|t| t["name"] == "search_project_docs")
            .unwrap();
        let props = &search["inputSchema"]["properties"];
        assert!(props["scope"].is_object(), "scope param should exist");
        assert!(
            !props["mode"].is_object(),
            "mode param should NOT be in schema (auto selection)"
        );
        // Check scope enum values
        let scope_enum = props["scope"]["enum"].as_array().unwrap();
        assert!(scope_enum.contains(&json!("project")));
        assert!(scope_enum.contains(&json!("global")));
    }

    #[test]
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
    fn dispatch_search_force_grep_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("forceproj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("DOC.md"), "# Doc\n\nForce grep marker plugh.").unwrap();

        // Build index
        crate::index::build_index_unlocked(tmp.path()).unwrap();

        unsafe { std::env::set_var("DOCS_ROOT", tmp.path().as_os_str()) };
        // Explicitly force grep mode (hidden param)
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
}
