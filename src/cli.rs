use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use console::style;
use dialoguer::{Input, MultiSelect, Select, theme::ColorfulTheme};
use rust_i18n::t;

use crate::config::{
    config_path, default_docs_root, load_config, DocConfig,
    DOC_REPO_REQUIRED, DOC_REPO_SUPPLEMENTARY, PROJECT_REPO_FILES,
};

// ---------------------------------------------------------------------------
// Agent definitions
// ---------------------------------------------------------------------------

struct AgentDef {
    name: &'static str,
    mcp_config: McpConfig,
    skill_dir: Option<&'static str>,
}

enum McpConfig {
    /// Standard JSON: { "<key>": { "alcove": { "command": "...", "env": {...} } } }
    Json {
        path: &'static str,
        server_key: &'static str,
    },
    /// OpenCode format: { "mcp": { "alcove": { "type": "local", ... } } }
    OpenCode {
        path: &'static str,
    },
    /// Codex TOML format
    Codex {
        path: &'static str,
    },
}

fn home() -> PathBuf {
    dirs::home_dir().expect("Cannot determine home directory")
}

fn agents() -> Vec<AgentDef> {
    vec![
        AgentDef {
            name: "Claude Code",
            mcp_config: McpConfig::Json { path: "~/.claude.json", server_key: "mcpServers" },
            skill_dir: Some("~/.claude/skills/alcove"),
        },
        AgentDef {
            name: "Cursor",
            mcp_config: McpConfig::Json { path: "~/.cursor/mcp.json", server_key: "mcpServers" },
            skill_dir: Some("~/.cursor/skills/alcove"),
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
            },
            skill_dir: None,
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
            },
            skill_dir: None,
        },
        AgentDef {
            name: "OpenCode",
            mcp_config: McpConfig::OpenCode { path: "~/.config/opencode/opencode.json" },
            skill_dir: Some("~/.opencode/skills/alcove"),
        },
        AgentDef {
            name: "Codex CLI",
            mcp_config: McpConfig::Codex { path: "~/.codex/config.toml" },
            skill_dir: None,
        },
        AgentDef {
            name: "Antigravity",
            mcp_config: McpConfig::Json { path: "~/.antigravity/settings.json", server_key: "mcpServers" },
            skill_dir: None,
        },
        AgentDef {
            name: "Gemini CLI",
            mcp_config: McpConfig::Json { path: "~/.gemini/settings.json", server_key: "mcpServers" },
            skill_dir: Some("~/.gemini/skills/alcove"),
        },
    ]
}

fn expand_path(p: &str) -> PathBuf {
    if let Some(stripped) = p.strip_prefix("~/") {
        home().join(stripped)
    } else {
        PathBuf::from(p)
    }
}

// ---------------------------------------------------------------------------
// Resolve docs root
// ---------------------------------------------------------------------------

/// Resolve docs root for setup: always show prompt with current value as default,
/// allowing the user to confirm or change it.
fn resolve_docs_root_interactive() -> Result<PathBuf> {
    let current = saved_docs_root();
    prompt_docs_root(current.as_deref())
}

/// Return saved docs root from env or config.toml, falling back to default.
fn saved_docs_root() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("DOCS_ROOT") {
        let p = PathBuf::from(&v);
        if p.is_dir() {
            return Some(p);
        }
    }
    let cfg = load_config();
    if let Some(p) = cfg.docs_root()
        && p.is_dir() {
            return Some(p);
        }
    // Fall back to default docs root if it exists
    let fallback = default_docs_root();
    if fallback.is_dir() {
        return Some(fallback);
    }
    None
}

/// Interactive prompt for docs root. Shows `default` as pre-filled value if provided.
/// Falls back to `~/.config/alcove/docs` when no previous value exists.
fn prompt_docs_root(default: Option<&Path>) -> Result<PathBuf> {
    let theme = ColorfulTheme::default();
    let prompt = t!("setup.docs_prompt");
    let fallback = default_docs_root();
    let default_path = default.unwrap_or(&fallback);

    let input: String = Input::with_theme(&theme)
        .with_prompt(prompt.as_ref())
        .default(default_path.to_string_lossy().into_owned())
        .interact_text()?;

    let p = PathBuf::from(shellexpand(&input));

    // Auto-create the directory if it doesn't exist (especially for default path)
    if !p.exists() {
        std::fs::create_dir_all(&p)?;
    }
    if !p.is_dir() {
        anyhow::bail!("{}", t!("setup.invalid_path", path = p.display()));
    }

    save_docs_root(&p)?;
    Ok(p)
}

