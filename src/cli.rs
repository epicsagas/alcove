use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use console::style;
use dialoguer::{Input, MultiSelect, Select, theme::ColorfulTheme};
use rust_i18n::t;

use crate::config::{
    DOC_REPO_REQUIRED, DOC_REPO_SUPPLEMENTARY, DocConfig, PROJECT_REPO_FILES, config_path,
    default_docs_root, load_config,
};

// ---------------------------------------------------------------------------
// Agent definitions
// ---------------------------------------------------------------------------

pub(crate) struct AgentDef {
    pub(crate) name: &'static str,
    pub(crate) mcp_config: McpConfig,
    pub(crate) skill_dir: Option<&'static str>,
}

pub(crate) enum McpConfig {
    /// Standard JSON: { "<key>": { "alcove": { "command": "...", "env": {...} } } }
    Json {
        path: &'static str,
        server_key: &'static str,
    },
    /// OpenCode format: { "mcp": { "alcove": { "type": "local", ... } } }
    OpenCode { path: &'static str },
    /// Codex TOML format
    Codex { path: &'static str },
}

fn home() -> PathBuf {
    dirs::home_dir().expect("Cannot determine home directory")
}

pub(crate) fn agents() -> Vec<AgentDef> {
    vec![
        AgentDef {
            name: "Claude Code",
            mcp_config: McpConfig::Json {
                path: "~/.claude.json",
                server_key: "mcpServers",
            },
            skill_dir: Some("~/.claude/skills/alcove"),
        },
        AgentDef {
            name: "Cursor",
            mcp_config: McpConfig::Json {
                path: "~/.cursor/mcp.json",
                server_key: "mcpServers",
            },
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
            skill_dir: Some("~/.cline/skills/alcove"),
        },
        AgentDef {
            name: "OpenCode",
            mcp_config: McpConfig::OpenCode {
                path: "~/.config/opencode/opencode.json",
            },
            skill_dir: Some("~/.opencode/skills/alcove"),
        },
        AgentDef {
            name: "Codex CLI",
            mcp_config: McpConfig::Codex {
                path: "~/.codex/config.toml",
            },
            skill_dir: Some("~/.codex/skills/alcove"),
        },
        AgentDef {
            name: "Copilot CLI",
            mcp_config: McpConfig::Json {
                path: "~/.copilot/mcp-config.json",
                server_key: "mcpServers",
            },
            skill_dir: Some("~/.copilot/skills/alcove"),
        },
        AgentDef {
            name: "Antigravity",
            mcp_config: McpConfig::Json {
                path: "~/.gemini/antigravity/mcp_config.json",
                server_key: "mcpServers",
            },
            skill_dir: None, // skills.txt references external skill dirs
        },
        AgentDef {
            name: "Gemini CLI",
            mcp_config: McpConfig::Json {
                path: "~/.gemini/settings.json",
                server_key: "mcpServers",
            },
            skill_dir: Some("~/.gemini/skills/alcove"),
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
// Resolve docs root
// ---------------------------------------------------------------------------

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
        && p.is_dir()
    {
        return Some(p);
    }
    // Fall back to default docs root if it exists
    let fallback = default_docs_root();
    if fallback.is_dir() {
        return Some(fallback);
    }
    None
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

fn write_json_mcp(
    config_path: &Path,
    server_key: &str,
    binary: &Path,
    docs_root: &Path,
) -> Result<()> {
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
// Setup wizard state machine
// ---------------------------------------------------------------------------

/// Total number of setup steps (for progress indicator)
const SETUP_STEPS: usize = 7;

/// Setup wizard steps
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Step {
    DocsRoot = 0,
    Categories = 1,
    Diagram = 2,
    Embedding = 3,
    Server = 4,
    Agents = 5,
    Summary = 6,
}

impl Step {
    fn header(&self) -> String {
        use std::borrow::Cow;
        let step_num = *self as usize + 1;
        let title: Cow<'_, str> = match self {
            Step::DocsRoot => t!("setup.docs_repo"),
            Step::Categories => t!("setup.categories"),
            Step::Diagram => t!("setup.diagram"),
            Step::Embedding => Cow::Borrowed("Embedding Model (Hybrid Search)"),
            Step::Server => Cow::Borrowed("HTTP RAG Server"),
            Step::Agents => t!("setup.agents"),
            Step::Summary => t!("setup.done"),
        };
        format!("[{}/{}] ── {} ──", step_num, SETUP_STEPS, title.as_ref())
    }

    fn next(&self) -> Option<Step> {
        match self {
            Step::DocsRoot => Some(Step::Categories),
            Step::Categories => Some(Step::Diagram),
            Step::Diagram => Some(Step::Embedding),
            Step::Embedding => Some(Step::Server),
            Step::Server => Some(Step::Agents),
            Step::Agents => Some(Step::Summary),
            Step::Summary => None,
        }
    }

    fn prev(&self) -> Option<Step> {
        match self {
            Step::DocsRoot => None,
            Step::Categories => Some(Step::DocsRoot),
            Step::Diagram => Some(Step::Categories),
            Step::Embedding => Some(Step::Diagram),
            Step::Server => Some(Step::Embedding),
            Step::Agents => Some(Step::Server),
            Step::Summary => Some(Step::Agents),
        }
    }
}

/// Result of a step execution
enum StepResult {
    /// Continue to next step
    Continue,
    /// Go back to previous step
    Back,
}

/// Holds all state collected during the setup wizard
#[derive(Default)]
struct SetupState {
    docs_root: Option<PathBuf>,
    core_files: Vec<String>,
    team_files: Vec<String>,
    public_files: Vec<String>,
    diagram_format: Option<String>,
    embedding_section: Option<String>,
    server_section: Option<String>,
    enable_server: bool,
    selected_agents: Vec<usize>,
}


/// Print step header with progress indicator
fn print_step_header(step: &Step) {
    println!();
    println!("{}", style(step.header()).bold());
}

// ---------------------------------------------------------------------------
// Embedding model selection
// ---------------------------------------------------------------------------

/// Embedding model options for setup wizard
#[cfg(feature = "alcove-full")]
const EMBEDDING_OPTIONS: &[(&str, &str, usize, usize)] = &[
    ("MultilingualE5Small", "Default, balanced (100+ langs, ~235MB)", 384, 235),
    ("SnowflakeArcticEmbedXS", "Smallest, fastest (~30MB)", 384, 30),
    ("SnowflakeArcticEmbedXSQ", "Quantized, minimal disk (~15MB)", 384, 15),
    ("SnowflakeArcticEmbedS", "Quality/size balance (~130MB)", 384, 130),
    ("MultilingualE5Base", "Large scale docs (~555MB)", 768, 555),
    ("disabled", "Disable embedding (BM25 only)", 0, 0),
];

// ---------------------------------------------------------------------------
// Step functions with Back support
// ---------------------------------------------------------------------------

/// Add "← Back" option to labels and return the modified list
fn add_back_option(labels: &[String]) -> Vec<String> {
    let mut result = vec![style("← Go back").yellow().to_string()];
    result.extend(labels.iter().cloned());
    result
}

/// Check if user selected "Back" (index 0 after adding back option)
fn is_back_selection(idx: usize) -> bool {
    idx == 0
}

/// Adjust index to account for "Back" option being at index 0
fn adjust_index_for_back(idx: usize) -> usize {
    idx.saturating_sub(1)
}

/// Step 1: Docs Root selection
fn step_docs_root(state: &mut SetupState) -> Result<StepResult> {
    print_step_header(&Step::DocsRoot);
    
    let current = state.docs_root.clone().or_else(saved_docs_root);
    
    // Show current value as default
    let theme = ColorfulTheme::default();
    let prompt = t!("setup.docs_prompt");
    let fallback = default_docs_root();
    let default_path = current.as_ref().unwrap_or(&fallback);

    // Add back option info
    println!("{}", style("  (Press Enter to confirm, or type '..' to go back)").dim());

    let input: String = Input::with_theme(&theme)
        .with_prompt(prompt.as_ref())
        .default(default_path.to_string_lossy().into_owned())
        .interact_text()?;

    // Check for back command
    if input.trim() == ".." {
        return Ok(StepResult::Back);
    }

    let p = PathBuf::from(shellexpand(&input));

    // Auto-create the directory if it doesn't exist
    if !p.exists() {
        std::fs::create_dir_all(&p)?;
    }
    if !p.is_dir() {
        anyhow::bail!("{}", t!("setup.invalid_path", path = p.display()));
    }

    state.docs_root = Some(p.clone());
    save_docs_root(&p)?;
    
    println!(
        "  {} {}",
        style("✓").green(),
        t!("setup.docs_root_set", path = p.display())
    );

    Ok(StepResult::Continue)
}

/// Step 2: Document Categories selection
fn step_categories(state: &mut SetupState) -> Result<StepResult> {
    print_step_header(&Step::Categories);
    println!("{}", style("  (Uncheck all and continue to go back)").dim());

    let (core_files, team_files, public_files) = select_categories_with_back(state)?;
    
    // If all empty, treat as back
    if core_files.is_empty() && team_files.is_empty() && public_files.is_empty() {
        return Ok(StepResult::Back);
    }

    state.core_files = core_files;
    state.team_files = team_files;
    state.public_files = public_files;

    Ok(StepResult::Continue)
}

/// Modified category selection that supports going back
fn select_categories_with_back(state: &SetupState) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
    // Use existing state or load from config
    let cfg = load_fresh_config();
    let existing: [Vec<String>; 3] = [
        if state.core_files.is_empty() {
            cfg.as_ref().map_or_else(
                || DOC_REPO_REQUIRED.iter().map(std::string::ToString::to_string).collect(),
                super::config::DocConfig::core_files,
            )
        } else {
            state.core_files.clone()
        },
        if state.team_files.is_empty() {
            cfg.as_ref().map_or_else(
                || DOC_REPO_SUPPLEMENTARY.iter().map(std::string::ToString::to_string).collect(),
                super::config::DocConfig::team_files,
            )
        } else {
            state.team_files.clone()
        },
        if state.public_files.is_empty() {
            cfg.as_ref().map_or_else(
                || PROJECT_REPO_FILES.iter().map(std::string::ToString::to_string).collect(),
                super::config::DocConfig::public_files,
            )
        } else {
            state.public_files.clone()
        },
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
            t!(
                "setup.category_status",
                label = cat.label,
                selected = files.len(),
                total = items.len()
            )
        );
        results.push(files);
    }

    Ok((results.remove(0), results.remove(0), results.remove(0)))
}

/// Step 3: Diagram Format selection
fn step_diagram(state: &mut SetupState) -> Result<StepResult> {
    print_step_header(&Step::Diagram);

    let existing_diagram = state.diagram_format.clone().unwrap_or_else(|| {
        load_fresh_config()
            .map(|c| c.diagram_format())
            .unwrap_or_default()
    });

    // Add back option
    let format_labels: Vec<String> = add_back_option(
        &DIAGRAM_FORMATS
            .iter()
            .map(|(_, l)| l.to_string())
            .collect::<Vec<_>>()
    );

    let diagram_default = DIAGRAM_FORMATS
        .iter()
        .position(|(k, _)| *k == existing_diagram)
        .map(|i| i + 1) // +1 for back option
        .unwrap_or(1);

    let idx = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(t!("setup.diagram_prompt").as_ref())
        .items(&format_labels)
        .default(diagram_default)
        .interact()?;

    if is_back_selection(idx) {
        return Ok(StepResult::Back);
    }

    let real_idx = adjust_index_for_back(idx);
    let diagram_format = DIAGRAM_FORMATS[real_idx].0;
    
    state.diagram_format = Some(diagram_format.to_string());
    
    println!(
        "  {} {}",
        style("✓").green(),
        t!("setup.diagram_set", format = diagram_format)
    );

    Ok(StepResult::Continue)
}

/// Step 4: Embedding Model selection
fn step_embedding(state: &mut SetupState) -> Result<StepResult> {
    print_step_header(&Step::Embedding);

    #[cfg(feature = "alcove-full")]
    {
        // Check current config
        let current_model = state.embedding_section.as_ref()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("model = ").map(|v| v.trim_matches('"')))
            })
            .unwrap_or_else(|| {
                load_config()
                    .embedding
                    .as_ref()
                    .map(|e| e.model.as_str())
                    .unwrap_or("MultilingualE5Small")
            });

        loop {
            // Add back and skip options
            let mut labels = vec![
                style("← Go back").yellow().to_string(),
                style("Skip for now (BM25 only)").dim().to_string(),
            ];

            let model_labels: Vec<String> = EMBEDDING_OPTIONS
                .iter()
                .map(|(name, desc, dim, size)| {
                    let marker = if *name == current_model { " [current]" } else { "" };
                    if *size == 0 {
                        format!("{} — {}{}", name, desc, marker)
                    } else {
                        format!("{} — {} ({}d){}", name, desc, dim, marker)
                    }
                })
                .collect();
            labels.extend(model_labels);

            let default_idx = EMBEDDING_OPTIONS
                .iter()
                .position(|(name, _, _, _)| *name == current_model)
                .map(|i| i + 2) // +2 for back and skip options
                .unwrap_or(2);

            let idx = Select::with_theme(&ColorfulTheme::default())
                .with_prompt("Select embedding model for hybrid search")
                .items(&labels)
                .default(default_idx)
                .interact()?;

            // Back
            if idx == 0 {
                return Ok(StepResult::Back);
            }

            // Skip
            if idx == 1 {
                println!(
                    "  {} Embedding skipped. Search will use BM25 only.",
                    style("✓").green()
                );
                state.embedding_section = None;
                return Ok(StepResult::Continue);
            }

            let real_idx = idx - 2;
            let (model_name, _, _, _) = EMBEDDING_OPTIONS[real_idx];

            if model_name == "disabled" {
                println!(
                    "  {} Embedding disabled. Search will use BM25 only.",
                    style("✓").green()
                );
                state.embedding_section = Some("\n[embedding]\nenabled = false\n".to_string());
                return Ok(StepResult::Continue);
            }

            // Ask about auto-download
            let auto_labels = vec![
                style("← Go back").yellow().to_string(),
                "Yes (recommended)".to_string(),
                "No — manual download only".to_string(),
            ];

            let auto_idx = Select::with_theme(&ColorfulTheme::default())
                .with_prompt("Download model automatically on first search?")
                .items(&auto_labels)
                .default(1)
                .interact()?;

            if auto_idx == 0 {
                // Go back to model selection via loop
                continue;
            }

            let auto_download = auto_idx == 1;

            println!(
                "  {} Model: {} (will download on first search)",
                style("✓").green(),
                model_name
            );

            let default_cache_dir = dirs::home_dir()
                .map(|p| p.join(".alcove").join("models").to_string_lossy().to_string())
                .unwrap_or_else(|| "~/.alcove/models".to_string());

            state.embedding_section = Some(format!(
                "\n[embedding]\nmodel = \"{}\"\nauto_download = {}\n# cache_dir = \"{}\"  # default, uncomment to override\nenabled = true\n",
                model_name, auto_download, default_cache_dir
            ));

            return Ok(StepResult::Continue);
        }
    }

