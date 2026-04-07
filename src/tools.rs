use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::config::{
    classify_tier_with, effective_config, is_doc_file, load_config, suggest_categorization,
};

// ---------------------------------------------------------------------------
// Project resolution
// ---------------------------------------------------------------------------

pub struct ResolvedProject {
    pub name: String,
    pub detected_via: &'static str,
    pub repo_path: Option<PathBuf>,
}

/// Resolve the active project name using this priority:
///   1. MCP_PROJECT_NAME env var (explicit override)
///   2. CWD-based auto-detection (walk up path components, match against docs_root)
pub fn resolve_project(docs_root: &Path) -> Option<ResolvedProject> {
    // 1. Explicit env override
    if let Ok(name) = env::var("MCP_PROJECT_NAME") {
        let name = name.trim().to_string();
        if !name.is_empty() && docs_root.join(&name).is_dir() {
            let repo_path = detect_repo_path(&name);
            return Some(ResolvedProject {
                name,
                detected_via: "env",
                repo_path,
            });
        }
    }

    // 2. Auto-detect from CWD
    if let Ok(cwd) = env::current_dir() {
        let available: Vec<String> = std::fs::read_dir(docs_root)
            .ok()?
            .filter_map(std::result::Result::ok)
            .filter(|e| e.path().is_dir())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.')
                    || name.starts_with('_')
                    || name == "mcp"
                    || name == "skills"
                    || name == "scripts"
                {
                    None
                } else {
                    Some(name)
                }
            })
            .collect();

        let mut path = cwd.as_path();
        loop {
            if let Some(dirname) = path.file_name().and_then(|f| f.to_str())
                && available.iter().any(|p| p == dirname)
            {
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

fn detect_repo_path(project_name: &str) -> Option<PathBuf> {
    let cwd = env::current_dir().ok()?;
    let mut path = cwd.as_path();
    loop {
        if let Some(dirname) = path.file_name().and_then(|f| f.to_str())
            && dirname == project_name
        {
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
// Tool: check_doc_changes
// ---------------------------------------------------------------------------

pub fn tool_check_doc_changes(docs_root: &Path, args: Value) -> Result<Value> {
    let mut result = crate::index::check_doc_changes(docs_root);

    let auto_rebuild = args
        .get("auto_rebuild")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    if auto_rebuild && result["is_stale"].as_bool().unwrap_or(false) {
        let rebuild_result = crate::index::build_index(docs_root)?;
        result["rebuild"] = rebuild_result;
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Tool: get_project_docs_overview
// ---------------------------------------------------------------------------

pub fn tool_overview(
    project_root: &Path,
    project_name: &str,
    detected_via: &str,
    repo_path: Option<&Path>,
) -> Result<Value> {
    let cfg = repo_path
        .map(effective_config)
        .unwrap_or_else(|| load_config().clone());

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
            "tier": classify_tier_with(&rel, &cfg),
        }));
    }

    // Scan project repo (root + docs/) if available
    let mut repo_files = Vec::new();
    if let Some(rp) = repo_path {
        for entry in std::fs::read_dir(rp).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_file() && is_doc_file(&path) {
                let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                repo_files.push(json!({
                    "path": filename,
                    "size_bytes": size,
                    "tier": classify_tier_with(filename, &cfg),
                }));
            }
        }
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
                let size = entry.metadata().ok().map(|m| m.len()).unwrap_or(0);
                repo_files.push(json!({
                    "path": rel,
                    "size_bytes": size,
                    "tier": classify_tier_with(&rel, &cfg),
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
        "diagram_format": cfg.diagram_format(),
        "hint": "Start with PRD.md (what/why), ARCHITECTURE.md (how), PROGRESS.md (status)",
        "diagram_hint": format!("Use {} syntax when creating or updating diagrams in docs.", cfg.diagram_format()),
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
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[alcove] Failed to read {}: {}", path.display(), e);
                continue;
            }
        };
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

pub fn tool_search(
    project_root: &Path,
    args_value: Value,
    repo_path: Option<&Path>,
) -> Result<Value> {
    let args: SearchArgs = serde_json::from_value(args_value)
        .context("search_project_docs requires { query, limit? }")?;

    let query_trimmed = args.query.trim();
    if query_trimmed.is_empty() {
        return Ok(json!({ "query": args.query, "matches": [], "truncated": false, "error": "empty query" }));
    }
    if args.limit == 0 {
        return Ok(json!({ "query": args.query, "matches": [], "truncated": false }));
    }

    // Extract project name and docs_root
    let project_name = project_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let docs_root = project_root.parent().unwrap_or(project_root);

    // Try indexed search first (faster and ranked)
    #[cfg(feature = "alcove-full")]
    {
        use crate::embedding::{EmbeddingModelChoice, EmbeddingService, EmbeddingConfig};
        use crate::index::search_hybrid;
        use crate::config::load_config;

        if crate::index::index_exists(docs_root) {
            // Get embedding config
            let cfg = load_config();
            let emb_cfg = cfg.embedding_config_with_defaults();
            
            // Create embedding service if enabled
            if emb_cfg.enabled {
                let model = EmbeddingModelChoice::parse(&emb_cfg.model).unwrap_or_default();
                let service = EmbeddingService::new(EmbeddingConfig {
                    model,
                    auto_download: emb_cfg.auto_download,
                    cache_dir: std::path::PathBuf::from(
                        emb_cfg.cache_dir.starts_with('~')
                            .then(|| std::env::var("HOME").ok())
                            .flatten()
                            .map(|h| emb_cfg.cache_dir.replacen('~', &h, 1))
                            .unwrap_or_else(|| emb_cfg.cache_dir.clone())
                    ),
                    enabled: true,
                });

                // Use hybrid search
                let result = search_hybrid(
                    docs_root,
                    query_trimmed,
                    &service,
                    args.limit,
                    Some(project_name),
                );

                if let Ok(json) = result {
                    // Transform to expected format
                    let matches: Vec<Value> = json["matches"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|m| {
                                    Some(json!({
                                        "file": m["file"].as_str().unwrap_or("?"),
                                        "line": m["line_start"].as_u64().unwrap_or(0),
                                        "snippet": m["snippet"].as_str().unwrap_or(""),
                                        "source": "hybrid",
                                        "score": m["score"].as_f64().unwrap_or(0.0),
                                    }))
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    return Ok(json!({
                        "query": args.query,
                        "matches": matches,
                        "truncated": json["truncated"].as_bool().unwrap_or(false),
                        "mode": json["mode"].as_str().unwrap_or("hybrid"),
                    }));
                }
            }
        }
    }

    // Fall back to indexed search (BM25 only) if index exists
    if crate::index::index_exists(docs_root) {
        let result = crate::index::search_indexed(docs_root, query_trimmed, args.limit, Some(project_name));
        if let Ok(json) = result {
            let matches: Vec<Value> = json["matches"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            Some(json!({
                                "file": m["file"].as_str().unwrap_or("?"),
                                "line": m["line_start"].as_u64().unwrap_or(0),
                                "snippet": m["snippet"].as_str().unwrap_or(""),
                                "source": "ranked",
                                "score": m["score"].as_f64().unwrap_or(0.0),
                            }))
                        })
                        .collect()
                })
                .unwrap_or_default();

            return Ok(json!({
                "query": args.query,
                "matches": matches,
                "truncated": json["truncated"].as_bool().unwrap_or(false),
                "mode": "ranked",
            }));
        }
    }

    // Fall back to grep search if no index
    let query_lower = query_trimmed.to_lowercase();
    let mut matches = Vec::new();

    // 1. Search alcove folder
    search_dir_for_query(
        project_root,
        project_root,
        &query_lower,
        "alcove",
        args.limit,
        &mut matches,
    );

    // 2. Search project repo (root-level + docs/) if available
    if let Some(rp) = repo_path {
        for entry in std::fs::read_dir(rp).into_iter().flatten().flatten() {
            if matches.len() >= args.limit {
                break;
            }
            let path = entry.path();
            if !path.is_file() || !is_doc_file(&path) {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[alcove] Failed to read {}: {}", path.display(), e);
                    continue;
                }
            };
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
        search_dir_for_query(
            &rp.join("docs"),
            rp,
            &query_lower,
            "project-repo",
            args.limit,
            &mut matches,
        );
    }

    let truncated = matches.len() >= args.limit;
    Ok(json!({ "query": args.query, "matches": matches, "truncated": truncated, "mode": "grep" }))
}

/// Global search across all projects in docs_root.
pub fn tool_search_global(docs_root: &Path, args_value: Value) -> Result<Value> {
    let args: SearchArgs = serde_json::from_value(args_value)
        .context("search_project_docs requires { query, scope?, limit? }")?;

    let query_trimmed = args.query.trim();
    if query_trimmed.is_empty() {
        return Ok(json!({ "query": args.query, "matches": [], "truncated": false, "error": "empty query" }));
    }
    if args.limit == 0 {
        return Ok(json!({ "query": args.query, "matches": [], "truncated": false }));
    }

    // Try indexed search first (faster and ranked)
    if crate::index::index_exists(docs_root) {
        let result = crate::index::search_indexed(docs_root, query_trimmed, args.limit, None);
        if let Ok(json) = result {
            let matches: Vec<Value> = json["matches"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            Some(json!({
                                "project": m["project"].as_str().unwrap_or("?"),
                                "file": m["file"].as_str().unwrap_or("?"),
                                "line": m["line_start"].as_u64().unwrap_or(0),
                                "snippet": m["snippet"].as_str().unwrap_or(""),
                                "source": "ranked",
                                "score": m["score"].as_f64().unwrap_or(0.0),
                            }))
                        })
                        .collect()
                })
                .unwrap_or_default();

            return Ok(json!({
                "query": args.query,
                "matches": matches,
                "truncated": json["truncated"].as_bool().unwrap_or(false),
                "mode": "ranked",
            }));
        }
    }

    // Fall back to grep search if no index
    let query_lower = query_trimmed.to_lowercase();
    let mut matches = Vec::new();

    let entries = std::fs::read_dir(docs_root).context("Failed to read DOCS_ROOT directory")?;

    for entry in entries.flatten() {
        if matches.len() >= args.limit {
            break;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if name.starts_with('.') || name.starts_with('_') || name == "mcp" || name == "skills" {
            continue;
        }

        for walk_entry in WalkDir::new(&path)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file() && is_doc_file(e.path()))
        {
            if matches.len() >= args.limit {
                break;
            }
            let file_path = walk_entry.path();
            let content = match std::fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[alcove] Failed to read {}: {}", file_path.display(), e);
                    continue;
                }
            };
            let rel = file_path
                .strip_prefix(&path)
                .unwrap_or(file_path)
                .to_string_lossy()
                .to_string();

            for (idx, line) in content.lines().enumerate() {
                if line.to_lowercase().contains(&query_lower) {
                    matches.push(json!({
                        "project": name,
                        "file": rel,
                        "line": idx + 1,
                        "snippet": line.trim(),
                    }));
                    if matches.len() >= args.limit {
                        break;
                    }
                }
            }
        }
    }

    let truncated = matches.len() >= args.limit;
    Ok(
        json!({ "query": args.query, "scope": "global", "matches": matches, "truncated": truncated, "mode": "grep" }),
    )
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