fn shellexpand(s: &str) -> String {
    if let Some(stripped) = s.strip_prefix("~/") {
        format!("{}/{}", home().display(), stripped)
    } else {
        s.to_string()
    }
}

fn save_docs_root(path: &Path) -> Result<()> {
    save_docs_root_to(&config_path(), path)
}

fn save_docs_root_to(cfg_path: &Path, path: &Path) -> Result<()> {
    fs::create_dir_all(cfg_path.parent().unwrap())?;

    if cfg_path.exists() {
        let content = fs::read_to_string(cfg_path)?;
        if content.contains("docs_root") {
            // Update existing
            let updated: String = content
                .lines()
                .map(|l| {
                    if l.trim_start().starts_with("docs_root") {
                        format!("docs_root = \"{}\"", path.display())
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(cfg_path, updated)?;
        } else {
            // Prepend
            let updated = format!("docs_root = \"{}\"\n\n{}", path.display(), content);
            fs::write(cfg_path, updated)?;
        }
    } else {
        fs::write(cfg_path, format!("docs_root = \"{}\"\n", path.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Binary path
// ---------------------------------------------------------------------------

fn binary_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("alcove"))
}

// ---------------------------------------------------------------------------
// Skill file
// ---------------------------------------------------------------------------

const SKILL_CONTENT: &str = include_str!("../skill/SKILL.md");

fn install_skill_to(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join("SKILL.md"), SKILL_CONTENT)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// MCP config writers
// ---------------------------------------------------------------------------

fn write_json_mcp(config_path: &Path, server_key: &str, binary: &Path, docs_root: &Path) -> Result<()> {
    let mut config: serde_json::Value = if config_path.exists() {
        let content = fs::read_to_string(config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let server_entry = serde_json::json!({
        "command": binary.to_string_lossy(),
        "args": [],
        "env": {
            "DOCS_ROOT": docs_root.to_string_lossy()
        }
    });

    config[server_key]["alcove"] = server_entry;

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(config_path, serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

fn write_opencode_mcp(config_path: &Path, binary: &Path, docs_root: &Path) -> Result<()> {
    let mut config: serde_json::Value = if config_path.exists() {
        let content = fs::read_to_string(config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    config["mcp"]["alcove"] = serde_json::json!({
        "type": "local",
        "command": [binary.to_string_lossy()],
        "environment": {
            "DOCS_ROOT": docs_root.to_string_lossy()
        }
    });

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(config_path, serde_json::to_string_pretty(&config)?)?;
    Ok(())
}

fn write_codex_mcp(config_path: &Path, binary: &Path, docs_root: &Path) -> Result<()> {
    let entry = format!(
        "\n[mcp_servers.alcove]\ncommand = \"{}\"\nargs = []\n\n[mcp_servers.alcove.env]\nDOCS_ROOT = \"{}\"\n",
        binary.display(),
        docs_root.display(),
    );

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    if config_path.exists() {
        let content = fs::read_to_string(config_path)?;
        if content.contains("[mcp_servers.alcove]") {
            // Already configured
            return Ok(());
        }
        fs::write(config_path, format!("{content}{entry}"))?;
    } else {
        fs::write(config_path, entry)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Agent selection UI
// ---------------------------------------------------------------------------

fn select_agents(prompt: &str) -> Result<Vec<usize>> {
    let agent_list = agents();
    let names: Vec<&str> = agent_list.iter().map(|a| a.name).collect();

    let selected = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .items(&names)
        .defaults(&vec![false; names.len()])
        .interact()?;

    Ok(selected)
}

// ---------------------------------------------------------------------------
// Diagram format selection
// ---------------------------------------------------------------------------

const DIAGRAM_FORMATS: &[(&str, &str)] = &[
    ("mermaid", "Mermaid — GitHub/GitLab native, most popular"),
    ("plantuml", "PlantUML — Enterprise UML, richest diagram types"),
    ("d2", "D2 — Modern, clean rendering, Go-based"),
    ("ascii", "ASCII art — Universal, no renderer needed"),
    ("graphviz", "Graphviz (DOT) — Classic graph visualization"),
    ("structurizr", "Structurizr (C4) — Architecture-focused C4 model"),
    ("excalidraw", "Excalidraw — Hand-drawn style, brainstorming"),
];

// ---------------------------------------------------------------------------
// Document category selection
// ---------------------------------------------------------------------------

struct CategoryDef {
    label: &'static str,
    defaults: &'static [&'static str],
}

const CATEGORIES: &[CategoryDef] = &[
    CategoryDef {
        label: "Core (private project docs)",
        defaults: DOC_REPO_REQUIRED,
    },
    CategoryDef {
        label: "Team (internal extras)",
        defaults: DOC_REPO_SUPPLEMENTARY,
    },
    CategoryDef {
        label: "Public (repo-facing docs)",
        defaults: PROJECT_REPO_FILES,
    },
];

/// Returns (core_files, team_files, public_files) after interactive selection.
/// Pre-checks items based on existing config or defaults.
fn select_categories() -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
    let cfg = load_fresh_config();
    let existing: [Vec<String>; 3] = [
        cfg.as_ref().map_or_else(
            || DOC_REPO_REQUIRED.iter().map(|s| s.to_string()).collect(),
            |c| c.core_files(),
        ),
        cfg.as_ref().map_or_else(
            || DOC_REPO_SUPPLEMENTARY.iter().map(|s| s.to_string()).collect(),
            |c| c.team_files(),
        ),
        cfg.as_ref().map_or_else(
            || PROJECT_REPO_FILES.iter().map(|s| s.to_string()).collect(),
            |c| c.public_files(),
        ),
    ];

    let theme = ColorfulTheme::default();
    let mut results: Vec<Vec<String>> = Vec::new();

    for (i, cat) in CATEGORIES.iter().enumerate() {
        let items: Vec<&str> = cat.defaults.to_vec();
        let defaults: Vec<bool> = items
            .iter()
            .map(|item| existing[i].iter().any(|e| e == *item))
            .collect();

        let selected = MultiSelect::with_theme(&theme)
            .with_prompt(cat.label)
            .items(&items)
            .defaults(&defaults)
            .interact()?;

        let files: Vec<String> = selected.iter().map(|&idx| items[idx].to_string()).collect();
        println!(
            "  {} {}",
            style("✓").green(),
            t!("setup.category_status", label = cat.label, selected = files.len(), total = items.len())
        );
        results.push(files);
    }

    Ok((
        results.remove(0),
        results.remove(0),
        results.remove(0),
    ))
}

/// Load config fresh from disk (bypasses OnceLock cache).
fn load_fresh_config() -> Option<DocConfig> {
    let path = config_path();
    if path.exists() {
        let content = fs::read_to_string(&path).ok()?;
        toml::from_str::<DocConfig>(&content).ok()
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

pub fn cmd_setup() -> Result<()> {
    println!();
    println!("{}", style("══════════════════════════════════════").bold());
    println!("  {}", style(t!("setup.title")).bold());
    println!("{}", style("══════════════════════════════════════").bold());

    // 1. Docs root
    println!();
    println!("{}", style(format!("── {} ──", t!("setup.docs_repo"))).bold());
    let docs_root = resolve_docs_root_interactive()?;
    println!("  {} {}", style("✓").green(), t!("setup.docs_root_set", path = docs_root.display()));

    // 2. Document categories
    println!();
    println!("{}", style(format!("── {} ──", t!("setup.categories"))).bold());
    let (core_files, team_files, public_files) = select_categories()?;

    // 3. Diagram format
    println!();
    println!("{}", style(format!("── {} ──", t!("setup.diagram"))).bold());
    let existing_diagram = load_fresh_config()
        .map(|c| c.diagram_format())
        .unwrap_or_default();
    let diagram_default = DIAGRAM_FORMATS
        .iter()
        .position(|(k, _)| *k == existing_diagram)
        .unwrap_or(0);
    let format_labels: Vec<&str> = DIAGRAM_FORMATS.iter().map(|(_, l)| *l).collect();
    let diagram_idx = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(t!("setup.diagram_prompt").as_ref())
        .items(&format_labels)
        .default(diagram_default)
        .interact()?;
    let diagram_format = DIAGRAM_FORMATS[diagram_idx].0;
    println!("  {} {}", style("✓").green(), t!("setup.diagram_set", format = diagram_format));

    // 4. Save config
    save_full_config(&docs_root, diagram_format, &core_files, &team_files, &public_files)?;

    // 4. Agent setup
    println!();
    println!("{}", style(format!("── {} ──", t!("setup.agents"))).bold());
    let selected = select_agents(&t!("setup.agents_prompt"))?;
    let agent_list = agents();
    let bin = binary_path();

    for idx in &selected {
        let agent = &agent_list[*idx];
        println!();
        println!("  {}", style(agent.name).cyan());

        // MCP
        match &agent.mcp_config {
            McpConfig::Json { path, server_key } => {
                let p = expand_path(path);
                write_json_mcp(&p, server_key, &bin, &docs_root)?;
                println!("  {} {}", style("✓").green(), t!("setup.mcp_set", path = path));
            }
            McpConfig::OpenCode { path } => {
                let p = expand_path(path);
                write_opencode_mcp(&p, &bin, &docs_root)?;
                println!("  {} {}", style("✓").green(), t!("setup.mcp_set", path = path));
            }
            McpConfig::Codex { path } => {
                let p = expand_path(path);
                write_codex_mcp(&p, &bin, &docs_root)?;
                println!("  {} {}", style("✓").green(), t!("setup.mcp_set", path = path));
            }
        }

        // Skill
        if let Some(skill_path) = agent.skill_dir {
            let p = expand_path(skill_path);
            install_skill_to(&p)?;
            println!("  {} {}", style("✓").green(), t!("setup.skill_set", path = skill_path));
        }
    }

    // 5. Summary
    println!();
    println!("{}", style(format!("── {} ──", t!("setup.done"))).bold());
    println!("  {}", t!("setup.binary", path = binary_path().display()));
    println!("  {}", t!("setup.config", path = config_path().display()));
    println!("  {}", t!("setup.docs", path = docs_root.display()));
    println!();
    println!("  {}", style(t!("setup.hint_update").to_string()).dim());
    println!("  {}", style(t!("setup.hint_uninstall").to_string()).dim());
    println!();

    Ok(())
}

pub fn cmd_uninstall() -> Result<()> {
    println!();
    println!("{}", style(t!("uninstall.title").to_string()).bold());
    println!();

    // Skills
    let skill_dirs = [
        "~/.claude/skills/alcove",
        "~/.cursor/skills/alcove",
        "~/.opencode/skills/alcove",
        "~/.gemini/skills/alcove",
    ];
    for d in &skill_dirs {
        let p = expand_path(d);
        if p.exists() {
            fs::remove_dir_all(&p)?;
            println!("  {} {}", style("✓").green(), t!("uninstall.removed_skill", path = d));
        }
    }

    // Config
    let cfg = config_path();
    if cfg.exists() {
        fs::remove_file(&cfg)?;
        println!("  {} {}", style("✓").green(), t!("uninstall.removed_config", path = cfg.display()));
    }
    // Legacy config
    let legacy = cfg.with_file_name("config");
    if legacy.exists() {
        fs::remove_file(&legacy)?;
        println!("  {} {}", style("✓").green(), t!("uninstall.removed_legacy", path = legacy.display()));
    }

    println!();
    println!("  {}", style(t!("uninstall.binary_hint").to_string()).yellow());
    println!();
    println!("  {}", t!("uninstall.mcp_hint"));
    println!("    Claude Code:    ~/.claude.json");
    println!("    Cursor:         ~/.cursor/mcp.json");
    println!("    Claude Desktop: ~/Library/Application Support/Claude/claude_desktop_config.json");
    println!("    OpenCode:       ~/.config/opencode/opencode.json");
    println!("    Codex:          ~/.codex/config.toml");
    println!("    Antigravity:    ~/.antigravity/settings.json");
    println!("    Gemini CLI:     ~/.gemini/settings.json");
    println!();

    Ok(())
}

// ---------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------

pub fn cmd_validate(format: &str, exit_code: bool) -> Result<()> {
    use crate::policy;
    use crate::tools;

    let docs_root = match saved_docs_root() {
        Some(p) => p,
        None => {
            anyhow::bail!("docs_root is not configured. Run `alcove setup` first.");
        }
    };

    let resolved = match tools::resolve_project(&docs_root) {
        Some(r) => r,
        None => {
            anyhow::bail!(
                "Could not detect project. Run from within a project directory or set MCP_PROJECT_NAME."
            );
        }
    };

    let repo_path = resolved.repo_path.as_deref();
    let source = policy::policy_source(&docs_root, &resolved.name);
    let (pol, results) = policy::validate(&docs_root, &resolved.name, repo_path)?;

    if format == "json" {
        let json = policy::validation_to_json(&pol, &results, source);
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        print_validation_human(&pol, &results, source, &resolved.name);
    }

    if exit_code {
        let has_fail = results.iter().any(|r| r.status == policy::FileStatus::Fail);
        if has_fail && pol.policy.enforce == "strict" {
            std::process::exit(1);
        }
    }

    Ok(())
}

fn print_validation_human(
    pol: &crate::policy::PolicyFile,
    results: &[crate::policy::ValidationResult],
    source: &str,
    project_name: &str,
) {
    use crate::policy::FileStatus;

    println!();
    println!(
        "{}",
        style(format!("Document Policy: {} (source: {source})", pol.policy.enforce)).bold()
    );
    println!("{}", style(format!("Project: {project_name}")).dim());
    println!();

    for r in results {
        let icon = match r.status {
            FileStatus::Pass => style("  PASS").green(),
            FileStatus::Warn => style("  WARN").yellow(),
            FileStatus::Fail => style("  FAIL").red(),
        };
        let reason = r.reason.as_deref().map(|s| format!(" — {s}")).unwrap_or_default();
        println!("{icon} {}{reason}", r.file);

        for s in &r.sections {
            let sec_icon = match s.status {
                FileStatus::Pass => style("    PASS").green(),
                FileStatus::Warn => style("    WARN").yellow(),
                FileStatus::Fail => style("    FAIL").red(),
            };
            let detail = s.detail.as_deref().map(|d| format!(" ({d})")).unwrap_or_default();
            println!("{sec_icon} {}{detail}", s.heading);
        }
    }

    let pass = results.iter().filter(|r| r.status == FileStatus::Pass).count();
    let warn = results.iter().filter(|r| r.status == FileStatus::Warn).count();
    let fail = results.iter().filter(|r| r.status == FileStatus::Fail).count();

    println!();
    println!(
        "Summary: {} passed, {} warning, {} error",
        style(pass).green(),
        style(warn).yellow(),
        style(fail).red(),
    );
    println!();
}

// ---------------------------------------------------------------------------
// alcove index
// ---------------------------------------------------------------------------

pub fn cmd_index() -> Result<()> {
    let docs_root = match saved_docs_root() {
        Some(p) => p,
        None => {
            anyhow::bail!("docs_root is not configured. Run `alcove setup` first.");
        }
    };

    println!(
        "{}",
        style("Building search index...").bold()
    );

    let result = crate::index::build_index(&docs_root)?;

    let projects = result["projects"].as_u64().unwrap_or(0);
    let indexed = result["indexed"].as_u64().unwrap_or(0);
    let skipped = result["skipped"].as_u64().unwrap_or(0);

    println!(
        "  {} {} project(s), {} file(s) indexed, {} skipped (unchanged)",
        style("✓").green(),
        projects,
        indexed,
        skipped,
    );
    println!(
        "  {} {}",
        style("Index:").dim(),
        result["index_path"].as_str().unwrap_or("unknown"),
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// alcove search
// ---------------------------------------------------------------------------

pub fn cmd_search(query: &str, scope: &str, mode: &str, limit: usize) -> Result<()> {
    let docs_root = match saved_docs_root() {
        Some(p) => p,
        None => {
            anyhow::bail!("docs_root is not configured. Run `alcove setup` first.");
        }
    };

    let use_ranked = match mode {
        "grep" => false,
        "ranked" => true,
        _ => {
            // "auto": use ranked if index exists or can be built
            let index_dir = docs_root.join(".alcove").join("index");
            index_dir.exists() || crate::index::is_index_stale(&docs_root)
        }
    };

    let result = if use_ranked {
        // Auto-rebuild index if stale or missing
        if crate::index::is_index_stale(&docs_root) {
            eprintln!("{}", style("Rebuilding search index...").dim());
            let _ = crate::index::build_index(&docs_root);
        }
        let project_filter = if scope == "global" {
            None
        } else {
            crate::tools::resolve_project(&docs_root).map(|r| r.name)
        };
        match crate::index::search_indexed(&docs_root, query, limit, project_filter.as_deref()) {
            Ok(v) => {
                let matches = v["matches"].as_array();
                if matches.is_some_and(|m| !m.is_empty()) {
                    v
                } else {
                    // Ranked returned 0 results → fallback to grep
                    run_grep_search(&docs_root, query, scope, limit)?
                }
            }
            Err(_) => {
                // Index error → fallback to grep
                if mode != "ranked" {
                    // Only show warning in auto mode, not when user explicitly chose ranked
                    eprintln!(
                        "{}",
                        style("Index unavailable, falling back to grep.").yellow()
                    );
                }
                run_grep_search(&docs_root, query, scope, limit)?
            }
        }
    } else {
        run_grep_search(&docs_root, query, scope, limit)?
    };

    let empty = vec![];
    let matches = result["matches"].as_array().unwrap_or(&empty);

    if matches.is_empty() {
        println!("{}", style("No results found.").dim());
        return Ok(());
    }

    println!(
        "{} {} result(s) for {}",
        style("Found").bold(),
        matches.len(),
        style(format!("\"{}\"", query)).cyan(),
    );
    if result.get("mode").and_then(|v| v.as_str()) == Some("ranked") {
        println!("{}", style("  (ranked by BM25 relevance)").dim());
    }
    println!();

    for m in matches {
        let project = m.get("project").and_then(|v| v.as_str());
        let file = m.get("file").and_then(|v| v.as_str()).unwrap_or("?");
        let line = m.get("line").or(m.get("line_start"));
        let snippet = m.get("snippet").and_then(|v| v.as_str()).unwrap_or("");
        let score = m.get("score").and_then(|v| v.as_f64());

        let location = if let Some(proj) = project {
            format!("{}:{}", style(proj).green(), style(file).cyan())
        } else {
            style(file).cyan().to_string()
        };

        let line_info = line
            .and_then(|v| v.as_u64())
            .map(|l| format!(":{l}"))
            .unwrap_or_default();

        let score_info = score
            .map(|s| format!(" {}", style(format!("[{s:.3}]")).dim()))
            .unwrap_or_default();

        println!("  {}{}{}", location, style(line_info).dim(), score_info);

        // Show snippet (truncate long lines, respecting char boundaries)
        let display = if snippet.chars().count() > 120 {
            let truncated: String = snippet.chars().take(117).collect();
            format!("{truncated}...")
        } else {
            snippet.to_string()
        };
        println!("    {}", style(display).dim());
    }

    if result.get("truncated") == Some(&serde_json::json!(true)) {
        println!();
        println!("{}", style("  (results truncated, use --limit to see more)").dim());
    }

    Ok(())
}

fn run_grep_search(docs_root: &std::path::Path, query: &str, scope: &str, limit: usize) -> Result<serde_json::Value> {
    if scope == "global" {
        crate::tools::tool_search_global(docs_root, serde_json::json!({"query": query, "scope": "global", "limit": limit}))
    } else {
        let resolved = crate::tools::resolve_project(docs_root);
        match resolved {
            Some(r) => {
                let project_root = docs_root.join(&r.name);
                crate::tools::tool_search(&project_root, serde_json::json!({"query": query, "limit": limit}), r.repo_path.as_deref())
            }
            None => {
                anyhow::bail!(
                    "Could not detect project. Run from within a project directory, set MCP_PROJECT_NAME, or use --scope global."
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Save config.toml
// ---------------------------------------------------------------------------

/// Save full config with all categories (used by setup).
fn save_full_config(
    docs_root: &Path,
    diagram_format: &str,
    core_files: &[String],
    team_files: &[String],
    public_files: &[String],
) -> Result<()> {
    let cfg_path = config_path();
    save_full_config_to(&cfg_path, docs_root, diagram_format, core_files, team_files, public_files)?;
    println!("  {} {}", style("✓").green(), t!("setup.config_saved", path = cfg_path.display()));
    Ok(())
}

fn save_full_config_to(
    cfg_path: &Path,
    docs_root: &Path,
    diagram_format: &str,
    core_files: &[String],
    team_files: &[String],
    public_files: &[String],
) -> Result<()> {
    fs::create_dir_all(cfg_path.parent().unwrap())?;

    let fmt_list = |files: &[String]| -> String {
        files.iter().map(|f| format!("\"{}\"", f)).collect::<Vec<_>>().join(", ")
    };

    let content = format!(
        "docs_root = \"{}\"\n\n[core]\nfiles = [{}]\n\n[team]\nfiles = [{}]\n\n[public]\nfiles = [{}]\n\n[diagram]\nformat = \"{}\"\n",
        docs_root.display(),
        fmt_list(core_files),
        fmt_list(team_files),
        fmt_list(public_files),
        diagram_format,
    );
    fs::write(cfg_path, content)?;
    Ok(())
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

    // ── shellexpand ──

    #[test]
    fn shellexpand_tilde() {
        let s = shellexpand("~/my/path");
        let expected = format!("{}/my/path", home().display());
        assert_eq!(s, expected);
    }

    #[test]
    fn shellexpand_no_tilde() {
        let s = shellexpand("/absolute/path");
        assert_eq!(s, "/absolute/path");
    }

    // ── binary_path ──

    #[test]
    fn binary_path_is_not_empty() {
        let p = binary_path();
        assert!(!p.as_os_str().is_empty());
    }

    // ── agents ──

    #[test]
    fn agents_returns_expected_count() {
        let a = agents();
        assert_eq!(a.len(), 8, "expected 8 agent definitions");
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
                McpConfig::Json { path, server_key } => {
                    assert!(!path.is_empty());
                    assert!(!server_key.is_empty());
                }
                McpConfig::OpenCode { path } => assert!(!path.is_empty()),
                McpConfig::Codex { path } => assert!(!path.is_empty()),
            }
        }
    }

    // ── DIAGRAM_FORMATS ──

    #[test]
    fn diagram_formats_has_expected_entries() {
        assert_eq!(DIAGRAM_FORMATS.len(), 7);
        let keys: Vec<&str> = DIAGRAM_FORMATS.iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&"mermaid"));
        assert!(keys.contains(&"plantuml"));
        assert!(keys.contains(&"d2"));
        assert!(keys.contains(&"ascii"));
        assert!(keys.contains(&"graphviz"));
        assert!(keys.contains(&"structurizr"));
        assert!(keys.contains(&"excalidraw"));
    }

    #[test]
    fn diagram_formats_all_have_labels() {
        for (key, label) in DIAGRAM_FORMATS {
            assert!(!key.is_empty());
            assert!(!label.is_empty());
        }
    }

    // ── CATEGORIES ──

    #[test]
    fn categories_has_three_entries() {
        assert_eq!(CATEGORIES.len(), 3);
    }

    #[test]
    fn categories_labels_match_expected() {
        assert!(CATEGORIES[0].label.contains("Core"));
        assert!(CATEGORIES[1].label.contains("Team"));
        assert!(CATEGORIES[2].label.contains("Public"));
    }

    #[test]
    fn categories_defaults_are_non_empty() {
        for cat in CATEGORIES {
            assert!(!cat.defaults.is_empty(), "category '{}' should have defaults", cat.label);
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

        let result = write_json_mcp(&cfg, "mcpServers", &bin, &docs);
        assert!(result.is_ok());
        assert!(cfg.exists());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("invalid json");

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
        fs::write(&cfg, serde_json::to_string_pretty(&existing).unwrap())
            .expect("failed to write");

        let bin = PathBuf::from("/bin/alcove");
        let docs = PathBuf::from("/docs");

        let result = write_json_mcp(&cfg, "mcpServers", &bin, &docs);
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

        let result = write_json_mcp(&cfg, "mcpServers", &bin, &docs);
        assert!(result.is_ok());
        assert!(cfg.exists());
    }

    // ── write_opencode_mcp ──

    #[test]
    fn write_opencode_mcp_creates_new_file() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("opencode.json");
        let bin = PathBuf::from("/bin/alcove");
        let docs = PathBuf::from("/docs");

        let result = write_opencode_mcp(&cfg, &bin, &docs);
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

        let result = write_opencode_mcp(&cfg, &PathBuf::from("/bin/alcove"), &PathBuf::from("/docs"));
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

        let result = write_codex_mcp(&cfg, &bin, &docs);
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        assert!(content.contains("[mcp_servers.alcove]"));
        assert!(content.contains(r#"command = "/bin/alcove""#));
        assert!(content.contains(r#"DOCS_ROOT = "/docs""#));
    }

    #[test]
    fn write_codex_mcp_appends_to_existing() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("config.toml");

        fs::write(&cfg, "[some_other_section]\nkey = \"value\"\n").expect("failed to write");

        let result = write_codex_mcp(&cfg, &PathBuf::from("/bin/alcove"), &PathBuf::from("/docs"));
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        assert!(content.contains("[some_other_section]"));
        assert!(content.contains("[mcp_servers.alcove]"));
    }

    #[test]
    fn write_codex_mcp_skips_if_already_configured() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("config.toml");

        let original = "[mcp_servers.alcove]\ncommand = \"/old/bin\"\n";
        fs::write(&cfg, original).expect("failed to write");

        let result = write_codex_mcp(&cfg, &PathBuf::from("/new/bin"), &PathBuf::from("/docs"));
        assert!(result.is_ok());

        // Content should be unchanged (skipped)
        let content = fs::read_to_string(&cfg).expect("failed to read");
        assert_eq!(content, original);
    }

    // ── save_docs_root_to ──

    #[test]
    fn save_docs_root_creates_new_config() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("config.toml");
        let docs = tmp.path().join("my_docs");

        let result = save_docs_root_to(&cfg, &docs);
        assert!(result.is_ok());
        assert!(cfg.exists());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        assert!(content.starts_with("docs_root = "));
        assert!(content.contains(&docs.display().to_string()));
    }

    #[test]
    fn save_docs_root_updates_existing_docs_root() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("config.toml");
        fs::write(&cfg, "docs_root = \"/old/path\"\nother = \"keep\"\n")
            .expect("failed to write");

        let new_docs = tmp.path().join("new_docs");
        let result = save_docs_root_to(&cfg, &new_docs);
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        assert!(content.contains(&new_docs.display().to_string()));
        assert!(!content.contains("/old/path"));
        assert!(content.contains("other = \"keep\""));
    }

    #[test]
    fn save_docs_root_prepends_when_no_docs_root_key() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("config.toml");
        fs::write(&cfg, "[diagram]\nformat = \"mermaid\"\n").expect("failed to write");

        let docs = tmp.path().join("docs");
        let result = save_docs_root_to(&cfg, &docs);
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        assert!(content.starts_with("docs_root = "));
        assert!(content.contains("[diagram]"));
    }

    // ── save_full_config_to ──

    #[test]
    fn save_full_config_writes_all_sections() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("config.toml");
        let docs = tmp.path().join("docs");
        let core = vec!["PRD.md".to_string(), "ARCHITECTURE.md".to_string()];
        let team = vec!["ENV_SETUP.md".to_string()];
        let public = vec!["README.md".to_string()];

        let result = save_full_config_to(&cfg, &docs, "mermaid", &core, &team, &public);
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        assert!(content.contains(&docs.display().to_string()));
        assert!(content.contains("[core]"));
        assert!(content.contains("\"PRD.md\""));
        assert!(content.contains("\"ARCHITECTURE.md\""));
        assert!(content.contains("[team]"));
        assert!(content.contains("\"ENV_SETUP.md\""));
        assert!(content.contains("[public]"));
        assert!(content.contains("\"README.md\""));
        assert!(content.contains("[diagram]"));
        assert!(content.contains("format = \"mermaid\""));
    }
}