    #[cfg(not(feature = "alcove-full"))]
    {
        // Add back option for non-full feature
        let labels = vec![
            style("← Go back").yellow().to_string(),
            style("Continue (BM25 only)").green().to_string(),
        ];

        let idx = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Embedding options")
            .items(&labels)
            .default(1)
            .interact()?;

        if idx == 0 {
            return Ok(StepResult::Back);
        }

        println!(
            "  {} Embedding search (hybrid RAG) requires the full installation.",
            style("ℹ").yellow()
        );
        println!();
        println!("  To enable hybrid search later, reinstall with:");
        println!(
            "  {} cargo install alcove --features alcove-full",
            style("→").cyan()
        );
        println!();
        println!(
            "  {} Search will use BM25 (keyword) only for now.",
            style("✓").green()
        );
        
        state.embedding_section = None;
        Ok(StepResult::Continue)
    }
}

// ---------------------------------------------------------------------------
// Server configuration
// ---------------------------------------------------------------------------

/// Server host options for setup wizard
const SERVER_HOST_OPTIONS: &[(&str, &str)] = &[
    ("127.0.0.1", "Localhost only (default, secure)"),
    ("0.0.0.0", "All interfaces (allows remote access)"),
];

/// Step 5: Server configuration (HTTP RAG server)
fn step_server(state: &mut SetupState) -> Result<StepResult> {
    print_step_header(&Step::Server);

    // Resolve current values: existing state > config.toml > defaults
    let (current_host, current_port) = state.server_section.as_ref()
        .and_then(|s| {
            let host = s.lines()
                .find_map(|l| l.strip_prefix("host = ").map(|v| v.trim_matches('"').to_string()));
            let port = s.lines()
                .find_map(|l| l.strip_prefix("port = ").map(|v| v.trim().to_string()));
            host.zip(port)
        })
        .unwrap_or_else(|| {
            let cfg = load_fresh_config();
            cfg.as_ref()
                .and_then(|c| c.server.as_ref())
                .map(|s| (s.host.clone(), s.port.to_string()))
                .unwrap_or_else(|| ("127.0.0.1".to_string(), "8080".to_string()))
        });

    loop {
        // ── Host selection ──
        let mut host_labels = vec![style("← Go back").yellow().to_string()];
        host_labels.extend(
            SERVER_HOST_OPTIONS
                .iter()
                .map(|(addr, desc)| {
                    let marker = if *addr == current_host { " [current]" } else { "" };
                    format!("{} — {}{}", addr, desc, marker)
                })
                .collect::<Vec<_>>(),
        );

        let host_default = SERVER_HOST_OPTIONS
            .iter()
            .position(|(k, _)| *k == current_host)
            .map(|i| i + 1) // +1 for back option
            .unwrap_or(1);

        let host_idx = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Select bind address")
            .items(&host_labels)
            .default(host_default)
            .interact()?;

        if is_back_selection(host_idx) {
            return Ok(StepResult::Back);
        }

        let selected_host = SERVER_HOST_OPTIONS[host_idx - 1].0;

        // ── Port input ──
        println!("{}", style("  (Press Enter to confirm, or '..' to go back)").dim());
        let port_input: String = Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Listen port")
            .default(current_port.clone())
            .interact_text()?;

        if port_input.trim() == ".." {
            continue; // Re-start server config loop
        }

        let port: u16 = match port_input.trim().parse() {
            Ok(p) => p,
            Err(_) => {
                println!("  {} Invalid port number. Please enter 1-65535.", style("⚠").yellow());
                continue;
            }
        };

        if port == 0 {
            println!("  {} Port 0 is not valid. Please enter 1-65535.", style("⚠").yellow());
            continue;
        }

        state.server_section = Some(format!(
            "\n[server]\nhost = \"{}\"\nport = {}\n",
            selected_host, port
        ));

        println!(
            "  {} Server will bind to {}:{}",
            style("✓").green(),
            selected_host,
            port
        );

        // ── Enable as background service ──
        #[cfg(all(feature = "alcove-server", target_os = "macos"))]
        {
            let already_enabled = crate::launchd::is_loaded();
            let enable_labels = vec![
                style("← Go back").yellow().to_string(),
                if already_enabled {
                    "Keep current (already registered)".to_string()
                } else {
                    "Yes — register as login item and start now".to_string()
                },
                "No — I'll run `alcove serve` manually".to_string(),
            ];

            let enable_default = if already_enabled { 1 } else { 2 };

            let enable_idx = Select::with_theme(&ColorfulTheme::default())
                .with_prompt("Register alcove serve as a macOS login item?")
                .items(&enable_labels)
                .default(enable_default)
                .interact()?;

            if is_back_selection(enable_idx) {
                continue; // Re-start server config loop
            }

            state.enable_server = enable_idx == 1;

            if state.enable_server {
                println!(
                    "  {} Background service will be registered in Summary step.",
                    style("✓").green()
                );
            }
        }

        #[cfg(not(all(feature = "alcove-server", target_os = "macos")))]
        {
            println!(
                "  {} Start manually with: alcove serve --host {} --port {}",
                style("ℹ").dim(),
                selected_host,
                port
            );
        }

        return Ok(StepResult::Continue);
    }
}

