use std::fs;

use anyhow::Result;
use console::style;
use rust_i18n::t;

use crate::agents::{agents, check_agent_registration, expand_path};
use crate::config::{config_path, load_config};
use crate::setup::saved_docs_root;

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
            let model = result["embedding_model"].as_str().unwrap_or("");
            let hint = if model.is_empty() {
                "run `alcove model download`".to_string()
            } else {
                format!("{} — run `alcove model download`", model)
            };
            println!(
                "  {} hybrid search unavailable  {}",
                style("·").dim(),
                style(hint).dim(),
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
// Token subcommand
// ---------------------------------------------------------------------------

/// Print the stored bearer token from config.toml.
pub fn cmd_token() -> Result<()> {
    let cfg = load_config();
    match cfg.server.as_ref().and_then(|s| s.token.as_ref()) {
        Some(token) => {
            println!("{token}");
            Ok(())
        }
        None => {
            println!(
                "  {} No token configured. Run `alcove setup` to generate one.",
                style("⚠").yellow()
            );
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// alcove uninstall
// ---------------------------------------------------------------------------

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
    let model_toml = toml::Value::String(model_name.to_string()).to_string();
    let embedding_section = format!("[embedding]\nmodel = {model_toml}\nauto_download = true\n");

    if content.contains("[embedding]") {
        // Replace existing embedding section
        let start = content.find("[embedding]").expect("checked above with contains()");
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

// ---------------------------------------------------------------------------
// alcove reap
// ---------------------------------------------------------------------------

#[cfg(unix)]
pub fn cmd_reap() -> Result<()> {
    let self_pid = std::process::id();
    let self_bin = std::env::current_exe()?;

    let output = std::process::Command::new("ps")
        .args(["-eo", "pid=,ppid=,args="])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut reaped = 0u32;
    let mut protected = 0u32;

    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(3, ' ').collect();
        if parts.len() < 3 {
            continue;
        }

        let pid: u32 = match parts[0].trim().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let ppid: u32 = match parts[1].trim().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let args = parts[2].trim();

        if ppid != 1 {
            continue;
        }
        if pid == self_pid {
            continue;
        }

        let bin_part = args.split_whitespace().next().unwrap_or("");
        if bin_part != self_bin.to_string_lossy() {
            continue;
        }

        // Protect `alcove serve` — managed by launchd, ppid=1 is expected
        if args.contains(" serve") || args.ends_with(" serve") {
            protected += 1;
            continue;
        }

        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        reaped += 1;
    }

    if reaped == 0 && protected == 0 {
        println!("  {} No orphaned processes found.", style("✓").green());
    } else {
        if reaped > 0 {
            println!(
                "  {} Reaped {} orphaned process(es).",
                style("✓").green(),
                style(reaped).bold()
            );
        }
        if protected > 0 {
            println!(
                "  {} Kept {} alcove serve process(es) (launchd-managed).",
                style("·").dim(),
                protected
            );
        }
    }

    Ok(())
}
