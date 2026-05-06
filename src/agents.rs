use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

// ---------------------------------------------------------------------------
// Agent definitions
// ---------------------------------------------------------------------------

/// How an agent references environment variables in its MCP config.
pub(crate) enum EnvVarSyntax {
    /// `"${VAR}"` — Claude Code, Claude Desktop, Gemini CLI, Copilot, Antigravity, Codex
    DollarBrace,
    /// `"${env:VAR}"` — Cursor, Cline
    DollarEnvColon,
    /// `"{env:VAR}"` — OpenCode
    BraceEnvColon,
}

impl EnvVarSyntax {
    pub(crate) fn render(&self, var: &str) -> Option<String> {
        match self {
            EnvVarSyntax::DollarBrace     => Some(format!("${{{var}}}")),
            EnvVarSyntax::DollarEnvColon  => Some(format!("${{env:{var}}}")),
            EnvVarSyntax::BraceEnvColon   => Some(format!("{{env:{var}}}")),
        }
    }
}

pub(crate) struct AgentDef {
    pub(crate) name: &'static str,
    pub(crate) mcp_config: McpConfig,
    pub(crate) skill_dir: Option<&'static str>,
    pub(crate) env_syntax: EnvVarSyntax,
}

pub(crate) enum McpConfig {
    /// Standard JSON: { "<key>": { "alcove": { "command": "...", "env": {...} } } }
    /// `omit_type`: set true for agents that do not support the "type" field (e.g. Cline).
    Json {
        path: &'static str,
        server_key: &'static str,
        omit_type: bool,
    },
    /// OpenCode format: { "mcp": { "alcove": { "type": "local", ... } } }
    OpenCode { path: &'static str },
    /// Codex TOML format
    Codex { path: &'static str },
}

pub(crate) fn home() -> PathBuf {
    dirs::home_dir().expect("Cannot determine home directory")
}

pub(crate) fn agents() -> Vec<AgentDef> {
    vec![
        AgentDef {
            name: "Claude Code",
            mcp_config: McpConfig::Json {
                path: "~/.claude.json",
                server_key: "mcpServers",
                omit_type: false,
            },
            skill_dir: Some("~/.claude/skills/alcove"),
            env_syntax: EnvVarSyntax::DollarBrace,
        },
        AgentDef {
            name: "Cursor",
            mcp_config: McpConfig::Json {
                path: "~/.cursor/mcp.json",
                server_key: "mcpServers",
                omit_type: false,
            },
            skill_dir: Some("~/.cursor/skills/alcove"),
            env_syntax: EnvVarSyntax::DollarEnvColon,
        },
        AgentDef {
            name: "Claude Desktop",
            mcp_config: McpConfig::Json {
                path: if cfg!(target_os = "macos") {
                    "~/Library/Application Support/Claude/claude_desktop_config.json"
                } else {
                    "~/.config/claude/claude_desktop_config.json"
                },
                server_key: "mcpServers",
                omit_type: false,
            },
            skill_dir: None,
            env_syntax: EnvVarSyntax::DollarBrace,
        },
        AgentDef {
            name: "Cline (VS Code)",
            mcp_config: McpConfig::Json {
                path: if cfg!(target_os = "macos") {
                    "~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json"
                } else {
                    "~/.config/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json"
                },
                server_key: "mcpServers",
                omit_type: true, // Cline does not support the "type" field
            },
            skill_dir: Some("~/.cline/skills/alcove"),
            env_syntax: EnvVarSyntax::DollarEnvColon, // Cline uses ${env:VAR} interpolation
        },
        AgentDef {
            name: "OpenCode",
            mcp_config: McpConfig::OpenCode {
                path: "~/.config/opencode/opencode.json",
            },
            skill_dir: Some("~/.opencode/skills/alcove"),
            env_syntax: EnvVarSyntax::BraceEnvColon,
        },
        AgentDef {
            name: "Codex CLI",
            mcp_config: McpConfig::Codex {
                path: "~/.codex/config.toml",
            },
            skill_dir: Some("~/.codex/skills/alcove"),
            env_syntax: EnvVarSyntax::DollarBrace, // Codex env section uses ${VAR} interpolation
        },
        AgentDef {
            name: "Copilot CLI",
            mcp_config: McpConfig::Json {
                path: "~/.copilot/mcp-config.json",
                server_key: "mcpServers",
                omit_type: false,
            },
            skill_dir: Some("~/.copilot/skills/alcove"),
            env_syntax: EnvVarSyntax::DollarBrace,
        },
        AgentDef {
            name: "Antigravity",
            mcp_config: McpConfig::Json {
                path: "~/.gemini/antigravity/mcp_config.json",
                server_key: "mcpServers",
                omit_type: false,
            },
            skill_dir: None,
            env_syntax: EnvVarSyntax::DollarBrace,
        },
        AgentDef {
            name: "Gemini CLI",
            mcp_config: McpConfig::Json {
                path: "~/.gemini/settings.json",
                server_key: "mcpServers",
                omit_type: false,
            },
            skill_dir: Some("~/.gemini/skills/alcove"),
            env_syntax: EnvVarSyntax::DollarBrace,
        },
    ]
}

pub(crate) fn expand_path(p: &str) -> PathBuf {
    if let Some(stripped) = p.strip_prefix("~/") {
        home().join(stripped)
    } else {
        PathBuf::from(p)
    }
}

// ---------------------------------------------------------------------------
// Skill file
// ---------------------------------------------------------------------------

const SKILL_CONTENT: &str = include_str!("../skill/SKILL.md");

pub(crate) fn install_skill_to(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join("SKILL.md"), SKILL_CONTENT)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// MCP config writers
// ---------------------------------------------------------------------------

pub(crate) fn write_json_mcp(
    config_path: &Path,
    server_key: &str,
    binary: &Path,
    docs_root: &Path,
    server_url: Option<&str>,
    token_ref: Option<&str>,
    omit_type: bool,
) -> Result<()> {
    let mut config: serde_json::Value = if config_path.exists() {
        let content = fs::read_to_string(config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let server_entry = if let Some(url) = server_url {
        let mut entry = serde_json::json!({ "url": url });
        if !omit_type {
            entry["type"] = serde_json::Value::String("http".to_string());
        }
        if let Some(ref_val) = token_ref {
            entry["headers"] = serde_json::json!({
                "Authorization": format!("Bearer {ref_val}")
            });
        }
        entry
    } else {
        let mut env = serde_json::json!({ "DOCS_ROOT": docs_root.to_string_lossy() });
        if let Some(ref_val) = token_ref {
            env["ALCOVE_TOKEN"] = serde_json::Value::String(ref_val.to_string());
        }
        let mut entry = serde_json::json!({
            "command": binary.to_string_lossy(),
            "args": [],
            "env": env
        });
        if !omit_type {
            entry["type"] = serde_json::Value::String("stdio".to_string());
        }
        entry
    };

    config[server_key]["alcove"] = server_entry;

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(config_path, serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

pub(crate) fn write_opencode_mcp(config_path: &Path, binary: &Path, docs_root: &Path, server_url: Option<&str>, token_ref: Option<&str>) -> Result<()> {
    let mut config: serde_json::Value = if config_path.exists() {
        let content = fs::read_to_string(config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if let Some(url) = server_url {
        let mut entry = serde_json::json!({
            "type": "remote",
            "url": url
        });
        if let Some(ref_val) = token_ref {
            entry["headers"] = serde_json::json!({
                "Authorization": format!("Bearer {ref_val}")
            });
        }
        config["mcp"]["alcove"] = entry;
    } else {
        let mut env = serde_json::json!({ "DOCS_ROOT": docs_root.to_string_lossy() });
        if let Some(ref_val) = token_ref {
            env["ALCOVE_TOKEN"] = serde_json::Value::String(ref_val.to_string());
        }
        config["mcp"]["alcove"] = serde_json::json!({
            "type": "local",
            "command": [binary.to_string_lossy()],
            "environment": env
        });
    }

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(config_path, serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

pub(crate) fn write_codex_mcp(config_path: &Path, binary: &Path, docs_root: &Path, server_url: Option<&str>, token_ref: Option<&str>) -> Result<()> {
    let entry = if let Some(url) = server_url {
        let url_toml = toml::Value::String(url.to_string()).to_string();
        // Codex HTTP: bearer_token_env_var points to the env var holding the actual token
        if token_ref.is_some() {
            format!("\n[mcpServers.alcove]\ntype = \"http\"\nurl = {url_toml}\nbearer_token_env_var = \"ALCOVE_TOKEN\"\n")
        } else {
            format!("\n[mcpServers.alcove]\ntype = \"http\"\nurl = {url_toml}\n")
        }
    } else {
        let binary_toml = toml::Value::String(binary.display().to_string()).to_string();
        let docs_root_toml = toml::Value::String(docs_root.display().to_string()).to_string();
        if let Some(ref_val) = token_ref {
            let token_toml = toml::Value::String(ref_val.to_string()).to_string();
            format!(
                "\n[mcpServers.alcove]\ncommand = {binary_toml}\nargs = []\n\n[mcpServers.alcove.env]\nDOCS_ROOT = {docs_root_toml}\nALCOVE_TOKEN = {token_toml}\n"
            )
        } else {
            format!(
                "\n[mcpServers.alcove]\ncommand = {binary_toml}\nargs = []\n\n[mcpServers.alcove.env]\nDOCS_ROOT = {docs_root_toml}\n"
            )
        }
    };

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    if config_path.exists() {
        let content = fs::read_to_string(config_path)?;
        if content.contains("[mcpServers.alcove]") {
            // Replace existing [mcpServers.alcove] block (and its sub-tables)
            // by removing everything from the header to the next top-level section.
            let mut out = String::new();
            let mut in_alcove = false;
            for line in content.lines() {
                if line.trim_start().starts_with("[mcpServers.alcove") {
                    in_alcove = true;
                    continue;
                }
                if in_alcove {
                    // A new top-level or sibling section ends the alcove block
                    if line.trim_start().starts_with('[') {
                        in_alcove = false;
                    } else {
                        continue;
                    }
                }
                out.push_str(line);
                out.push('\n');
            }
            fs::write(config_path, format!("{out}{entry}"))?;
        } else {
            fs::write(config_path, format!("{content}{entry}"))?;
        }
    } else {
        fs::write(config_path, entry)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Agent registration check (used by cmd_doctor)
// ---------------------------------------------------------------------------

pub(crate) fn check_agent_registration(agent: &AgentDef) -> (&'static str, String) {
    use rust_i18n::t;

    let path = match &agent.mcp_config {
        McpConfig::Json { path, .. } => *path,
        McpConfig::OpenCode { path } => *path,
        McpConfig::Codex { path } => *path,
    };
    let expanded = expand_path(path);

    if !expanded.exists() {
        return ("skip", t!("doctor.agent_config_not_found", path = path).to_string());
    }

    let content = match fs::read_to_string(&expanded) {
        Ok(c) => c,
        Err(_) => return ("error", t!("doctor.agent_cannot_read", path = path).to_string()),
    };

    let has_alcove = match &agent.mcp_config {
        McpConfig::Json { server_key, .. } => {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                parsed
                    .get(*server_key)
                    .and_then(|s| s.get("alcove"))
                    .is_some()
            } else {
                false
            }
        }
        McpConfig::OpenCode { .. } => {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                parsed
                    .get("mcp")
                    .and_then(|m| m.get("alcove"))
                    .is_some()
            } else {
                false
            }
        }
        McpConfig::Codex { .. } => content.contains("[mcpServers.alcove]"),
    };

    if has_alcove {
        ("ok", t!("doctor.agent_registered").to_string())
    } else {
        ("error", t!("doctor.agent_not_registered", path = path).to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── expand_path ──

    #[test]
    fn expand_path_absolute_unchanged() {
        let p = expand_path("/usr/local/bin");
        assert_eq!(p, PathBuf::from("/usr/local/bin"));
    }

    #[test]
    fn expand_path_tilde_expands_to_home() {
        let p = expand_path("~/Documents/test");
        let expected = home().join("Documents/test");
        assert_eq!(p, expected);
    }

    #[test]
    fn expand_path_relative_unchanged() {
        let p = expand_path("relative/path");
        assert_eq!(p, PathBuf::from("relative/path"));
    }

    #[test]
    fn expand_path_tilde_only_no_slash_unchanged() {
        // "~foo" should NOT expand (only "~/" triggers expansion)
        let p = expand_path("~foo");
        assert_eq!(p, PathBuf::from("~foo"));
    }

    // ── agents ──

    #[test]
    fn agents_returns_expected_count() {
        let a = agents();
        assert_eq!(a.len(), 9, "expected 9 agent definitions");
    }

    #[test]
    fn agents_contains_known_names() {
        let a = agents();
        let names: Vec<&str> = a.iter().map(|x| x.name).collect();
        assert!(names.contains(&"Claude Code"));
        assert!(names.contains(&"Cursor"));
        assert!(names.contains(&"Claude Desktop"));
        assert!(names.contains(&"Cline (VS Code)"));
        assert!(names.contains(&"OpenCode"));
        assert!(names.contains(&"Codex CLI"));
        assert!(names.contains(&"Antigravity"));
        assert!(names.contains(&"Gemini CLI"));
    }

    #[test]
    fn agents_all_have_mcp_config() {
        let a = agents();
        for agent in &a {
            match &agent.mcp_config {
                McpConfig::Json { path, server_key, .. } => {
                    assert!(!path.is_empty());
                    assert!(!server_key.is_empty());
                }
                McpConfig::OpenCode { path } => assert!(!path.is_empty()),
                McpConfig::Codex { path } => assert!(!path.is_empty()),
            }
        }
    }

    // ── install_skill_to ──

    #[test]
    fn install_skill_to_creates_skill_file() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let skill_dir = tmp.path().join("skills/alcove");
        let result = install_skill_to(&skill_dir);
        assert!(result.is_ok());

        let skill_path = skill_dir.join("SKILL.md");
        assert!(skill_path.exists());

        let content = fs::read_to_string(&skill_path).expect("failed to read SKILL.md");
        assert_eq!(content, SKILL_CONTENT);
    }

    #[test]
    fn install_skill_to_overwrites_existing() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let skill_dir = tmp.path().join("skills");
        fs::create_dir_all(&skill_dir).expect("failed to create dir");
        fs::write(skill_dir.join("SKILL.md"), "old content").expect("failed to write");

        let result = install_skill_to(&skill_dir);
        assert!(result.is_ok());

        let content = fs::read_to_string(skill_dir.join("SKILL.md")).expect("failed to read");
        assert_eq!(content, SKILL_CONTENT);
    }

    // ── write_json_mcp ──

    #[test]
    fn write_json_mcp_creates_new_file() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("mcp.json");
        let bin = PathBuf::from("/usr/local/bin/alcove");
        let docs = PathBuf::from("/docs/root");

        let result = write_json_mcp(&cfg, "mcpServers", &bin, &docs, None, None, false);
        assert!(result.is_ok());
        assert!(cfg.exists());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("invalid json");

        assert_eq!(parsed["mcpServers"]["alcove"]["type"], "stdio");
        assert_eq!(
            parsed["mcpServers"]["alcove"]["command"],
            "/usr/local/bin/alcove"
        );
        assert_eq!(
            parsed["mcpServers"]["alcove"]["env"]["DOCS_ROOT"],
            "/docs/root"
        );
    }

    #[test]
    fn write_json_mcp_merges_with_existing() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("mcp.json");

        let existing = serde_json::json!({
            "mcpServers": {
                "other": { "command": "other-tool" }
            }
        });
        fs::write(&cfg, serde_json::to_string_pretty(&existing).unwrap()).expect("failed to write");

        let bin = PathBuf::from("/bin/alcove");
        let docs = PathBuf::from("/docs");

        let result = write_json_mcp(&cfg, "mcpServers", &bin, &docs, None, None, false);
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("invalid json");

        // Existing entry preserved
        assert_eq!(parsed["mcpServers"]["other"]["command"], "other-tool");
        // New entry added
        assert_eq!(parsed["mcpServers"]["alcove"]["command"], "/bin/alcove");
    }

    #[test]
    fn write_json_mcp_creates_parent_dirs() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("deep/nested/mcp.json");
        let bin = PathBuf::from("/bin/alcove");
        let docs = PathBuf::from("/docs");

        let result = write_json_mcp(&cfg, "mcpServers", &bin, &docs, None, None, false);
        assert!(result.is_ok());
        assert!(cfg.exists());
    }

    // ── write_json_mcp with HTTP mode ──

    #[test]
    fn write_json_mcp_http_mode() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("mcp.json");
        let bin = PathBuf::from("/usr/local/bin/alcove");
        let docs = PathBuf::from("/docs/root");

        let result = write_json_mcp(&cfg, "mcpServers", &bin, &docs, Some("http://127.0.0.1:57384/mcp"), None, false);
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("invalid json");

        assert_eq!(parsed["mcpServers"]["alcove"]["type"], "http");
        assert_eq!(parsed["mcpServers"]["alcove"]["url"], "http://127.0.0.1:57384/mcp");
        assert!(parsed["mcpServers"]["alcove"]["command"].is_null());
    }

    // ── write_opencode_mcp ──

    #[test]
    fn write_opencode_mcp_creates_new_file() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("opencode.json");
        let bin = PathBuf::from("/bin/alcove");
        let docs = PathBuf::from("/docs");

        let result = write_opencode_mcp(&cfg, &bin, &docs, None, None);
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("invalid json");

        assert_eq!(parsed["mcp"]["alcove"]["type"], "local");
        assert_eq!(parsed["mcp"]["alcove"]["command"][0], "/bin/alcove");
        assert_eq!(parsed["mcp"]["alcove"]["environment"]["DOCS_ROOT"], "/docs");
    }

    #[test]
    fn write_opencode_mcp_merges_existing() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("opencode.json");

        let existing = serde_json::json!({ "mcp": { "other": { "type": "remote" } } });
        fs::write(&cfg, serde_json::to_string(&existing).unwrap()).expect("failed to write");

        let result =
            write_opencode_mcp(&cfg, &PathBuf::from("/bin/alcove"), &PathBuf::from("/docs"), None, None);
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("invalid json");

        assert_eq!(parsed["mcp"]["other"]["type"], "remote");
        assert_eq!(parsed["mcp"]["alcove"]["type"], "local");
    }

    // ── write_codex_mcp ──

    #[test]
    fn write_codex_mcp_creates_new_file() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("config.toml");
        let bin = PathBuf::from("/bin/alcove");
        let docs = PathBuf::from("/docs");

        let result = write_codex_mcp(&cfg, &bin, &docs, None, None);
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        assert!(content.contains("[mcpServers.alcove]"));
        assert!(content.contains(r#"command = "/bin/alcove""#));
        assert!(content.contains(r#"DOCS_ROOT = "/docs""#));
    }

    #[test]
    fn write_codex_mcp_appends_to_existing() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("config.toml");

        fs::write(&cfg, "[some_other_section]\nkey = \"value\"\n").expect("failed to write");

        let result = write_codex_mcp(&cfg, &PathBuf::from("/bin/alcove"), &PathBuf::from("/docs"), None, None);
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        assert!(content.contains("[some_other_section]"));
        assert!(content.contains("[mcpServers.alcove]"));
    }

    #[test]
    fn write_codex_mcp_replaces_existing_alcove_block() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("config.toml");

        let original = "[some_section]\nkey = \"value\"\n\n[mcpServers.alcove]\ncommand = \"/old/bin\"\n\n[mcpServers.alcove.env]\nDOCS_ROOT = \"/old/docs\"\n";
        fs::write(&cfg, original).expect("failed to write");

        let result = write_codex_mcp(&cfg, &PathBuf::from("/new/bin"), &PathBuf::from("/new/docs"), None, None);
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        // Other sections preserved
        assert!(content.contains("[some_section]"), "other sections must be preserved");
        assert!(content.contains("key = \"value\""), "other keys must be preserved");
        // Alcove block updated
        assert!(content.contains("/new/bin"), "new binary must be written");
        assert!(content.contains("/new/docs"), "new docs_root must be written");
        assert!(!content.contains("/old/bin"), "old binary must be removed");
    }

    #[test]
    fn write_codex_mcp_http_mode_with_bearer() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("config.toml");
        let bin = PathBuf::from("/bin/alcove");
        let docs = PathBuf::from("/docs");

        let result = write_codex_mcp(&cfg, &bin, &docs, Some("http://127.0.0.1:57384/mcp"), Some("$ALCOVE_TOKEN"));
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        assert!(content.contains(r#"type = "http""#));
        assert!(content.contains("http://127.0.0.1:57384/mcp"));
        assert!(content.contains(r#"bearer_token_env_var = "ALCOVE_TOKEN""#));
    }

    #[test]
    fn write_codex_mcp_escapes_binary_path() {
        let dir = TempDir::new().expect("temp dir");
        let cfg_path = dir.path().join("codex_config.toml");
        let binary = std::path::PathBuf::from(r#"/usr/local/bin/al"cove"#);
        let docs_root = dir.path().join("docs");

        write_codex_mcp(&cfg_path, &binary, &docs_root, None, None).expect("write should succeed");

        let written = fs::read_to_string(&cfg_path).expect("read back");
        let parsed: toml::Value = toml::from_str(&written).expect("must be valid TOML");
        let got = parsed["mcpServers"]["alcove"]["command"]
            .as_str()
            .expect("command is a string");
        assert_eq!(got, binary.display().to_string());
    }

    #[test]
    fn write_codex_mcp_escapes_server_url() {
        let dir = TempDir::new().expect("temp dir");
        let cfg_path = dir.path().join("codex_config.toml");
        let binary = dir.path().join("alcove");
        let docs_root = dir.path().join("docs");
        // URL with a quote — injection attempt.
        let url = r#"http://localhost:57384/mcp"extra"#;

        write_codex_mcp(&cfg_path, &binary, &docs_root, Some(url), None).expect("write should succeed");

        let written = fs::read_to_string(&cfg_path).expect("read back");
        let parsed: toml::Value = toml::from_str(&written).expect("must be valid TOML");
        let got = parsed["mcpServers"]["alcove"]["url"]
            .as_str()
            .expect("url is a string");
        assert_eq!(got, url);
    }
}