/// Step 6: Agent selection
fn step_agents(state: &mut SetupState) -> Result<StepResult> {
    print_step_header(&Step::Agents);

    let agent_list = agents();
    let names: Vec<&str> = agent_list.iter().map(|a| a.name).collect();

    loop {
        // Pre-select previously selected agents or none
        let defaults: Vec<bool> = if state.selected_agents.is_empty() {
            vec![false; names.len()]
        } else {
            names.iter()
                .enumerate()
                .map(|(i, _)| state.selected_agents.contains(&i))
                .collect()
        };

        let selected = MultiSelect::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("{} (select at least 1)", t!("setup.agents_prompt").as_ref()))
            .items(&names)
            .defaults(&defaults)
            .interact()?;

        // Validate at least one selected
        if selected.is_empty() {
            println!(
                "  {} Please select at least one agent to continue.",
                style("⚠").yellow()
            );
            // Re-prompt via loop
            continue;
        }

        state.selected_agents = selected;

        for &idx in &state.selected_agents {
            let agent = &agent_list[idx];
            println!("  {} {}", style("✓").green(), agent.name);
        }

        return Ok(StepResult::Continue);
    }
}

/// Step 6: Summary and finalization
fn step_summary(state: &mut SetupState) -> Result<StepResult> {
    print_step_header(&Step::Summary);

    let docs_root = state.docs_root.clone().unwrap_or_else(default_docs_root);
    let diagram_format = state.diagram_format.clone().unwrap_or_else(|| "mermaid".to_string());

    // Save config
    save_full_config(
        &docs_root,
        &diagram_format,
        &state.core_files,
        &state.team_files,
        &state.public_files,
        state.embedding_section.as_deref(),
        state.server_section.as_deref(),
    )?;

    // Install agent configs
    let agent_list = agents();
    let bin = binary_path();

    for &idx in &state.selected_agents {
        let agent = &agent_list[idx];
        println!();
        println!("  {}", style(agent.name).cyan());

        // MCP
        match &agent.mcp_config {
            McpConfig::Json { path, server_key } => {
                let p = expand_path(path);
                write_json_mcp(&p, server_key, &bin, &docs_root)?;
                println!(
                    "  {} {}",
                    style("✓").green(),
                    t!("setup.mcp_set", path = path)
                );
            }
            McpConfig::OpenCode { path } => {
                let p = expand_path(path);
                write_opencode_mcp(&p, &bin, &docs_root)?;
                println!(
                    "  {} {}",
                    style("✓").green(),
                    t!("setup.mcp_set", path = path)
                );
            }
            McpConfig::Codex { path } => {
                let p = expand_path(path);
                write_codex_mcp(&p, &bin, &docs_root)?;
                println!(
                    "  {} {}",
                    style("✓").green(),
                    t!("setup.mcp_set", path = path)
                );
            }
        }

        // Skill
        if let Some(skill_path) = agent.skill_dir {
            let p = expand_path(skill_path);
            install_skill_to(&p)?;
            println!(
                "  {} {}",
                style("✓").green(),
                t!("setup.skill_set", path = skill_path)
            );
        }
    }

    // Enable background service if requested
    #[cfg(all(feature = "alcove-server", target_os = "macos"))]
    if state.enable_server {
        println!();
        println!("  {}", style("Background Service").cyan());
        match crate::launchd::enable() {
            Ok(()) => {}
            Err(e) => {
                println!(
                    "  {} Failed to register login item: {}",
                    style("⚠").yellow(),
                    e
                );
                println!(
                    "  {} You can run `alcove enable` manually later.",
                    style("ℹ").dim()
                );
            }
        }
    }

    // Print summary
    println!();
    println!("{}", style("── Configuration Summary ──").bold());
    println!("  {}", t!("setup.binary", path = binary_path().display()));
    println!("  {}", t!("setup.config", path = config_path().display()));
    println!("  {}", t!("setup.docs", path = docs_root.display()));

    // Show embedding status
    match &state.embedding_section {
        Some(toml_section) => {
            let model = toml_section
                .lines()
                .find_map(|l| l.strip_prefix("model = ").map(|v| v.trim_matches('"')));
            if let Some(model_name) = model {
                #[cfg(feature = "alcove-full")]
                {
                    let m = crate::embedding::EmbeddingModelChoice::parse(model_name);
                    println!(
                        "  Embedding: {} ({}d, ~{}MB)",
                        model_name,
                        m.map(|m| m.dimension()).unwrap_or(384),
                        m.map(|m| m.size_mb()).unwrap_or(235)
                    );
                }
                #[cfg(not(feature = "alcove-full"))]
                {
                    println!("  Embedding: {} (configured)", model_name);
                }
            } else if toml_section.contains("enabled = false") {
                println!("  Embedding: disabled");
            }
        }
        None => {
            println!("  Embedding: not configured (BM25 only)");
        }
    }

    // Show server status
    match &state.server_section {
        Some(section) => {
            let host = section.lines()
                .find_map(|l| l.strip_prefix("host = ").map(|v| v.trim_matches('"')));
            let port = section.lines()
                .find_map(|l| l.strip_prefix("port = ").map(|v| v.trim()));

            #[cfg(all(feature = "alcove-server", target_os = "macos"))]
            {
                let service_status = if state.enable_server {
                    "enabled (login item)"
                } else {
                    "manual only"
                };
                println!(
                    "  Server: {}:{} ({})",
                    host.unwrap_or("127.0.0.1"),
                    port.unwrap_or("8080"),
                    service_status
                );
            }
            #[cfg(not(all(feature = "alcove-server", target_os = "macos")))]
            {
                println!(
                    "  Server: {}:{}",
                    host.unwrap_or("127.0.0.1"),
                    port.unwrap_or("8080")
                );
            }
        }
        None => {
            println!("  Server: default (127.0.0.1:8080)");
        }
    }

    println!();
    println!("  {}", style(t!("setup.hint_update").to_string()).dim());
    println!("  {}", style(t!("setup.hint_uninstall").to_string()).dim());
    println!();

    Ok(StepResult::Continue)
}

