use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use console::style;

// ---------------------------------------------------------------------------
// Agent definitions
// ---------------------------------------------------------------------------

/// How an agent references environment variables in its MCP config.
pub(crate) enum EnvVarSyntax {
    /// `"${VAR}"` — Claude Code, Claude Desktop, Copilot, Codex
    DollarBrace,
    /// `"${env:VAR}"` — Cursor, Cline
    DollarEnvColon,
    /// `"{env:VAR}"` — OpenCode
    BraceEnvColon,
}

impl EnvVarSyntax {
    pub(crate) fn render(&self, var: &str) -> Option<String> {
        match self {
            EnvVarSyntax::DollarBrace => Some(format!("${{{var}}}")),
            EnvVarSyntax::DollarEnvColon => Some(format!("${{env:{var}}}")),
            EnvVarSyntax::BraceEnvColon => Some(format!("{{env:{var}}}")),
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
}

pub(crate) fn home() -> PathBuf {
    dirs::home_dir().expect("Cannot determine home directory")
}

pub(crate) fn agents() -> Vec<AgentDef> {
    vec![
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
            name: "Copilot CLI",
            mcp_config: McpConfig::Json {
                path: "~/.copilot/mcp-config.json",
                server_key: "mcpServers",
                omit_type: false,
            },
            skill_dir: Some("~/.copilot/skills/alcove"),
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

const SKILL_CONTENT: &str = include_str!("../registry/skills/alcove/SKILL.md");

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
    _server_key: &str,
    _binary: &Path,
    _docs_root: &Path,
    _server_url: Option<&str>,
    _token_ref: Option<&str>,
    _omit_type: bool,
) -> Result<()> {
    println!(
        "  {} MCP server not auto-configured (skill-driven mode).",
        style("ℹ").dim()
    );
    println!("     To use MCP, see registry/mcp.json for manual setup.");
    let _ = config_path; // suppress unused warning
    Ok(())
}

pub(crate) fn write_opencode_mcp(
    config_path: &Path,
    _binary: &Path,
    _docs_root: &Path,
    _server_url: Option<&str>,
    _token_ref: Option<&str>,
) -> Result<()> {
    println!(
        "  {} MCP server not auto-configured (skill-driven mode).",
        style("ℹ").dim()
    );
    println!("     To use MCP, see registry/mcp.json for manual setup.");
    let _ = config_path; // suppress unused warning
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
    };
    let expanded = expand_path(path);

    if !expanded.exists() {
        return (
            "skip",
            t!("doctor.agent_config_not_found", path = path).to_string(),
        );
    }

    let content = match fs::read_to_string(&expanded) {
        Ok(c) => c,
        Err(_) => {
            return (
                "error",
                t!("doctor.agent_cannot_read", path = path).to_string(),
            );
        }
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
                parsed.get("mcp").and_then(|m| m.get("alcove")).is_some()
            } else {
                false
            }
        }
    };

    if has_alcove {
        ("ok", t!("doctor.agent_registered").to_string())
    } else {
        (
            "error",
            t!("doctor.agent_not_registered", path = path).to_string(),
        )
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
        assert_eq!(a.len(), 5, "expected 5 agent definitions");
    }

    #[test]
    fn agents_contains_known_names() {
        let a = agents();
        let names: Vec<&str> = a.iter().map(|x| x.name).collect();
        assert!(names.contains(&"Cursor"));
        assert!(names.contains(&"Claude Desktop"));
        assert!(names.contains(&"Cline (VS Code)"));
        assert!(names.contains(&"OpenCode"));
        assert!(names.contains(&"Copilot CLI"));
    }

    #[test]
    fn agents_all_have_mcp_config() {
        let a = agents();
        for agent in &a {
            match &agent.mcp_config {
                McpConfig::Json {
                    path, server_key, ..
                } => {
                    assert!(!path.is_empty());
                    assert!(!server_key.is_empty());
                }
                McpConfig::OpenCode { path } => assert!(!path.is_empty()),
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

    // ── write_json_mcp (skill-driven mode: no file written) ──

    #[test]
    fn write_json_mcp_returns_ok() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("mcp.json");
        let bin = PathBuf::from("/usr/local/bin/alcove");
        let docs = PathBuf::from("/docs/root");

        let result = write_json_mcp(&cfg, "mcpServers", &bin, &docs, None, None, false);
        assert!(result.is_ok());
        // Skill-driven mode: no file is created
        assert!(!cfg.exists());
    }

    // ── write_opencode_mcp (skill-driven mode: no file written) ──

    #[test]
    fn write_opencode_mcp_returns_ok() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("opencode.json");
        let bin = PathBuf::from("/bin/alcove");
        let docs = PathBuf::from("/docs");

        let result = write_opencode_mcp(&cfg, &bin, &docs, None, None);
        assert!(result.is_ok());
        // Skill-driven mode: no file is created
        assert!(!cfg.exists());
    }

    // ── Consistency tests (compile-time validated via include_str!) ──

    const HOOKS_CLAUDE: &str = include_str!("../.claude-plugin/hooks.json");
    const PLUGIN_CLAUDE: &str = include_str!("../.claude-plugin/plugin.json");
    const PLUGIN_CODEX: &str = include_str!("../.codex-plugin/plugin.json");
    const MCP_CONFIG: &str = include_str!("../registry/mcp.json");

    #[test]
    fn hooks_claude_valid() {
        let parsed: serde_json::Value = serde_json::from_str(HOOKS_CLAUDE)
            .expect(".claude-plugin/hooks.json must be valid JSON");
        let hooks = &parsed["hooks"];
        assert!(hooks.get("SessionStart").is_some(), "missing SessionStart");
        assert!(hooks.get("SessionEnd").is_some(), "missing SessionEnd");

        let session_end_cmd = hooks["SessionEnd"][0]["hooks"][0]["command"]
            .as_str()
            .expect("missing SessionEnd command");
        assert!(
            session_end_cmd.contains("TOOL=$(command -v alcove"),
            "SessionEnd must use TOOL guard pattern"
        );
    }

    #[test]
    fn plugin_claude_valid() {
        let parsed: serde_json::Value = serde_json::from_str(PLUGIN_CLAUDE)
            .expect(".claude-plugin/plugin.json must be valid JSON");
        assert!(parsed["name"].is_string(), "missing name");
        assert!(parsed["skills"].is_string(), "missing skills reference");
        assert!(parsed["hooks"].is_string(), "missing hooks reference");
        // mcpServers is optional in skill-driven mode
    }

    #[test]
    fn plugin_codex_valid() {
        let parsed: serde_json::Value = serde_json::from_str(PLUGIN_CODEX)
            .expect(".codex-plugin/plugin.json must be valid JSON");
        assert!(parsed["name"].is_string(), "missing name");
    }

    #[test]
    fn mcp_config_valid() {
        let parsed: serde_json::Value =
            serde_json::from_str(MCP_CONFIG).expect("registry/mcp.json must be valid JSON");
        assert!(parsed["mcpServers"]["alcove"]["command"].is_string());
    }

    #[test]
    fn hooks_claude_references_install_js() {
        let parsed: serde_json::Value = serde_json::from_str(HOOKS_CLAUDE).expect("invalid JSON");
        let cmd = parsed["hooks"]["SessionStart"][0]["hooks"][0]["command"]
            .as_str()
            .expect("missing command");
        assert!(
            cmd.contains("install.js"),
            "SessionStart must reference install.js"
        );
    }
}