pub fn tool_get_file(project_root: &Path, args_value: Value) -> Result<Value> {
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

pub fn tool_list_projects(docs_root: &Path) -> Result<Value> {
    let mut projects = Vec::new();

    let entries = std::fs::read_dir(docs_root).context("Failed to read DOCS_ROOT directory")?;

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

        if name.starts_with('.') || name.starts_with('_') || name == "mcp" || name == "skills" {
            continue;
        }

        let doc_count = WalkDir::new(&path)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file() && is_doc_file(e.path()))
            .count();

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
// Tool: configure_project
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ConfigureProjectArgs {
    project_name: String,
    #[serde(default)]
    diagram_format: Option<String>,
    #[serde(default)]
    core_files: Option<Vec<String>>,
    #[serde(default)]
    team_files: Option<Vec<String>>,
    #[serde(default)]
    public_files: Option<Vec<String>>,
}

/// Create or update `{repo_path}/alcove.toml` with the provided settings.
/// `repo_path` is the project's repository root (CWD-detected or explicit).
/// Existing settings not mentioned in the call are preserved.
pub fn tool_configure_project(repo_path: &Path, args_value: Value) -> Result<Value> {
    use crate::config::{CategoryConfig, DiagramConfig, load_project_config, project_config_path};

    let args: ConfigureProjectArgs = serde_json::from_value(args_value)
        .context("configure_project requires { project_name, ... }")?;

    // Load existing project config (empty if none)
    let mut cfg = load_project_config(repo_path);

    // Apply updates for each provided field
    if let Some(fmt) = args.diagram_format {
        cfg.diagram = Some(DiagramConfig { format: fmt });
    }
    if let Some(files) = args.core_files {
        cfg.core = Some(CategoryConfig { files });
    }
    if let Some(files) = args.team_files {
        cfg.team = Some(CategoryConfig { files });
    }
    if let Some(files) = args.public_files {
        cfg.public = Some(CategoryConfig { files });
    }

    // Serialize and write
    let toml_str = toml::to_string_pretty(&cfg).context("Failed to serialize config")?;
    let config_path = project_config_path(repo_path);
    std::fs::write(&config_path, &toml_str).context("Failed to write alcove.toml")?;

    Ok(json!({
        "project_name": args.project_name,
        "config_path": config_path.to_string_lossy(),
        "written": toml_str,
    }))
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
    #[serde(default)]
    files: Option<Vec<String>>,
}

/// Create user-facing docs (README, CHANGELOG, QUICKSTART) in the project repository.
fn create_repo_docs(
    name: &str,
    project_path: &Option<String>,
    file_filter: &Option<Vec<String>>,
    overwrite: bool,
) -> Result<(Vec<String>, Vec<String>, String)> {
    let mut repo_created = Vec::new();
    let mut repo_skipped = Vec::new();
    let mut repo_path_used = String::new();

    let Some(project_path) = project_path else {
        return Ok((repo_created, repo_skipped, repo_path_used));
    };

    let project_dir = PathBuf::from(project_path);
    if !project_dir.exists() || !project_dir.is_dir() {
        anyhow::bail!("project_path does not exist or is not a directory: {project_path}");
    }

    repo_path_used = project_path.clone();

    let mut create_file = |filename: &str, content: String| -> Result<()> {
        if let Some(filter) = file_filter
            && !filter.iter().any(|f| f == filename)
        {
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

    create_file(
        "README.md",
        format!(
            r"# {name}

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
",
        ),
    )?;

    create_file(
        "CHANGELOG.md",
        format!(
            r"# Changelog

All notable changes to {name} will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial project setup
",
        ),
    )?;

    create_file(
        "QUICKSTART.md",
        format!(
            r"# {name} — Quick Start

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
",
        ),
    )?;

    Ok((repo_created, repo_skipped, repo_path_used))
}

pub fn tool_init_project(docs_root: &Path, args_value: Value) -> Result<Value> {
    let args: InitProjectArgs = serde_json::from_value(args_value)
        .context("init_project requires { project_name, project_path?, overwrite? }")?;

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

    std::fs::create_dir_all(project_root.join("reports"))?;

    let mut bridge_created = Vec::new();
    let mut bridge_skipped = Vec::new();

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

        let filename = rel.file_name().and_then(|f| f.to_str()).unwrap_or("");

        if rel == Path::new("README.md") || filename == ".gitkeep" {
            continue;
        }

        if let Some(ref filter) = file_filter {
            let rel_str = rel.to_string_lossy();
            if !filter
                .iter()
                .any(|f| f == filename || f == rel_str.as_ref())
            {
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

    // External: Create user-facing docs in project repository
    let (repo_created, repo_skipped, repo_path_used) =
        create_repo_docs(name, &args.project_path, &file_filter, overwrite)?;

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
// Helpers for audit
// ---------------------------------------------------------------------------

/// Scan a project repository (root + docs/) for documentation files.
fn scan_repo_docs(
    repo_path: Option<&Path>,
    cfg: &crate::config::DocConfig,
) -> (Vec<Value>, String) {
    let mut repo_docs = Vec::new();
    let mut repo_path_str = String::new();

    let Some(rp) = repo_path else {
        return (repo_docs, repo_path_str);
    };

    repo_path_str = rp.to_string_lossy().to_string();

    for entry in std::fs::read_dir(rp).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_file() && is_doc_file(&path) {
            let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
            let rel = filename.to_string();
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let tier = classify_tier_with(&rel, cfg);
            repo_docs.push(json!({
                "path": rel,
                "filename": filename,
                "size_bytes": size,
                "tier": tier,
            }));
        }
    }

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
            let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
            let size = entry.metadata().ok().map(|m| m.len()).unwrap_or(0);
            let tier = classify_tier_with(&rel, cfg);
            repo_docs.push(json!({
                "path": rel,
                "filename": filename,
                "size_bytes": size,
                "tier": tier,
            }));
        }
    }

    (repo_docs, repo_path_str)
}

// ---------------------------------------------------------------------------
// Tool: audit_project
// ---------------------------------------------------------------------------

pub fn tool_audit(
    project_root: &Path,
    project_name: &str,
    repo_path: Option<&Path>,
) -> Result<Value> {
    let cfg = repo_path
        .map(effective_config)
        .unwrap_or_else(|| load_config().clone());

    let mut tier1_status = Vec::new();
    let core_files = cfg.core_files();

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

    let mut supplementary_files = Vec::new();
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

        if core_files.iter().any(|f| f == filename) || rel.starts_with("reports/") {
            continue;
        }

        let meta = entry.metadata()?;
        let tier = classify_tier_with(&rel, &cfg);

        match tier {
            "doc-repo-supplementary" | "project-repo" => {
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

    // Scan project repository
    let (repo_docs, repo_path_str) = scan_repo_docs(repo_path, &cfg);

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
    let populated =
        core_files.len() - missing_files.len() - unfilled_files.len() - minimal_files.len();

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

    // Cross-repo suggestions
    if repo_path.is_some() {
        let repo_filenames: Vec<String> = repo_docs
            .iter()
            .filter_map(|d| d["filename"].as_str().map(String::from))
            .collect();

        let public_files = cfg.public_files();
        let missing_public: Vec<&str> = public_files
            .iter()
            .filter(|f| {
                !repo_filenames
                    .iter()
                    .any(|r| r.to_lowercase() == f.to_lowercase())
            })
            .map(std::string::String::as_str)
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

        let mut duplicate_in_repo = Vec::new();
        for doc in &repo_docs {
            let fname = doc["filename"].as_str().unwrap_or("");
            let fname_lower = fname.to_lowercase();
            if core_files.iter().any(|f| f.to_lowercase() == fname_lower) {
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
                    "If the project repo version has additional content, merge it into alcove first, ",
                    "then remove from the project repo."
                )
            }));
        }

        let mut repo_reports = Vec::new();
        let mut repo_incorporable = Vec::new();
        for doc in &repo_docs {
            let fname = doc["filename"].as_str().unwrap_or("");
            let fname_lower = fname.to_lowercase();
            let tier = doc["tier"].as_str().unwrap_or("");

            if tier == "project-repo" {
                continue;
            }
            if duplicate_in_repo
                .iter()
                .any(|d| d.to_lowercase() == fname_lower)
            {
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
        "diagram_format": cfg.diagram_format(),
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn slice_content(content: &str, offset: Option<usize>, limit: Option<usize>) -> String {
    let start = offset.unwrap_or(0);
    let chars: Vec<char> = content.chars().collect();
    let total = chars.len();

    if start >= total {
        return String::new();
    }

    let end = limit.map(|l| (start + l).min(total)).unwrap_or(total);

    chars[start..end].iter().collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Create a minimal docs_root with one project folder containing a few doc files.
    fn setup_docs_root() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("testproj");
        fs::create_dir_all(project.join("reports")).unwrap();
        fs::write(
            project.join("PRD.md"),
            "# Product Requirements\n\nReal content here about the product.",
        )
        .unwrap();
        fs::write(
            project.join("ARCHITECTURE.md"),
            "# Architecture\n\nSystem design overview.",
        )
        .unwrap();
        fs::write(
            project.join("reports/audit.md"),
            "# Audit Report\n\nFindings.",
        )
        .unwrap();
        // Template dir for init_project
        let template = tmp.path().join("_template");
        fs::create_dir_all(template.join("reports")).unwrap();
        fs::write(template.join("PRD.md"), "# ProjectName PRD\n\nTODO").unwrap();
        fs::write(
            template.join("ARCHITECTURE.md"),
            "# ProjectName Architecture\n\nTODO",
        )
        .unwrap();
        fs::write(template.join("README.md"), "Template readme (skipped)").unwrap();
        fs::write(template.join("reports/.gitkeep"), "").unwrap();
        tmp
    }

    // -- slice_content --

    #[test]
    fn slice_content_full() {
        assert_eq!(slice_content("hello world", None, None), "hello world");
    }

    #[test]
    fn slice_content_with_offset() {
        assert_eq!(slice_content("hello world", Some(6), None), "world");
    }

    #[test]
    fn slice_content_with_limit() {
        assert_eq!(slice_content("hello world", None, Some(5)), "hello");
    }

    #[test]
    fn slice_content_offset_and_limit() {
        assert_eq!(slice_content("hello world", Some(6), Some(3)), "wor");
    }

    #[test]
    fn slice_content_offset_beyond_length() {
        assert_eq!(slice_content("hello", Some(100), None), "");
    }

    #[test]
    fn slice_content_limit_beyond_length() {
        assert_eq!(slice_content("hi", Some(0), Some(999)), "hi");
    }

    #[test]
    fn slice_content_unicode() {
        let s = "안녕하세요";
        assert_eq!(slice_content(s, Some(2), Some(2)), "하세");
    }

    // -- tool_list_projects --

    #[test]
    fn list_projects_finds_project() {
        let tmp = setup_docs_root();
        let result = tool_list_projects(tmp.path()).unwrap();
        let projects = result["projects"].as_array().unwrap();
        assert!(projects.iter().any(|p| p["name"] == "testproj"));
    }

    #[test]
    fn list_projects_skips_hidden_and_template() {
        let tmp = setup_docs_root();
        fs::create_dir_all(tmp.path().join(".hidden")).unwrap();
        // _template already exists
        let result = tool_list_projects(tmp.path()).unwrap();
        let names: Vec<&str> = result["projects"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["name"].as_str())
            .collect();
        assert!(!names.contains(&".hidden"));
        assert!(!names.contains(&"_template"));
    }

    #[test]
    fn list_projects_counts_docs() {
        let tmp = setup_docs_root();
        let result = tool_list_projects(tmp.path()).unwrap();
        let proj = result["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "testproj")
            .unwrap();
        // PRD.md + ARCHITECTURE.md + reports/audit.md = 3
        assert_eq!(proj["total_docs"].as_u64().unwrap(), 3);
    }

    // -- tool_overview --

    #[test]
    fn overview_lists_files() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let result = tool_overview(&project_root, "testproj", "test", None).unwrap();
        assert_eq!(result["project_name"], "testproj");
        assert_eq!(result["detected_via"], "test");
        let doc_repo = &result["doc_repo"];
        assert_eq!(doc_repo["count"].as_u64().unwrap(), 3);
    }

    #[test]
    fn overview_with_repo_path() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        // Create a fake repo with a README
        let repo = TempDir::new().unwrap();
        fs::write(repo.path().join("README.md"), "# Test").unwrap();

        let result = tool_overview(&project_root, "testproj", "test", Some(repo.path())).unwrap();
        assert!(result["project_repo"].is_object());
        assert_eq!(result["project_repo"]["count"].as_u64().unwrap(), 1);
    }

    // -- tool_search --

    #[test]
    fn search_finds_matching_lines() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let args = json!({"query": "Architecture"});
        let result = tool_search(&project_root, args, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty());
        assert!(
            matches
                .iter()
                .any(|m| m["file"].as_str().unwrap().contains("ARCHITECTURE"))
        );
    }

    #[test]
    fn search_case_insensitive() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let args = json!({"query": "architecture"});
        let result = tool_search(&project_root, args, None).unwrap();
        assert!(!result["matches"].as_array().unwrap().is_empty());
    }

    #[test]
    fn search_respects_limit() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let args = json!({"query": "#", "limit": 1});
        let result = tool_search(&project_root, args, None).unwrap();
        assert_eq!(result["matches"].as_array().unwrap().len(), 1);
        assert_eq!(result["truncated"], true);
    }

    #[test]
    fn search_no_results() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let args = json!({"query": "zzz_nonexistent_zzz"});
        let result = tool_search(&project_root, args, None).unwrap();
        assert!(result["matches"].as_array().unwrap().is_empty());
        assert_eq!(result["truncated"], false);
    }

    #[test]
    fn search_includes_repo_path() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let repo = TempDir::new().unwrap();
        fs::write(repo.path().join("README.md"), "# Unique marker xyz123").unwrap();

        let args = json!({"query": "xyz123"});
        let result = tool_search(&project_root, args, Some(repo.path())).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(matches.iter().any(|m| m["source"] == "project-repo"));
    }

    // -- tool_get_file --

    #[test]
    fn get_file_reads_content() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let args = json!({"relative_path": "PRD.md"});
        let result = tool_get_file(&project_root, args).unwrap();
        assert_eq!(result["path"], "PRD.md");
        assert!(
            result["content"]
                .as_str()
                .unwrap()
                .contains("Product Requirements")
        );
        assert!(result["total_chars"].as_u64().unwrap() > 0);
    }

    #[test]
    fn get_file_with_offset_limit() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let args = json!({"relative_path": "PRD.md", "offset": 0, "limit": 5});
        let result = tool_get_file(&project_root, args).unwrap();
        assert_eq!(result["content"].as_str().unwrap(), "# Pro");
    }

    #[test]
    fn get_file_rejects_traversal() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let args = json!({"relative_path": "../_template/PRD.md"});
        let result = tool_get_file(&project_root, args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("traversal"));
    }

    #[test]
    fn get_file_rejects_nonexistent() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let args = json!({"relative_path": "NOPE.md"});
        let result = tool_get_file(&project_root, args);
        assert!(result.is_err());
    }

    #[test]
    fn get_file_rejects_non_doc() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        fs::write(project_root.join("code.rs"), "fn main() {}").unwrap();
        let args = json!({"relative_path": "code.rs"});
        let result = tool_get_file(&project_root, args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not allowed"));
    }

    #[test]
    fn get_file_reads_nested() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let args = json!({"relative_path": "reports/audit.md"});
        let result = tool_get_file(&project_root, args).unwrap();
        assert!(result["content"].as_str().unwrap().contains("Audit Report"));
    }

    // -- tool_init_project --

    #[test]
    fn init_project_creates_docs() {
        let tmp = setup_docs_root();
        let args = json!({"project_name": "newproj"});
        let result = tool_init_project(tmp.path(), args).unwrap();
        assert_eq!(result["project_name"], "newproj");
        let created: Vec<&str> = result["internal_docs"]["created"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(created.contains(&"PRD.md"));
        assert!(created.contains(&"ARCHITECTURE.md"));
        // Verify files on disk
        assert!(tmp.path().join("newproj/PRD.md").exists());
        // Verify template substitution
        let content = fs::read_to_string(tmp.path().join("newproj/PRD.md")).unwrap();
        assert!(content.contains("newproj"));
        assert!(!content.contains("ProjectName"));
    }

    #[test]
    fn init_project_skips_existing() {
        let tmp = setup_docs_root();
        // First init
        tool_init_project(tmp.path(), json!({"project_name": "skiptest"})).unwrap();
        // Second init without overwrite
        let result = tool_init_project(tmp.path(), json!({"project_name": "skiptest"})).unwrap();
        let skipped = result["internal_docs"]["skipped"].as_array().unwrap();
        assert!(!skipped.is_empty());
        let created = result["internal_docs"]["created"].as_array().unwrap();
        assert!(created.is_empty());
    }

    #[test]
    fn init_project_overwrite() {
        let tmp = setup_docs_root();
        tool_init_project(tmp.path(), json!({"project_name": "overtest"})).unwrap();
        let result = tool_init_project(
            tmp.path(),
            json!({"project_name": "overtest", "overwrite": true}),
        )
        .unwrap();
        let created: Vec<&str> = result["internal_docs"]["created"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(created.contains(&"PRD.md"));
    }

    #[test]
    fn init_project_with_file_filter() {
        let tmp = setup_docs_root();
        let args = json!({"project_name": "filtered", "files": ["PRD.md"]});
        let result = tool_init_project(tmp.path(), args).unwrap();
        let created: Vec<&str> = result["internal_docs"]["created"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(created.contains(&"PRD.md"));
        assert!(!created.contains(&"ARCHITECTURE.md"));
    }

    #[test]
    fn init_project_creates_repo_docs() {
        let tmp = setup_docs_root();
        let repo = TempDir::new().unwrap();
        let args = json!({
            "project_name": "repoproj",
            "project_path": repo.path().to_string_lossy()
        });
        let result = tool_init_project(tmp.path(), args).unwrap();
        let repo_created: Vec<&str> = result["external_docs"]["created"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(repo_created.contains(&"README.md"));
        assert!(repo_created.contains(&"CHANGELOG.md"));
        assert!(repo_created.contains(&"QUICKSTART.md"));
        assert!(repo.path().join("README.md").exists());
    }

    #[test]
    fn init_project_rejects_invalid_names() {
        let tmp = setup_docs_root();
        for name in ["", ".hidden", "_private", "a/b", "a\\b", "a..b"] {
            let result = tool_init_project(tmp.path(), json!({"project_name": name}));
            assert!(result.is_err(), "should reject name: {name}");
        }
    }

    #[test]
    fn init_project_no_template_fails() {
        let tmp = TempDir::new().unwrap(); // no _template dir
        let result = tool_init_project(tmp.path(), json!({"project_name": "test"}));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Template"));
    }

    // -- tool_audit --

    #[test]
    fn audit_reports_file_status() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let result = tool_audit(&project_root, "testproj", None).unwrap();
        assert_eq!(result["project_name"], "testproj");

        let required = result["doc_repo"]["required"].as_array().unwrap();
        // PRD.md should be populated (>100 bytes? check)
        let prd = required.iter().find(|r| r["file"] == "PRD.md").unwrap();
        assert_eq!(prd["exists"], true);

        // PROGRESS.md should be missing
        let progress = required.iter().find(|r| r["file"] == "PROGRESS.md");
        if let Some(p) = progress {
            assert_eq!(p["status"], "missing");
        }
    }

    #[test]
    fn audit_detects_template_unfilled() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        // Write a file with ProjectName placeholder
        fs::write(
            project_root.join("CONVENTIONS.md"),
            "# ProjectName Conventions\n\nPlaceholder.",
        )
        .unwrap();
        let result = tool_audit(&project_root, "testproj", None).unwrap();
        let required = result["doc_repo"]["required"].as_array().unwrap();
        let conv = required
            .iter()
            .find(|r| r["file"] == "CONVENTIONS.md")
            .unwrap();
        assert_eq!(conv["status"], "template-unfilled");
    }

    #[test]
    fn audit_detects_minimal() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        fs::write(project_root.join("DEBT.md"), "# Debt\n").unwrap(); // <100 bytes, no placeholder
        let result = tool_audit(&project_root, "testproj", None).unwrap();
        let required = result["doc_repo"]["required"].as_array().unwrap();
        let debt = required.iter().find(|r| r["file"] == "DEBT.md").unwrap();
        assert_eq!(debt["status"], "minimal");
    }

    #[test]
    fn audit_with_repo_path() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let repo = TempDir::new().unwrap();
        fs::write(repo.path().join("README.md"), "# Test Project").unwrap();

        let result = tool_audit(&project_root, "testproj", Some(repo.path())).unwrap();
        assert!(result["project_repo"].is_object());
        let repo_docs = result["project_repo"]["docs"].as_array().unwrap();
        assert!(!repo_docs.is_empty());
    }

    #[test]
    fn audit_suggests_missing_internal() {
        let tmp = TempDir::new().unwrap();
        let project_root = tmp.path().join("emptyproj");
        fs::create_dir_all(&project_root).unwrap();

        let result = tool_audit(&project_root, "emptyproj", None).unwrap();
        let actions = result["suggested_actions"].as_array().unwrap();
        assert!(
            actions
                .iter()
                .any(|a| a["action"] == "create_missing_internal")
        );
    }

    #[test]
    fn audit_no_actions_when_complete() {
        let tmp = TempDir::new().unwrap();
        let project_root = tmp.path().join("completeproj");
        fs::create_dir_all(&project_root).unwrap();

        // Create all core files with enough content
        let core = load_config().core_files();
        for f in &core {
            let content = format!("# {f}\n\n{}", "x".repeat(200));
            fs::write(project_root.join(f), content).unwrap();
        }

        let result = tool_audit(&project_root, "completeproj", None).unwrap();
        let actions = result["suggested_actions"].as_array().unwrap();
        assert!(actions.iter().any(|a| a["action"] == "none"));
    }

    // =====================================================================
    // Edge-case & coverage tests
    // =====================================================================

    // -- tool_search: offset/limit in arguments --

    #[test]
    fn search_with_offset_limit_args() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        // "#" appears in headings of multiple files; limit to 2
        let args = json!({"query": "#", "limit": 2});
        let result = tool_search(&project_root, args, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(result["truncated"], true);
    }

    // -- tool_search: empty query string --

    #[test]
    fn search_with_empty_query() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let args = json!({"query": ""});
        let result = tool_search(&project_root, args, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(matches.is_empty(), "empty query should return no matches");
        assert_eq!(result["error"].as_str(), Some("empty query"));
    }

    // -- tool_get_file: read a .txt file --

    #[test]
    fn get_file_reads_txt() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        fs::write(project_root.join("notes.txt"), "Plain text notes here.").unwrap();

        let args = json!({"relative_path": "notes.txt"});
        let result = tool_get_file(&project_root, args).unwrap();
        assert_eq!(result["path"], "notes.txt");
        assert!(
            result["content"]
                .as_str()
                .unwrap()
                .contains("Plain text notes")
        );
    }

    // -- tool_get_file: read a file in a nested subdirectory (3 levels deep) --

    #[test]
    fn get_file_reads_deeply_nested() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let deep_dir = project_root.join("level1/level2/level3");
        fs::create_dir_all(&deep_dir).unwrap();
        fs::write(
            deep_dir.join("deep.md"),
            "# Deep File\n\nContent at depth 3.",
        )
        .unwrap();

        let args = json!({"relative_path": "level1/level2/level3/deep.md"});
        let result = tool_get_file(&project_root, args).unwrap();
        assert_eq!(result["path"], "level1/level2/level3/deep.md");
        assert!(result["content"].as_str().unwrap().contains("Deep File"));
    }

    // -- tool_init_project: invalid name with path separators --

    #[test]
    fn init_project_rejects_path_separator_names() {
        let tmp = setup_docs_root();

        let slash = tool_init_project(tmp.path(), json!({"project_name": "foo/bar"}));
        assert!(slash.is_err());
        let err_msg = slash.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid project name"), "got: {err_msg}");

        let backslash = tool_init_project(tmp.path(), json!({"project_name": "foo\\bar"}));
        assert!(backslash.is_err());
        let err_msg = backslash.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid project name"), "got: {err_msg}");

        let dotdot = tool_init_project(tmp.path(), json!({"project_name": "foo..bar"}));
        assert!(dotdot.is_err());
    }

    // -- tool_init_project: project_path creates README and CHANGELOG --

    #[test]
    fn init_project_creates_repo_readme_and_changelog() {
        let tmp = setup_docs_root();
        let repo = TempDir::new().unwrap();
        let args = json!({
            "project_name": "repodocs",
            "project_path": repo.path().to_string_lossy()
        });
        let result = tool_init_project(tmp.path(), args).unwrap();

        let repo_created: Vec<&str> = result["external_docs"]["created"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(
            repo_created.contains(&"README.md"),
            "README.md should be created"
        );
        assert!(
            repo_created.contains(&"CHANGELOG.md"),
            "CHANGELOG.md should be created"
        );

        // Verify content on disk contains project name substitution
        let readme = fs::read_to_string(repo.path().join("README.md")).unwrap();
        assert!(
            readme.contains("repodocs"),
            "README should contain project name"
        );

        let changelog = fs::read_to_string(repo.path().join("CHANGELOG.md")).unwrap();
        assert!(
            changelog.contains("repodocs"),
            "CHANGELOG should contain project name"
        );
    }

    // -- tool_overview: empty project directory --

    #[test]
    fn overview_empty_project_dir() {
        let tmp = TempDir::new().unwrap();
        let empty_project = tmp.path().join("emptyproj");
        fs::create_dir_all(&empty_project).unwrap();

        let result = tool_overview(&empty_project, "emptyproj", "test", None).unwrap();
        assert_eq!(result["project_name"], "emptyproj");
        assert_eq!(result["total_files"].as_u64().unwrap(), 0);
        assert_eq!(result["doc_repo"]["count"].as_u64().unwrap(), 0);
    }

    // -- tool_overview: mixed file types (.md, .txt, non-doc files) --

    #[test]
    fn overview_mixed_file_types() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("mixedproj");
        fs::create_dir_all(&project).unwrap();

        fs::write(project.join("README.md"), "# Readme").unwrap();
        fs::write(project.join("notes.txt"), "some notes").unwrap();
        fs::write(project.join("main.rs"), "fn main() {}").unwrap();
        fs::write(project.join("style.css"), "body {}").unwrap();
        fs::write(project.join("data.json"), "{}").unwrap(); // json not matching openapi/swagger

        let result = tool_overview(&project, "mixedproj", "test", None).unwrap();
        let files = result["doc_repo"]["files"].as_array().unwrap();
        let file_paths: Vec<&str> = files.iter().filter_map(|f| f["path"].as_str()).collect();

        // .md and .txt are doc files
        assert!(file_paths.contains(&"README.md"), "should include .md");
        assert!(file_paths.contains(&"notes.txt"), "should include .txt");
        // .rs, .css, plain .json are not doc files
        assert!(!file_paths.contains(&"main.rs"), "should exclude .rs");
        assert!(!file_paths.contains(&"style.css"), "should exclude .css");
        assert!(
            !file_paths.contains(&"data.json"),
            "should exclude non-openapi .json"
        );
        // Total should be 2
        assert_eq!(result["doc_repo"]["count"].as_u64().unwrap(), 2);
    }

    // -- tool_audit: repo_path with docs/ subdirectory --

    #[test]
    fn audit_with_repo_docs_subdirectory() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");

        let repo = TempDir::new().unwrap();
        fs::write(repo.path().join("README.md"), "# Project Readme").unwrap();
        let docs_dir = repo.path().join("docs");
        fs::create_dir_all(&docs_dir).unwrap();
        fs::write(
            docs_dir.join("guide.md"),
            "# User Guide\n\nDetailed usage instructions.",
        )
        .unwrap();
        fs::write(
            docs_dir.join("api.md"),
            "# API Reference\n\nEndpoint documentation.",
        )
        .unwrap();

        let result = tool_audit(&project_root, "testproj", Some(repo.path())).unwrap();
        let repo_docs = result["project_repo"]["docs"].as_array().unwrap();

        // Should find README.md (root) + guide.md + api.md (docs/)
        assert!(
            repo_docs.len() >= 3,
            "expected at least 3 repo docs, got {}",
            repo_docs.len()
        );

        let paths: Vec<&str> = repo_docs
            .iter()
            .filter_map(|d| d["path"].as_str())
            .collect();
        assert!(paths.contains(&"README.md"));
        assert!(paths.iter().any(|p| p.contains("guide.md")));
        assert!(paths.iter().any(|p| p.contains("api.md")));
    }

    // -- tool_list_projects: empty docs_root --

    #[test]
    fn list_projects_empty_docs_root() {
        let tmp = TempDir::new().unwrap();
        // No project directories, just an empty root
        let result = tool_list_projects(tmp.path()).unwrap();
        let projects = result["projects"].as_array().unwrap();
        assert!(
            projects.is_empty(),
            "empty docs root should yield no projects"
        );
    }

    // -- resolve_project: MCP_PROJECT_NAME env var --

    #[test]
    fn resolve_project_with_env_var() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path().join("envproj");
        fs::create_dir_all(&project_dir).unwrap();

        // SAFETY: This test is single-threaded and we restore the env var immediately.
        unsafe {
            env::set_var("MCP_PROJECT_NAME", "envproj");
        }
        let resolved = resolve_project(tmp.path());
        // Clean up env var immediately to avoid test pollution
        unsafe {
            env::remove_var("MCP_PROJECT_NAME");
        }

        let resolved = resolved.expect("should resolve project from env var");
        assert_eq!(resolved.name, "envproj");
        assert_eq!(resolved.detected_via, "env");
    }

    // -- detect_repo_path: repo detection logic --

    #[test]
    fn detect_repo_path_returns_none_for_unrelated_name() {
        // detect_repo_path walks up CWD looking for a directory component
        // matching the project name. A random name should not match.
        let result = detect_repo_path("__nonexistent_project_name_xyz__");
        assert!(
            result.is_none(),
            "should return None for unrelated project name"
        );
    }

    // -- tool_search_global --

    fn setup_multi_project_root() -> TempDir {
        let tmp = TempDir::new().unwrap();
        // Project 1: backend
        let backend = tmp.path().join("backend");
        fs::create_dir_all(&backend).unwrap();
        fs::write(
            backend.join("PRD.md"),
            "# Backend PRD\n\nAuthentication flow using OAuth.",
        )
        .unwrap();
        fs::write(
            backend.join("ARCHITECTURE.md"),
            "# Backend Architecture\n\nMicroservices design.",
        )
        .unwrap();
        // Project 2: frontend
        let frontend = tmp.path().join("frontend");
        fs::create_dir_all(&frontend).unwrap();
        fs::write(
            frontend.join("PRD.md"),
            "# Frontend PRD\n\nLogin page with OAuth integration.",
        )
        .unwrap();
        // Project 3: notes (knowledge base)
        let notes = tmp.path().join("notes");
        fs::create_dir_all(&notes).unwrap();
        fs::write(
            notes.join("k8s-tips.md"),
            "# K8s Tips\n\nTroubleshooting CrashLoopBackOff.",
        )
        .unwrap();
        fs::write(
            notes.join("oauth-memo.md"),
            "# OAuth Memo\n\nOAuth 2.0 refresh token flow.",
        )
        .unwrap();
        // Hidden/template dirs (should be skipped)
        fs::create_dir_all(tmp.path().join(".hidden")).unwrap();
        fs::write(tmp.path().join(".hidden/secret.md"), "# Secret").unwrap();
        fs::create_dir_all(tmp.path().join("_template")).unwrap();
        fs::write(tmp.path().join("_template/TEMPLATE.md"), "# Template").unwrap();
        tmp
    }

    #[test]
    fn global_search_finds_across_projects() {
        let tmp = setup_multi_project_root();
        let args = json!({"query": "OAuth", "scope": "global"});
        let result = tool_search_global(tmp.path(), args).unwrap();
        let matches = result["matches"].as_array().unwrap();
        // OAuth appears in backend, frontend, and notes
        let projects: Vec<&str> = matches
            .iter()
            .filter_map(|m| m["project"].as_str())
            .collect();
        assert!(projects.contains(&"backend"), "should find in backend");
        assert!(projects.contains(&"frontend"), "should find in frontend");
        assert!(projects.contains(&"notes"), "should find in notes");
    }

    #[test]
    fn global_search_returns_scope_field() {
        let tmp = setup_multi_project_root();
        let args = json!({"query": "OAuth", "scope": "global"});
        let result = tool_search_global(tmp.path(), args).unwrap();
        assert_eq!(result["scope"], "global");
    }

    #[test]
    fn global_search_includes_project_field() {
        let tmp = setup_multi_project_root();
        let args = json!({"query": "K8s", "scope": "global"});
        let result = tool_search_global(tmp.path(), args).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty());
        // Every match must have a project field
        for m in matches {
            assert!(
                m["project"].is_string(),
                "each match must have project field"
            );
        }
    }

    #[test]
    fn global_search_respects_limit() {
        let tmp = setup_multi_project_root();
        let args = json!({"query": "#", "scope": "global", "limit": 2});
        let result = tool_search_global(tmp.path(), args).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(result["truncated"], true);
    }

    #[test]
    fn global_search_skips_hidden_and_template() {
        let tmp = setup_multi_project_root();
        let args = json!({"query": "Secret", "scope": "global"});
        let result = tool_search_global(tmp.path(), args).unwrap();
        let matches = result["matches"].as_array().unwrap();
        let projects: Vec<&str> = matches
            .iter()
            .filter_map(|m| m["project"].as_str())
            .collect();
        assert!(!projects.contains(&".hidden"), "should skip hidden dirs");
        assert!(
            !projects.contains(&"_template"),
            "should skip template dirs"
        );
    }

    #[test]
    fn global_search_no_results() {
        let tmp = setup_multi_project_root();
        let args = json!({"query": "zzz_nonexistent_zzz", "scope": "global"});
        let result = tool_search_global(tmp.path(), args).unwrap();
        assert!(result["matches"].as_array().unwrap().is_empty());
        assert_eq!(result["truncated"], false);
    }

    #[test]
    fn global_search_case_insensitive() {
        let tmp = setup_multi_project_root();
        let args = json!({"query": "oauth", "scope": "global"});
        let result = tool_search_global(tmp.path(), args).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(
            matches.len() >= 3,
            "case-insensitive should find OAuth matches"
        );
    }

    // -- audit: cross-repo duplicate detection --

    #[test]
    fn audit_detects_internal_docs_in_repo() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let repo = TempDir::new().unwrap();
        // Put an internal doc (PRD.md) in the project repo — this is a warning
        fs::write(repo.path().join("PRD.md"), "# PRD exposed publicly").unwrap();
        fs::write(repo.path().join("README.md"), "# README").unwrap();

        let result = tool_audit(&project_root, "testproj", Some(repo.path())).unwrap();
        let actions = result["suggested_actions"].as_array().unwrap();
        let has_exposed_warning = actions
            .iter()
            .any(|a| a["action"] == "resolve_exposed_internal_docs");
        assert!(
            has_exposed_warning,
            "should warn about internal docs in project repo"
        );
    }

    #[test]
    fn audit_without_repo_shows_not_detected() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let result = tool_audit(&project_root, "testproj", None).unwrap();
        assert_eq!(result["project_repo"]["status"], "not_detected");
    }

    // -- search: repo docs/ subdirectory --

    #[test]
    fn search_includes_repo_docs_subdir() {
        let tmp = setup_docs_root();
        let project_root = tmp.path().join("testproj");
        let repo = TempDir::new().unwrap();
        let docs_dir = repo.path().join("docs");
        fs::create_dir_all(&docs_dir).unwrap();
        fs::write(
            docs_dir.join("guide.md"),
            "# Guide\n\nUnique marker zxcvbn.",
        )
        .unwrap();

        let args = json!({"query": "zxcvbn"});
        let result = tool_search(&project_root, args, Some(repo.path())).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(
            matches
                .iter()
                .any(|m| m["source"] == "project-repo"
                    && m["file"].as_str().unwrap().contains("guide")),
            "should find file in repo's docs/ subdirectory"
        );
    }

    // -- tool_search_global: empty query --

    #[test]
    fn global_search_empty_query() {
        let tmp = setup_multi_project_root();
        let args = json!({"query": ""});
        let result = tool_search_global(tmp.path(), args).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(matches.is_empty(), "empty query should return no matches");
        assert_eq!(result["error"].as_str(), Some("empty query"));
    }

    // -- tool_configure_project --

    #[test]
    fn configure_project_creates_alcove_toml_in_repo_root() {
        let repo = TempDir::new().unwrap();
        let args = json!({
            "project_name": "myproj",
            "diagram_format": "plantuml"
        });
        let result = tool_configure_project(repo.path(), args).unwrap();
        assert_eq!(result["project_name"], "myproj");

        // alcove.toml must be written in the repo root
        let toml_path = repo.path().join("alcove.toml");
        assert!(toml_path.exists());
        let contents = fs::read_to_string(&toml_path).unwrap();
        assert!(contents.contains("plantuml"));
    }

    #[test]
    fn configure_project_updates_existing_preserves_other_fields() {
        let repo = TempDir::new().unwrap();
        // First call: set core_files
        tool_configure_project(
            repo.path(),
            json!({"project_name": "myproj", "core_files": ["SPEC.md"]}),
        )
        .unwrap();
        // Second call: set diagram_format only
        tool_configure_project(
            repo.path(),
            json!({"project_name": "myproj", "diagram_format": "plantuml"}),
        )
        .unwrap();

        // Both settings must be present
        let cfg = crate::config::load_project_config(repo.path());
        assert_eq!(cfg.diagram_format(), "plantuml");
        assert_eq!(cfg.core_files(), vec!["SPEC.md"]);
    }
}