// ---------------------------------------------------------------------------
// Diagram format selection
// ---------------------------------------------------------------------------

const DIAGRAM_FORMATS: &[(&str, &str)] = &[
    ("mermaid", "Mermaid — GitHub/GitLab native, most popular"),
    (
        "plantuml",
        "PlantUML — Enterprise UML, richest diagram types",
    ),
    ("d2", "D2 — Modern, clean rendering, Go-based"),
    ("ascii", "ASCII art — Universal, no renderer needed"),
    ("graphviz", "Graphviz (DOT) — Classic graph visualization"),
    (
        "structurizr",
        "Structurizr (C4) — Architecture-focused C4 model",
    ),
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
    println!();
    println!(
        "  {} Use arrow keys to navigate, Enter to select. Type '..' to go back.",
        style("ℹ").dim()
    );

    let mut state = SetupState::default();
    let mut current_step = Step::DocsRoot;

    loop {
        let result = match current_step {
            Step::DocsRoot => step_docs_root(&mut state)?,
            Step::Categories => step_categories(&mut state)?,
            Step::Diagram => step_diagram(&mut state)?,
            Step::Embedding => step_embedding(&mut state)?,
            Step::Server => step_server(&mut state)?,
            Step::Agents => step_agents(&mut state)?,
            Step::Summary => {
                step_summary(&mut state)?;
                break; // Done!
            }
        };

        match result {
            StepResult::Continue => {
                if let Some(next) = current_step.next() {
                    current_step = next;
                } else {
                    break;
                }
            }
            StepResult::Back => {
                if let Some(prev) = current_step.prev() {
                    current_step = prev;
                }
            }
        }
    }

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
        "~/.cline/skills/alcove",
        "~/.opencode/skills/alcove",
        "~/.codex/skills/alcove",
        "~/.copilot/skills/alcove",
        "~/.gemini/skills/alcove",
    ];
    for d in &skill_dirs {
        let p = expand_path(d);
        if p.exists() {
            fs::remove_dir_all(&p)?;
            println!(
                "  {} {}",
                style("✓").green(),
                t!("uninstall.removed_skill", path = d)
            );
        }
    }

    // Config
    let cfg = config_path();
    if cfg.exists() {
        fs::remove_file(&cfg)?;
        println!(
            "  {} {}",
            style("✓").green(),
            t!("uninstall.removed_config", path = cfg.display())
        );
    }
    // Legacy config
    let legacy = cfg.with_file_name("config");
    if legacy.exists() {
        fs::remove_file(&legacy)?;
        println!(
            "  {} {}",
            style("✓").green(),
            t!("uninstall.removed_legacy", path = legacy.display())
        );
    }

    println!();
    println!(
        "  {}",
        style(t!("uninstall.binary_hint").to_string()).yellow()
    );
    println!();
    println!("  {}", t!("uninstall.mcp_hint"));
    println!("    Claude Code:    ~/.claude.json");
    println!("    Cursor:         ~/.cursor/mcp.json");
    println!("    Claude Desktop: ~/Library/Application Support/Claude/claude_desktop_config.json");
    println!(
        "    Cline:          ~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json"
    );
    println!("    OpenCode:       ~/.config/opencode/opencode.json");
    println!("    Codex:          ~/.codex/config.toml");
    println!("    Copilot CLI:    ~/.copilot/mcp-config.json");
    println!("    Antigravity:    ~/.gemini/antigravity/mcp_config.json");
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
    let (pol, results) = policy::validate(&docs_root, &resolved.name, repo_path);

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
        style(format!(
            "Document Policy: {} (source: {source})",
            pol.policy.enforce
        ))
        .bold()
    );
    println!("{}", style(format!("Project: {project_name}")).dim());
    println!();

    for r in results {
        let icon = match r.status {
            FileStatus::Pass => style("  PASS").green(),
            FileStatus::Warn => style("  WARN").yellow(),
            FileStatus::Fail => style("  FAIL").red(),
        };
        let reason = r
            .reason
            .as_deref()
            .map(|s| format!(" — {s}"))
            .unwrap_or_default();
        println!("{icon} {}{reason}", r.file);

        for s in &r.sections {
            let sec_icon = match s.status {
                FileStatus::Pass => style("    PASS").green(),
                FileStatus::Warn => style("    WARN").yellow(),
                FileStatus::Fail => style("    FAIL").red(),
            };
            let detail = s
                .detail
                .as_deref()
                .map(|d| format!(" ({d})"))
                .unwrap_or_default();
            println!("{sec_icon} {}{detail}", s.heading);
        }
    }

    let pass = results
        .iter()
        .filter(|r| r.status == FileStatus::Pass)
        .count();
    let warn = results
        .iter()
        .filter(|r| r.status == FileStatus::Warn)
        .count();
    let fail = results
        .iter()
        .filter(|r| r.status == FileStatus::Fail)
        .count();

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

    print_index_result(crate::index::build_index(&docs_root)?, false)
}


