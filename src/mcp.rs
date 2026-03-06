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
    pub jsonrpc: String,
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
        Self { jsonrpc: "2.0".into(), id, result: Some(result), error: None }
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
        _ => Some(RpcResponse::err(req.id, -32601, format!("Method not found: {}", req.method))),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

fn handle_initialize(id: Option<Value>) -> RpcResponse {
    RpcResponse::ok(id, json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": { "listChanged": false }
        },
        "serverInfo": {
            "name": "alcove",
            "version": env!("CARGO_PKG_VERSION")
        }
    }))
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
            description: "Search across all documentation files for a keyword or phrase. Returns matching lines with context.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (case-insensitive substring match)"
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
        Err(_) => {
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

    // list_projects and init_project don't need a resolved project
    if call.name == "list_projects" {
        return match tools::tool_list_projects(&docs_root) {
            Ok(v) => RpcResponse::ok(id, mcp_text_result(&v)),
            Err(e) => RpcResponse::err(id, -32002, format!("Tool `{}` failed: {e}", call.name)),
        };
    }
    if call.name == "init_project" {
        return match tools::tool_init_project(&docs_root, call.arguments) {
            Ok(v) => RpcResponse::ok(id, mcp_text_result(&v)),
            Err(e) => RpcResponse::err(id, -32002, format!("Tool `{}` failed: {e}", call.name)),
        };
    }

    // All other tools require a resolved project
    let resolved = match tools::resolve_project(&docs_root) {
        Some(r) => r,
        None => {
            let available: Vec<String> = std::fs::read_dir(&docs_root)
                .ok()
                .map(|rd| {
                    rd.filter_map(|e| e.ok())
                        .filter(|e| e.path().is_dir())
                        .filter_map(|e| {
                            let name = e.file_name().to_string_lossy().to_string();
                            if name.starts_with('.') || name.starts_with('_') { None } else { Some(name) }
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
        "get_project_docs_overview" => tools::tool_overview(&project_root, &resolved.name, resolved.detected_via, repo_path),
        "search_project_docs" => tools::tool_search(&project_root, call.arguments, repo_path),
        "get_doc_file" => tools::tool_get_file(&project_root, call.arguments),
        "audit_project" => tools::tool_audit(&project_root, &resolved.name, repo_path),
        "validate_docs" => {
            let source = crate::policy::policy_source(&docs_root, &resolved.name);
            match crate::policy::validate(&docs_root, &resolved.name, repo_path) {
                Ok((pol, results)) => Ok(crate::policy::validation_to_json(&pol, &results, source)),
                Err(e) => Err(e),
            }
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
        assert!(resp.is_none(), "notifications should not produce a response");
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
    }

    #[test]
    fn dispatch_tools_list_has_schemas() {
        let resp = dispatch(make_req("tools/list", json!({}))).unwrap();
        let tools = resp.result.unwrap()["tools"].as_array().unwrap().clone();
        for tool in &tools {
            assert!(tool["inputSchema"].is_object(), "tool {} missing schema", tool["name"]);
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
}
