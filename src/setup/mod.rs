use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use console::style;
use dialoguer::{Input, MultiSelect, Select, theme::ColorfulTheme};
use rust_i18n::t;

use crate::config::{
    CategoryConfig, DiagramConfig, DOC_REPO_REQUIRED, DOC_REPO_SUPPLEMENTARY, DocConfig,
    PROJECT_REPO_FILES, config_path, default_docs_root, load_config,
};
use crate::agents::{
    McpConfig, agents, expand_path, install_skill_to,
    write_codex_mcp, write_json_mcp, write_opencode_mcp,
};

// ---------------------------------------------------------------------------
// Resolve docs root
// ---------------------------------------------------------------------------

/// Return saved docs root from env or config.toml, falling back to default.
pub fn saved_docs_root() -> Option<PathBuf> {
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
        let home = dirs::home_dir().expect("Cannot determine home directory");
        format!("{}/{}", home.display(), stripped)
    } else {
        s.to_string()
    }
}

pub(crate) fn save_docs_root(path: &Path) -> Result<()> {
    save_docs_root_to(&config_path(), path)
}

pub(crate) fn save_docs_root_to(cfg_path: &Path, path: &Path) -> Result<()> {
    let parent = cfg_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("config path has no parent directory: {}", cfg_path.display()))?;
    fs::create_dir_all(parent)?;

    if cfg_path.exists() {
        let content = fs::read_to_string(cfg_path)?;
        if content.contains("docs_root") {
            // Update existing
            let updated: String = content
                .lines()
                .map(|l| {
                    if l.trim_start().starts_with("docs_root") {
                        format!("docs_root = {}", toml::Value::String(path.display().to_string()))
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(cfg_path, updated)?;
        } else {
            // Prepend
            let updated = format!("docs_root = {}\n\n{}", toml::Value::String(path.display().to_string()), content);
            fs::write(cfg_path, updated)?;
        }
    } else {
        fs::write(cfg_path, format!("docs_root = {}\n", toml::Value::String(path.display().to_string())))?;
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
// Shell rc seeding
// ---------------------------------------------------------------------------

/// Detect the user's shell rc files to seed environment variables into.
fn detect_shell_rc_files() -> Vec<PathBuf> {
    let home = dirs::home_dir().expect("Cannot determine home directory");
    let mut candidates = vec![
        home.join(".zshrc"),
        home.join(".bashrc"),
        home.join(".bash_profile"),
        home.join(".profile"),
        home.join(".config/fish/config.fish"),
    ];
    // Only return files that already exist — don't create new ones.
    candidates.retain(|p| p.exists());
    // If none exist yet, fall back to the shell inferred from $SHELL.
    if candidates.is_empty() {
        let shell = std::env::var("SHELL").unwrap_or_default();
        if shell.contains("zsh") {
            candidates.push(home.join(".zshrc"));
        } else if shell.contains("fish") {
            candidates.push(home.join(".config/fish/config.fish"));
        } else {
            candidates.push(home.join(".bashrc"));
        }
    }
    candidates
}

/// Append `export ALCOVE_TOKEN=<token>` (or fish `set -gx`) to detected rc files.
/// Skips files that already contain the export. Returns list of files written.
fn seed_token_to_shell_rc(token: &str) -> Vec<PathBuf> {
    let mut written = Vec::new();
    for rc in detect_shell_rc_files() {
        let is_fish = rc.to_string_lossy().contains("config.fish");
        let marker = "ALCOVE_TOKEN";
        let existing = fs::read_to_string(&rc).unwrap_or_default();
        if existing.contains(marker) {
            // Already present — overwrite only the token value.
            let updated: String = existing
                .lines()
                .map(|l| {
                    if l.contains(marker) {
                        if is_fish {
                            format!("set -gx ALCOVE_TOKEN {token}")
                        } else {
                            format!("export ALCOVE_TOKEN={token}")
                        }
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let _ = fs::write(&rc, updated);
        } else {
            let line = if is_fish {
                format!("\n# Added by alcove setup\nset -gx ALCOVE_TOKEN {token}\n")
            } else {
                format!("\n# Added by alcove setup\nexport ALCOVE_TOKEN={token}\n")
            };
            let mut content = existing;
            content.push_str(&line);
            let _ = fs::write(&rc, content);
        }
        written.push(rc);
    }
    written
}

// ---------------------------------------------------------------------------
// Setup wizard state machine
// ---------------------------------------------------------------------------

/// Total number of setup steps (for progress indicator)
const SETUP_STEPS: usize = 8;

/// Setup wizard steps
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Step {
    DocsRoot = 0,
    Categories = 1,
    Diagram = 2,
    Embedding = 3,
    Server = 4,
    Agents = 5,
    Telemetry = 6,
    Summary = 7,
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
            Step::Telemetry => Cow::Borrowed("Telemetry"),
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
            Step::Agents => Some(Step::Telemetry),
            Step::Telemetry => Some(Step::Summary),
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
            Step::Telemetry => Some(Step::Agents),
            Step::Summary => Some(Step::Telemetry),
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
    use_token: bool,
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
                crate::config::DocConfig::core_files,
            )
        } else {
            state.core_files.clone()
        },
        if state.team_files.is_empty() {
            cfg.as_ref().map_or_else(
                || DOC_REPO_SUPPLEMENTARY.iter().map(std::string::ToString::to_string).collect(),
                crate::config::DocConfig::team_files,
            )
        } else {
            state.team_files.clone()
        },
        if state.public_files.is_empty() {
            cfg.as_ref().map_or_else(
                || PROJECT_REPO_FILES.iter().map(std::string::ToString::to_string).collect(),
                crate::config::DocConfig::public_files,
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

            let model_toml = toml::Value::String(model_name.to_string()).to_string();
            let cache_toml = toml::Value::String(default_cache_dir.clone()).to_string();
            state.embedding_section = Some(format!(
                "\n[embedding]\nmodel = {model_toml}\nauto_download = {auto_download}\n# cache_dir = {cache_toml}  # default, uncomment to override\nenabled = true\n"
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
                .unwrap_or_else(|| ("127.0.0.1".to_string(), "57384".to_string()))
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

        // ── Token configuration ──
        let existing_token = state.server_section.as_ref()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("token = ").map(|v| v.trim_matches('"').to_string()))
            })
            .or_else(|| {
                load_fresh_config()
                    .and_then(|c| c.server)
                    .and_then(|s| s.token)
            });

        let is_public = selected_host == "0.0.0.0";

        let token_labels = if is_public {
            vec![
                style("← Go back").yellow().to_string(),
                "Yes — generate token automatically".to_string(),
                "Yes — enter token manually".to_string(),
                // 0.0.0.0 without token is not allowed
            ]
        } else {
            vec![
                style("← Go back").yellow().to_string(),
                "Yes — generate token automatically".to_string(),
                "Yes — enter token manually".to_string(),
                "No — skip (localhost only, not recommended)".to_string(),
            ]
        };

        if is_public {
            println!(
                "  {} 0.0.0.0 binds to all interfaces. A bearer token is required.",
                style("⚠").yellow()
            );
        }

        let token_idx = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Set up a bearer token for authentication?")
            .items(&token_labels)
            .default(1)
            .interact()?;

        if is_back_selection(token_idx) {
            continue;
        }

        // "No" is only available for localhost and is the last option
        let skip_token = !is_public && token_idx == token_labels.len() - 1;

        let token: Option<String> = if skip_token {
            None
        } else if token_idx == 2 {
            // Manual entry
            println!("{}", style("  (Leave blank to auto-generate)").dim());
            let input: String = Input::with_theme(&ColorfulTheme::default())
                .with_prompt("Bearer token")
                .allow_empty(true)
                .interact_text()?;
            let t = if input.trim().is_empty() {
                existing_token.unwrap_or_else(generate_token)
            } else {
                input.trim().to_string()
            };
            Some(t)
        } else {
            // Auto-generate (reuse existing if available)
            Some(existing_token.unwrap_or_else(generate_token))
        };

        state.use_token = token.is_some();

        let host_toml = toml::Value::String(selected_host.to_string()).to_string();
        state.server_section = Some(if let Some(ref t) = token {
            let token_toml = toml::Value::String(t.clone()).to_string();
            format!("\n[server]\nhost = {host_toml}\nport = {port}\ntoken = {token_toml}\n")
        } else {
            format!("\n[server]\nhost = {host_toml}\nport = {port}\n")
        });

        if token.is_some() {
            println!(
                "  {} Token configured. Run {} to view it.",
                style("✓").green(),
                style("alcove token").cyan()
            );
            println!(
                "  {} Token will be exported to your shell rc as {}.",
                style("ℹ").dim(),
                style("ALCOVE_TOKEN").cyan()
            );
        }

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

            let enable_default = 1;

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

/// Step 7: Telemetry consent
fn step_telemetry() -> Result<StepResult> {
    print_step_header(&Step::Telemetry);
    let level = crate::telemetry::prompt_consent_interactive();
    crate::telemetry::write_consent(level);
    match level {
        crate::telemetry::ConsentLevel::On => {
            println!("  {} Telemetry enabled. To opt out later: alcove telemetry off", style("✓").green());
        }
        crate::telemetry::ConsentLevel::Off => {
            println!("  {} Telemetry disabled. To enable later: alcove telemetry on", style("✓").green());
        }
    }
    Ok(StepResult::Continue)
}

/// Step 8: Summary and finalization
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

    // Compute MCP connection mode: enable_server → HTTP direct, else stdio
    let server_url: Option<String> = if state.enable_server {
        state.server_section.as_ref()
            .and_then(|s| {
                let host = s.lines()
                    .find_map(|l| l.strip_prefix("host = ").map(|v| v.trim_matches('"')));
                let port = s.lines()
                    .find_map(|l| l.strip_prefix("port = ").map(|v| v.trim()));
                host.zip(port)
            })
            .map(|(h, p)| format!("http://{}:{}/mcp", h, p))
            .or_else(|| Some("http://127.0.0.1:57384/mcp".to_string()))
    } else {
        None
    };

    // Extract stored token from server_section for shell rc seeding
    let stored_token: Option<String> = state.server_section.as_ref().and_then(|s| {
        s.lines().find_map(|l| l.strip_prefix("token = ").map(|v| v.trim_matches('"').to_string()))
    });

    for &idx in &state.selected_agents {
        let agent = &agent_list[idx];
        println!();
        println!("  {}", style(agent.name).cyan());

        // Compute per-agent token reference string (None when no token configured).
        let token_ref: Option<String> = stored_token.as_deref()
            .and_then(|_| agent.env_syntax.render("ALCOVE_TOKEN"));

        // MCP
        match &agent.mcp_config {
            McpConfig::Json { path, server_key, omit_type } => {
                let p = expand_path(path);
                write_json_mcp(&p, server_key, &bin, &docs_root, server_url.as_deref(), token_ref.as_deref(), *omit_type)?;
                println!(
                    "  {} {} ({})",
                    style("✓").green(),
                    t!("setup.mcp_set", path = path),
                    if server_url.is_some() { "HTTP" } else { "stdio" }
                );
            }
            McpConfig::OpenCode { path } => {
                let p = expand_path(path);
                write_opencode_mcp(&p, &bin, &docs_root, server_url.as_deref(), token_ref.as_deref())?;
                println!(
                    "  {} {} ({})",
                    style("✓").green(),
                    t!("setup.mcp_set", path = path),
                    if server_url.is_some() { "HTTP" } else { "stdio" }
                );
            }
            McpConfig::Codex { path } => {
                let p = expand_path(path);
                write_codex_mcp(&p, &bin, &docs_root, server_url.as_deref(), token_ref.as_deref())?;
                println!(
                    "  {} {} ({})",
                    style("✓").green(),
                    t!("setup.mcp_set", path = path),
                    if server_url.is_some() { "HTTP" } else { "stdio" }
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
                    port.unwrap_or("57384"),
                    service_status
                );
            }
            #[cfg(not(all(feature = "alcove-server", target_os = "macos")))]
            {
                println!(
                    "  Server: {}:{}",
                    host.unwrap_or("127.0.0.1"),
                    port.unwrap_or("57384")
                );
            }
        }
        None => {
            println!("  Server: default (127.0.0.1:57384)");
        }
    }

    // Seed token to shell rc files
    if let Some(token) = stored_token.as_deref() {
        {
            let seeded = seed_token_to_shell_rc(token);
            if seeded.is_empty() {
                println!(
                    "  {} Could not detect a shell rc file. Add manually:",
                    style("⚠").yellow()
                );
                println!("      export ALCOVE_TOKEN={token}");
            } else {
                for rc in &seeded {
                    println!(
                        "  {} ALCOVE_TOKEN exported → {}",
                        style("✓").green(),
                        rc.display()
                    );
                }
                println!(
                    "  {} Reload your shell or run: {}",
                    style("ℹ").dim(),
                    style("source ~/.zshrc").cyan()
                );
            }
            println!(
                "  {} View token anytime: {}",
                style("ℹ").dim(),
                style("alcove token").cyan()
            );
        }
    }

    println!();
    println!("  {}", style(t!("setup.hint_update").to_string()).dim());
    println!("  {}", style(t!("setup.hint_uninstall").to_string()).dim());
    println!();

    crate::telemetry::Telemetry::init().track_setup_completed();

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
    save_full_config_to(FullConfigParams {
        cfg_path: &cfg_path,
        docs_root,
        diagram_format,
        core_files,
        team_files,
        public_files,
        embedding_section,
        server_section,
    })?;
    println!(
        "  {} {}",
        style("✓").green(),
        t!("setup.config_saved", path = cfg_path.display())
    );
    Ok(())
}

pub(crate) struct FullConfigParams<'a> {
    pub(crate) cfg_path: &'a Path,
    pub(crate) docs_root: &'a Path,
    pub(crate) diagram_format: &'a str,
    pub(crate) core_files: &'a [String],
    pub(crate) team_files: &'a [String],
    pub(crate) public_files: &'a [String],
    pub(crate) embedding_section: Option<&'a str>,
    pub(crate) server_section: Option<&'a str>,
}

pub(crate) fn save_full_config_to(
    FullConfigParams {
        cfg_path,
        docs_root,
        diagram_format,
        core_files,
        team_files,
        public_files,
        embedding_section,
        server_section,
    }: FullConfigParams<'_>,
) -> Result<()> {
    let parent = cfg_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("config path has no parent directory: {}", cfg_path.display()))?;
    fs::create_dir_all(parent)?;

    let base = DocConfig {
        docs_root: Some(docs_root.display().to_string()),
        core: Some(CategoryConfig { files: core_files.to_vec() }),
        team: Some(CategoryConfig { files: team_files.to_vec() }),
        public: Some(CategoryConfig { files: public_files.to_vec() }),
        diagram: Some(DiagramConfig { format: diagram_format.to_string() }),
        ..DocConfig::default()
    };
    let mut content = toml::to_string(&base)
        .map_err(|e| anyhow::anyhow!("failed to serialize config: {}", e))?;

    // Guard: DocConfig::default() must not emit [embedding] or [server] sections,
    // otherwise the manually appended sections below will create duplicate headers.
    debug_assert!(
        !content.contains("[embedding]") && !content.contains("[server]"),
        "base config already contains [embedding] or [server] — would produce duplicate TOML sections"
    );

    if let Some(section) = embedding_section {
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push('\n');
        content.push_str(section.trim_start_matches('\n'));
    }

    if let Some(section) = server_section {
        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push('\n');
        content.push_str(section.trim_start_matches('\n'));
    }

    fs::write(cfg_path, content)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Token generation
// ---------------------------------------------------------------------------

/// Generate a random bearer token prefixed with `alcove-`.
pub(crate) fn generate_token() -> String {
    use std::fmt::Write;
    use rand::RngExt;
    let bytes: [u8; 16] = rand::rng().random();
    let hex = bytes.iter().fold(String::with_capacity(32), |mut s: String, b| {
        let _ = write!(s, "{b:02x}");
        s
    });
    format!("alcove-{hex}")
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
            Step::Telemetry => step_telemetry()?,
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── shellexpand ──

    #[test]
    fn shellexpand_tilde() {
        let s = shellexpand("~/my/path");
        let expected = format!("{}/my/path", dirs::home_dir().unwrap().display());
        assert_eq!(s, expected);
    }

    #[test]
    fn shellexpand_no_tilde() {
        let s = shellexpand("/absolute/path");
        assert_eq!(s, "/absolute/path");
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

        let result = save_full_config_to(FullConfigParams {
            cfg_path: &cfg,
            docs_root: &docs,
            diagram_format: "mermaid",
            core_files: &core,
            team_files: &team,
            public_files: &public,
            embedding_section: None,
            server_section: None,
        });
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

    #[test]
    fn save_full_config_writes_embedding_section() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("config.toml");
        let docs = tmp.path().join("docs");
        let core: Vec<String> = vec![];
        let team: Vec<String> = vec![];
        let public: Vec<String> = vec![];
        let embedding = "\n[embedding]\nmodel = \"nomic-embed-text\"\n";

        let result = save_full_config_to(FullConfigParams {
            cfg_path: &cfg,
            docs_root: &docs,
            diagram_format: "mermaid",
            core_files: &core,
            team_files: &team,
            public_files: &public,
            embedding_section: Some(embedding),
            server_section: None,
        });
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        assert!(content.contains("[embedding]"));
        assert!(content.contains("model = \"nomic-embed-text\""));
    }

    #[test]
    fn save_full_config_writes_server_section() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("config.toml");
        let docs = tmp.path().join("docs");
        let core: Vec<String> = vec![];
        let team: Vec<String> = vec![];
        let public: Vec<String> = vec![];
        let server = "\n[server]\nport = 57384\n";

        let result = save_full_config_to(FullConfigParams {
            cfg_path: &cfg,
            docs_root: &docs,
            diagram_format: "mermaid",
            core_files: &core,
            team_files: &team,
            public_files: &public,
            embedding_section: None,
            server_section: Some(server),
        });
        assert!(result.is_ok());

        let content = fs::read_to_string(&cfg).expect("failed to read");
        assert!(content.contains("[server]"));
        assert!(content.contains("port = 57384"));
    }

    #[test]
    fn save_full_config_special_chars_round_trip() {
        let tmp = TempDir::new().expect("failed to create temp dir");
        let cfg = tmp.path().join("config.toml");
        let docs = tmp.path().join("docs");
        let core: Vec<String> = vec![];
        let team: Vec<String> = vec![];
        let public: Vec<String> = vec![];

        let model_name = r#"nomic \"embed\" text\special"#;
        let model_toml = toml::Value::String(model_name.to_string()).to_string();
        let embedding = format!("\n[embedding]\nmodel = {model_toml}\nenabled = true\n");

        let result = save_full_config_to(FullConfigParams {
            cfg_path: &cfg,
            docs_root: &docs,
            diagram_format: "mermaid",
            core_files: &core,
            team_files: &team,
            public_files: &public,
            embedding_section: Some(&embedding),
            server_section: None,
        });
        assert!(result.is_ok(), "write failed: {:?}", result);

        let content = fs::read_to_string(&cfg).expect("failed to read");
        let parsed: toml::Value = toml::from_str(&content).expect("invalid TOML written");

        let parsed_model = parsed
            .get("embedding")
            .and_then(|e| e.get("model"))
            .and_then(|m| m.as_str())
            .expect("embedding.model not found");
        assert_eq!(parsed_model, model_name);
    }

    // ── generate_token ──

    #[test]
    fn generate_token_has_correct_prefix_and_length() {
        let token = generate_token();
        assert!(token.starts_with("alcove-"), "expected 'alcove-' prefix, got: {token}");
        assert_eq!(token.len(), 39, "unexpected token length: {token}");
    }

    #[test]
    fn generate_token_contains_only_hex_chars() {
        let token = generate_token();
        let hex_part = token.strip_prefix("alcove-").expect("prefix missing");
        assert!(
            hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "non-hex char in token: {token}"
        );
    }

    #[test]
    fn generate_token_produces_unique_values() {
        let t1 = generate_token();
        let t2 = generate_token();
        assert_ne!(t1, t2, "two consecutive tokens must differ");
    }

    // ── save_docs_root_to ──

    #[test]
    fn save_docs_root_to_escapes_special_chars() {
        let dir = TempDir::new().expect("temp dir");
        let cfg_path = dir.path().join("config.toml");

        let tricky = std::path::PathBuf::from(r#"/tmp/path"with"quotes"#);
        save_docs_root_to(&cfg_path, &tricky).expect("write should succeed");

        let written = fs::read_to_string(&cfg_path).expect("read back");
        let parsed: toml::Value = toml::from_str(&written).expect("must be valid TOML");
        let got = parsed["docs_root"].as_str().expect("docs_root is a string");
        assert_eq!(got, tricky.display().to_string());
    }

    #[test]
    fn save_docs_root_to_escapes_backslash() {
        let dir = TempDir::new().expect("temp dir");
        let cfg_path = dir.path().join("config.toml");

        let tricky = std::path::PathBuf::from(r"C:\Users\test\docs");
        save_docs_root_to(&cfg_path, &tricky).expect("write should succeed");

        let written = fs::read_to_string(&cfg_path).expect("read back");
        let parsed: toml::Value = toml::from_str(&written).expect("must be valid TOML");
        let got = parsed["docs_root"].as_str().expect("docs_root is a string");
        assert_eq!(got, tricky.display().to_string());
    }
}