// ---------------------------------------------------------------------------
// alcove rebuild
// ---------------------------------------------------------------------------

pub fn cmd_rebuild() -> Result<()> {
    let docs_root = match saved_docs_root() {
        Some(p) => p,
        None => {
            anyhow::bail!("docs_root is not configured. Run `alcove setup` first.");
        }
    };

    print_index_result(crate::index::rebuild_index(&docs_root)?, true)
}

// ---------------------------------------------------------------------------
// Shared index result printer
// ---------------------------------------------------------------------------

fn print_index_result(result: serde_json::Value, is_rebuild: bool) -> Result<()> {
    let projects = result["projects"].as_u64().unwrap_or(0);
    let indexed   = result["indexed"].as_u64().unwrap_or(0);
    let skipped   = result["skipped"].as_u64().unwrap_or(0);
    let index_path = result["index_path"].as_str().unwrap_or("unknown");

    // Header line
    let label = if is_rebuild { "Rebuilt" } else { "Indexed" };
    if indexed == 0 && skipped > 0 {
        println!(
            "  {} already up to date  {} projects  {} files",
            style("✓").green(),
            style(projects).bold(),
            style(skipped).dim(),
        );
    } else {
        println!(
            "  {} {}  {} projects  {} files",
            style("✓").green(),
            style(label).bold(),
            style(projects).bold(),
            style(indexed).bold(),
        );
        if skipped > 0 {
            println!("  {} {} unchanged", style("·").dim(), style(skipped).dim());
        }
    }

    // Vector status
    let vector_status = result["vector_status"].as_str().unwrap_or("disabled");
    match vector_status {
        "ok" => {
            let vectors = result["vectors_indexed"].as_u64().unwrap_or(0);
            let errors  = result["vector_errors"].as_u64().unwrap_or(0);
            let model   = result["embedding_model"].as_str().unwrap_or("unknown");
            println!(
                "  {} {} vectors  {}",
                style("✓").green(),
                style(vectors).bold(),
                style(model).dim(),
            );
            if errors > 0 {
                println!("  {} {} embedding error(s)", style("!").yellow(), errors);
            }
        }
        "model_not_ready" => {
            let status = result["embedding_status"].as_str().unwrap_or("unknown");
            println!(
                "  {} hybrid search unavailable  {}",
                style("·").dim(),
                style(status).dim(),
            );
        }
        "failed" => {
            let err = result["vector_error"].as_str().unwrap_or("unknown");
            println!("  {} vector indexing failed: {}", style("✗").red(), err);
        }
        _ => {} // disabled — silent
    }

    println!("  {} {}", style("·").dim(), style(index_path).dim());
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
        let score = m.get("score").and_then(serde_json::Value::as_f64);

        let location = if let Some(proj) = project {
            format!("{}:{}", style(proj).green(), style(file).cyan())
        } else {
            style(file).cyan().to_string()
        };

        let line_info = line
            .and_then(serde_json::Value::as_u64)
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
        println!(
            "{}",
            style("  (results truncated, use --limit to see more)").dim()
        );
    }

    Ok(())
}

fn run_grep_search(
    docs_root: &std::path::Path,
    query: &str,
    scope: &str,
    limit: usize,
) -> Result<serde_json::Value> {
    if scope == "global" {
        crate::tools::tool_search_global(
            docs_root,
            serde_json::json!({"query": query, "scope": "global", "limit": limit}),
        )
    } else {
        let resolved = crate::tools::resolve_project(docs_root);
        match resolved {
            Some(r) => {
                let project_root = docs_root.join(&r.name);
                crate::tools::tool_search(
                    &project_root,
                    serde_json::json!({"query": query, "limit": limit}),
                    r.repo_path.as_deref(),
                )
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
// alcove doctor
// ---------------------------------------------------------------------------

pub fn cmd_doctor(format: &str) -> Result<()> {
    let mut checks: Vec<serde_json::Value> = Vec::new();

    // 1. Config file
    let cfg_path = config_path();
    let (cfg_status, cfg_msg) = if cfg_path.exists() {
        match fs::read_to_string(&cfg_path) {
            Ok(content) => match toml::from_str::<toml::Value>(&content) {
                Ok(_) => ("ok", t!("doctor.config_valid", path = cfg_path.display()).to_string()),
                Err(e) => ("error", t!("doctor.config_parse_error", error = e).to_string()),
            },
            Err(e) => ("error", t!("doctor.config_read_error", error = e).to_string()),
        }
    } else {
        ("warn", t!("doctor.config_not_found").to_string())
    };
    checks.push(serde_json::json!({
        "check": "config",
        "status": cfg_status,
        "message": cfg_msg,
    }));

    // 2. docs_root
    let docs_root = saved_docs_root();
    let (dr_status, dr_msg, dr_path) = match &docs_root {
        Some(p) if p.is_dir() => ("ok", format!("{}", p.display()), Some(p.clone())),
        Some(p) => (
            "error",
            t!("doctor.docs_root_not_exists", path = p.display()).to_string(),
            None,
        ),
        None => ("error", t!("doctor.docs_root_missing").to_string(), None),
    };
    checks.push(serde_json::json!({
        "check": "docs_root",
        "status": dr_status,
        "message": dr_msg,
    }));

    // 3. Projects
    let mut project_names: Vec<String> = Vec::new();
    if let Some(root) = &dr_path {
        if let Ok(entries) = fs::read_dir(root) {
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
                if !name.starts_with('.') && !name.starts_with('_') && name != "mcp" && name != "skills" {
                    project_names.push(name);
                }
            }
        }
        project_names.sort();
    }
    let proj_status = if project_names.is_empty() {
        "warn"
    } else {
        "ok"
    };
    checks.push(serde_json::json!({
        "check": "projects",
        "status": proj_status,
        "message": t!("doctor.projects_count", count = project_names.len()).to_string(),
        "details": project_names,
    }));

    // 4. Agent registration
    let mut agent_details: Vec<serde_json::Value> = Vec::new();
    for agent in agents() {
        let (status, msg) = check_agent_registration(&agent);
        agent_details.push(serde_json::json!({
            "name": agent.name,
            "status": status,
            "message": msg,
        }));
    }
    let registered = agent_details
        .iter()
        .filter(|a| a["status"] == "ok")
        .count();
    let agent_status = if registered > 0 { "ok" } else { "warn" };
    checks.push(serde_json::json!({
        "check": "agents",
        "status": agent_status,
        "message": t!("doctor.agents_count", registered = registered, total = agent_details.len()).to_string(),
        "details": agent_details,
    }));

    // 5. Search index
    let (idx_status, idx_msg) = if let Some(root) = &dr_path {
        if crate::index::index_exists(root) {
            if crate::index::is_index_stale(root) {
                ("warn", t!("doctor.index_stale").to_string())
            } else {
                ("ok", t!("doctor.index_fresh").to_string())
            }
        } else {
            ("warn", t!("doctor.index_none").to_string())
        }
    } else {
        ("error", t!("doctor.index_no_root").to_string())
    };
    checks.push(serde_json::json!({
        "check": "index",
        "status": idx_status,
        "message": idx_msg,
    }));

    // 6. PDF support (pdftotext)
    let (pdf_status, pdf_msg) = if std::process::Command::new("pdftotext")
        .arg("-v")
        .output()
        .is_ok()
    {
        ("ok", t!("doctor.pdftotext_available").to_string())
    } else {
        ("warn", t!("doctor.pdftotext_missing").to_string())
    };
    checks.push(serde_json::json!({
        "check": "pdftotext",
        "status": pdf_status,
        "message": pdf_msg,
    }));

    // 7. Binary
    let bin_path = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    checks.push(serde_json::json!({
        "check": "binary",
        "status": "ok",
        "message": bin_path,
    }));

    // Output
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&checks)?);
    } else {
        print_doctor_human(&checks);
    }

    Ok(())
}

fn check_agent_registration(agent: &AgentDef) -> (&'static str, String) {
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
        McpConfig::Codex { .. } => content.contains("[mcp_servers.alcove]"),
    };

    if has_alcove {
        ("ok", t!("doctor.agent_registered").to_string())
    } else {
        ("error", t!("doctor.agent_not_registered", path = path).to_string())
    }
}

fn print_doctor_human(checks: &[serde_json::Value]) {
    println!();
    println!("{}", style(t!("doctor.title").to_string()).bold());
    println!();

    for check in checks {
        let name = check["check"].as_str().unwrap_or("");
        let status = check["status"].as_str().unwrap_or("");
        let msg = check["message"].as_str().unwrap_or("");

        let icon = match status {
            "ok" => style("  ✅").green(),
            "warn" => style("  ⚠️ ").yellow(),
            "error" => style("  ❌").red(),
            "skip" => style("  ⏭️ ").dim(),
            _ => style("  ?").dim(),
        };

        let label_key = format!("doctor.{name}");
        let label_translated = t!(&label_key);
        let label = label_translated.as_ref();

        println!("{icon} {}: {msg}", style(label).bold());

        // Show details for projects and agents
        if name == "projects"
            && let Some(details) = check["details"].as_array()
        {
            for d in details {
                if let Some(s) = d.as_str() {
                    println!("       {}", style(s).dim());
                }
            }
        }
        if name == "agents"
            && let Some(details) = check["details"].as_array()
        {
            for d in details {
                let aname = d["name"].as_str().unwrap_or("");
                let astatus = d["status"].as_str().unwrap_or("");
                let amsg = d["message"].as_str().unwrap_or("");
                let aicon = match astatus {
                    "ok" => style("✅").green(),
                    "error" => style("❌").red(),
                    "skip" => style("⏭️ ").dim(),
                    _ => style("?").dim(),
                };
                println!("       {aicon} {aname}: {}", style(amsg).dim());
            }
        }
    }

    println!();
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
    embedding_section: Option<&str>,
    server_section: Option<&str>,
) -> Result<()> {
    let cfg_path = config_path();
    save_full_config_to(
        &cfg_path,
        docs_root,
        diagram_format,
        core_files,
        team_files,
        public_files,
        embedding_section,
        server_section,
    )?;
    println!(
        "  {} {}",
        style("✓").green(),
        t!("setup.config_saved", path = cfg_path.display())
    );
    Ok(())
}

fn save_full_config_to(
    cfg_path: &Path,
    docs_root: &Path,
    diagram_format: &str,
    core_files: &[String],
    team_files: &[String],
    public_files: &[String],
    embedding_section: Option<&str>,
    server_section: Option<&str>,
) -> Result<()> {
    fs::create_dir_all(cfg_path.parent().unwrap())?;

    let fmt_list = |files: &[String]| -> String {
        files
            .iter()
            .map(|f| format!("\"{}\"", f))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut content = format!(
        "docs_root = \"{}\"\n\n[core]\nfiles = [{}]\n\n[team]\nfiles = [{}]\n\n[public]\nfiles = [{}]\n\n[diagram]\nformat = \"{}\"\n",
        docs_root.display(),
        fmt_list(core_files),
        fmt_list(team_files),
        fmt_list(public_files),
        diagram_format,
    );

    if let Some(section) = embedding_section {
        content.push_str(section);
    }

    if let Some(section) = server_section {
        content.push_str(section);
    }

    fs::write(cfg_path, content)?;
    Ok(())
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Model subcommand (alcove-full feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "alcove-full")]
pub fn cmd_model(subcmd: crate::ModelCommands) -> Result<()> {
    use crate::ModelCommands;

    match subcmd {
        ModelCommands::List => cmd_model_list(),
        ModelCommands::Download => cmd_model_download(),
        ModelCommands::Remove => cmd_model_remove(),
        ModelCommands::Set { model } => cmd_model_set(&model),
        ModelCommands::Status => cmd_model_status(),
    }
}

#[cfg(feature = "alcove-full")]
fn cmd_model_list() -> Result<()> {
    use crate::embedding::EmbeddingModelChoice;
    
    println!("{}", style("Available embedding models:").bold());
    println!();
    println!(
        "{:<30} {:<8} {:<10} {}",
        style("Model").bold(),
        style("Dim").bold(),
        style("Size").bold(),
        style("Description").bold()
    );
    println!("{}", "-".repeat(80));

    let current = load_config()
        .embedding
        .as_ref()
        .map(|e| e.model.as_str())
        .unwrap_or("MultilingualE5Small");

    for model in EmbeddingModelChoice::all() {
        let marker = if model.as_str() == current { " [current]" } else { "" };
        let desc = match model {
            EmbeddingModelChoice::SnowflakeArcticEmbedXS => "Mobile/low-spec, fastest",
            EmbeddingModelChoice::SnowflakeArcticEmbedXSQ => "Quantized XS, smallest",
            EmbeddingModelChoice::MultilingualE5Small => "Default, balanced (100+ langs)",
            EmbeddingModelChoice::SnowflakeArcticEmbedS => "Quality/size balance",
            EmbeddingModelChoice::SnowflakeArcticEmbedSQ => "Quantized S",
            EmbeddingModelChoice::MultilingualE5Base => "Large scale docs",
            EmbeddingModelChoice::SnowflakeArcticEmbedM => "Medium, 768d",
            EmbeddingModelChoice::SnowflakeArcticEmbedMQ => "Quantized M",
            EmbeddingModelChoice::MultilingualE5Large => "Best quality, heavy",
            EmbeddingModelChoice::BGEM3 => "Dense+Sparse+ColBERT",
        };
        println!(
            "{:<30} {:<8} {:<10} {}{}",
            model.as_str(),
            model.dimension(),
            format!("~{}MB", model.size_mb()),
            desc,
            style(marker).cyan()
        );
    }

    println!();
    println!("Change model: alcove model set <ModelName>");
    println!("Check status: alcove model status");

    Ok(())
}

#[cfg(feature = "alcove-full")]
fn cmd_model_download() -> Result<()> {
    #[cfg(feature = "alcove-full")]
    {
        use crate::embedding::EmbeddingService;
        
        let cfg = load_config().embedding_config_with_defaults();
        let service = EmbeddingService::new(crate::config::EmbeddingConfig {
            model: crate::embedding::EmbeddingModelChoice::parse(&cfg.model).unwrap_or_default().as_str().to_string(),
            auto_download: true,
            cache_dir: cfg.cache_dir.starts_with('~')
                .then(|| std::env::var("HOME").ok())
                .flatten()
                .map(|h| cfg.cache_dir.replacen('~', &h, 1))
                .unwrap_or_else(|| cfg.cache_dir.clone()),
            enabled: true,
            query_cache_size: cfg.query_cache_size,
        });

        println!(
            "{} Downloading embedding model: {}",
            style("⏳").yellow(),
            style(&cfg.model).cyan()
        );
        println!("This may take a few minutes on first run...");
        
        let pb = indicatif::ProgressBar::new_spinner();
        pb.set_style(indicatif::ProgressStyle::default_spinner().template("{spinner:.green} {msg}")?);
        pb.set_message("Downloading and loading model...");
        pb.enable_steady_tick(std::time::Duration::from_millis(100));

        service.ensure_model().map_err(|e| anyhow::anyhow!("{}", e))?;
        
        pb.finish_and_clear();

        println!(
            "{} Model downloaded and ready: {}",
            style("✓").green(),
            style(&cfg.model).cyan()
        );
        println!("Cache location: {}", cfg.cache_dir);
    }

    #[cfg(not(feature = "alcove-full"))]
    {
        println!(
            "{} The 'alcove-full' feature is required for embedding support.",
            style("✗").red()
        );
        println!("Install with: cargo install alcove --features alcove-full");
    }

    Ok(())
}

#[cfg(feature = "alcove-full")]
fn cmd_model_remove() -> Result<()> {
    let cfg = load_config().embedding_config_with_defaults();
    let cache_dir = std::path::PathBuf::from(
        cfg.cache_dir.starts_with('~')
            .then(|| std::env::var("HOME").ok())
            .flatten()
            .map(|h| cfg.cache_dir.replacen('~', &h, 1))
            .unwrap_or_else(|| cfg.cache_dir.clone())
    );

    let model_dir = cache_dir.join(&cfg.model);

    if model_dir.exists() {
        // Calculate size before removal
        let size_mb: u64 = walkdir::WalkDir::new(&model_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum::<u64>()
            / 1_000_000;

        fs::remove_dir_all(&model_dir)?;
        println!(
            "{} Removed model cache: {} (freed ~{}MB)",
            style("✓").green(),
            style(&cfg.model).cyan(),
            size_mb
        );
    } else {
        println!(
            "{} No cached model found at: {}",
            style("!").yellow(),
            model_dir.display()
        );
    }

    Ok(())
}

#[cfg(feature = "alcove-full")]
fn cmd_model_set(model_name: &str) -> Result<()> {
    use crate::embedding::EmbeddingModelChoice;
    
    // Validate model name
    let model = EmbeddingModelChoice::parse(model_name)
        .ok_or_else(|| anyhow::anyhow!("Unknown model: {}. Run 'alcove model list' to see available models.", model_name))?;

    // Read current config
    let config_file = config_path();
    let mut content = if config_file.exists() {
        fs::read_to_string(&config_file)?
    } else {
        String::new()
    };

    // Update or add [embedding] section
    let embedding_section = format!(
        "[embedding]\nmodel = \"{}\"\nauto_download = true\n",
        model_name
    );

    if content.contains("[embedding]") {
        // Replace existing embedding section
        let start = content.find("[embedding]").unwrap();
        let end = content[start..]
            .find("\n[")
            .map(|i| start + i)
            .unwrap_or(content.len());
        
        // Find the actual end of the embedding section (before next section or EOF)
        let section_end = content[start..].find("\n\n[").map(|i| start + i).unwrap_or(end);
        
        content = format!("{}{}{}", &content[..start], embedding_section.trim_end(), &content[section_end..]);
    } else {
        // Add new embedding section
        if !content.ends_with('\n') && !content.is_empty() {
            content.push('\n');
        }
        content.push('\n');
        content.push_str(&embedding_section);
    }

    fs::write(&config_file, content)?;

    println!(
        "{} Model set to: {} ({}d, ~{}MB)",
        style("✓").green(),
        style(model_name).cyan(),
        model.dimension(),
        model.size_mb()
    );
    println!();
    println!("Run 'alcove model download' to download the model.");
    println!("Run 'alcove index' to rebuild the vector index with the new model.");

    Ok(())
}

#[cfg(feature = "alcove-full")]
fn cmd_model_status() -> Result<()> {
    let cfg = load_config();
    let emb_cfg = cfg.embedding_config_with_defaults();

    println!("{}", style("Embedding Model Status").bold());
    println!("{}", "-".repeat(40));
    println!(
        "{:<20} {}",
        style("Configured model:").dim(),
        style(&emb_cfg.model).cyan()
    );
    
    let model_choice = crate::embedding::EmbeddingModelChoice::parse(&emb_cfg.model)
        .unwrap_or_default();

    println!(
        "{:<20} {}d",
        style("Dimension:").dim(),
        model_choice.dimension()
    );
    println!(
        "{:<20} ~{}MB",
        style("Size:").dim(),
        model_choice.size_mb()
    );
    println!(
        "{:<20} {}",
        style("Auto-download:").dim(),
        emb_cfg.auto_download
    );
    println!(
        "{:<20} {}",
        style("Cache dir:").dim(),
        emb_cfg.cache_dir
    );

    let cache_path = expand_path(&emb_cfg.cache_dir);
    let model_id = model_choice.model_id();
    let folder_name = format!("models--{}", model_id.replace('/', "--"));
    let model_dir = cache_path.join(folder_name);

    println!();
    if model_dir.exists() {
        println!(
            "{} Model cached and ready!",
            style("✓").green()
        );
        println!("  Location: {}", model_dir.display());
    } else {
        println!(
            "{} Model not cached locally.",
            style("⏳").yellow()
        );
        println!("  Run 'alcove model download' to prepare for hybrid search.");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// alcove lint
// ---------------------------------------------------------------------------

pub fn cmd_lint(format: &str) -> Result<()> {
    use crate::lint;
    use crate::tools;

    let docs_root = match saved_docs_root() {
        Some(p) => p,
        None => {
            anyhow::bail!("docs_root is not configured. Run `alcove setup` first.");
        }
    };

    let project_filter = tools::resolve_project(&docs_root).map(|r| r.name);
    let report = lint::lint(&docs_root, project_filter.as_deref());

    if format == "json" {
        let json = lint::lint_to_json(&report);
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        let project_label = project_filter.as_deref().unwrap_or("(all projects)");
        lint::print_lint_human(&report, project_label);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// alcove promote
// ---------------------------------------------------------------------------

pub fn cmd_promote(source: &std::path::Path, project: Option<&str>, mv: bool) -> Result<()> {
    use crate::promote;

    let docs_root = match saved_docs_root() {
        Some(p) => p,
        None => {
            anyhow::bail!("docs_root is not configured. Run `alcove setup` first.");
        }
    };

    let opts = promote::PromoteOptions {
        source: source.to_path_buf(),
        project: project.map(|s| s.to_string()),
        copy: !mv,
    };

    let result = promote::promote(&docs_root, opts)?;

    println!(
        "{} {} → {}  (project: {})",
        style(result.action).green().bold(),
        result.source.display(),
        result.destination.display(),
        style(&result.project).cyan(),
    );

    Ok(())
}
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
            assert!(
                !cat.defaults.is_empty(),
                "category '{}' should have defaults",
                cat.label
            );
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
        fs::write(&cfg, serde_json::to_string_pretty(&existing).unwrap()).expect("failed to write");

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

        let result =
            write_opencode_mcp(&cfg, &PathBuf::from("/bin/alcove"), &PathBuf::from("/docs"));
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
        fs::write(&cfg, "docs_root = \"/old/path\"\nother = \"keep\"\n").expect("failed to write");

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

        let result = save_full_config_to(
            &cfg,
            &docs,
            "mermaid",
            &core,
            &team,
            &public,
            None,
            None,
        );
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
