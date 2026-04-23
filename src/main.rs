mod cli;
mod config;
#[cfg(feature = "alcove-full")]
mod embedding;
mod index;
mod lint;
#[cfg(feature = "alcove-server")]
mod launchd;
mod mcp;
mod policy;
mod promote;
mod tools;
mod vault;

#[cfg(feature = "alcove-full")]
mod vector;

#[cfg(feature = "alcove-server")]
mod server;

use std::io::{self, BufRead, Write as _};

use anyhow::Result;
use clap::{Parser, Subcommand};

rust_i18n::i18n!("locales", fallback = "en");

/// Detect system locale and set i18n language.
/// Supports: en, ko, zh-CN, ja, es, hi, pt-BR, de, fr, ru
fn init_locale() {
    use std::env;
    let locale = env::var("ALCOVE_LANG")
        .ok()
        .or_else(sys_locale::get_locale)
        .unwrap_or_else(|| "en".to_string());
    let lang = match locale.as_str() {
        s if s.starts_with("ko") => "ko",
        s if s.starts_with("zh") => "zh-CN",
        s if s.starts_with("ja") => "ja",
        s if s.starts_with("es") => "es",
        s if s.starts_with("hi") => "hi",
        s if s.starts_with("pt") => "pt-BR",
        s if s.starts_with("de") => "de",
        s if s.starts_with("fr") => "fr",
        s if s.starts_with("ru") => "ru",
        _ => "en",
    };
    rust_i18n::set_locale(lang);
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "alcove", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Interactive setup: docs root, categories, diagram format, agents
    Setup,
    /// Remove skills, config, and legacy files
    Uninstall,
    /// Validate project docs against policy
    Validate {
        /// Output format: human (default) or json
        #[arg(long, default_value = "human")]
        format: String,
        /// Exit with code 1 on validation failure (for CI)
        #[arg(long)]
        exit_code: bool,
    },
    /// Update the search index (incremental — only changed files)
    Index,
    /// Rebuild the search index from scratch (drops and recreates all data)
    Rebuild,
    /// Check the health of the alcove installation
    Doctor {
        /// Output format: human (default) or json
        #[arg(long, default_value = "human")]
        format: String,
    },
    /// Search across project docs from the command line
    Search {
        /// Search query
        query: String,
        /// Search scope: global (default) or project
        #[arg(long, default_value = "global")]
        scope: String,
        /// Search mode: auto (default, ranked if index exists, else grep), grep, or ranked
        #[arg(long, default_value = "auto")]
        mode: String,
        /// Max results
        #[arg(long, default_value = "20")]
        limit: usize,
    },
    /// Manage embedding models for hybrid search
    #[cfg(feature = "alcove-full")]
    Model {
        #[command(subcommand)]
        subcmd: ModelCommands,
    },
    /// Lint project docs for broken links, orphans, stale markers
    Lint {
        /// Output format: human (default) or json
        #[arg(long, default_value = "human")]
        format: String,
    },
    /// Promote a document from an external vault into alcove docs
    Promote {
        /// Source file path
        source: std::path::PathBuf,
        /// Target project (auto-detected if omitted)
        #[arg(long)]
        project: Option<String>,
        /// Move instead of copy
        #[arg(long)]
        mv: bool,
    },
    /// Start HTTP RAG server for external access
    #[cfg(feature = "alcove-server")]
    Serve {
        /// Host / bind address (default: 127.0.0.1, use 0.0.0.0 for all interfaces)
        #[arg(long)]
        host: Option<String>,
        /// Port to listen on
        #[arg(long)]
        port: Option<u16>,
        /// Bearer token for authentication (optional)
        #[arg(long)]
        token: Option<String>,
    },
    /// Register alcove serve as a macOS login item and start it
    #[cfg(feature = "alcove-server")]
    Enable,
    /// Unregister alcove from login items and stop it
    #[cfg(feature = "alcove-server")]
    Disable,
    /// Start the background alcove serve process
    #[cfg(feature = "alcove-server")]
    Start,
    /// Stop the background alcove serve process
    #[cfg(feature = "alcove-server")]
    Stop,
    /// Restart the background alcove serve process
    #[cfg(feature = "alcove-server")]
    Restart,
    /// Manage knowledge base vaults
    Vault {
        #[command(subcommand)]
        subcmd: VaultCommands,
    },
    /// Print the bearer token from config (for team sharing)
    Token,
}

#[derive(Subcommand)]
enum VaultCommands {
    /// Create a new empty vault
    Create { name: String },
    /// Link an external directory as a vault (e.g., Obsidian vault)
    Link {
        name: String,
        path: std::path::PathBuf,
    },
    /// List all vaults
    List,
    /// Remove a vault (symlinks: remove link only; directories: remove all)
    Remove { name: String },
    /// Add a document to a vault
    Add {
        vault: String,
        source: std::path::PathBuf,
    },
    /// Build search index for vaults
    Index { name: Option<String> },
    /// Rebuild vault search index from scratch
    Rebuild { name: Option<String> },
}

#[derive(Subcommand)]
#[cfg(feature = "alcove-full")]
enum ModelCommands {
    /// List available embedding models
    List,
    /// Download the configured embedding model
    Download,
    /// Remove cached embedding model to free disk space
    Remove,
    /// Set the embedding model (updates config.toml)
    Set {
        /// Model name (e.g., MultilingualE5Small, SnowflakeArcticEmbedXS)
        model: String,
    },
    /// Show current model status
    Status,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    config::migrate_legacy_paths();
    init_locale();
    let cli = {
        use clap::{CommandFactory, FromArgMatches};
        use rust_i18n::t;
        let cmd = Cli::command().about(t!("about").to_string());
        let mut matches = cmd.get_matches();
        Cli::from_arg_matches_mut(&mut matches)?
    };

    match cli.command {
        None => serve(),
        Some(Commands::Setup) => cli::cmd_setup(),
        Some(Commands::Uninstall) => cli::cmd_uninstall(),
        Some(Commands::Validate { format, exit_code }) => cli::cmd_validate(&format, exit_code),
        Some(Commands::Index) => cli::cmd_index(),
        Some(Commands::Rebuild) => cli::cmd_rebuild(),
        Some(Commands::Doctor { format }) => cli::cmd_doctor(&format),
        Some(Commands::Search {
            query,
            scope,
            mode,
            limit,
        }) => cli::cmd_search(&query, &scope, &mode, limit),
        Some(Commands::Lint { format }) => cli::cmd_lint(&format),
        Some(Commands::Promote { source, project, mv }) => {
            cli::cmd_promote(&source, project.as_deref(), mv)
        }
        Some(Commands::Vault { subcmd }) => match subcmd {
            VaultCommands::Create { name } => {
                let path = vault::create_vault(&name)?;
                println!("  \u{2713} Created vault '{}' at {}", name, path.display());
                Ok(())
            }
            VaultCommands::Link { name, path } => {
                let vault_path = vault::link_vault(&name, &path)?;
                let _ = vault_path;
                println!("  \u{2713} Linked vault '{}' \u{2192} {}", name, path.display());
                Ok(())
            }
            VaultCommands::List => {
                let vaults = vault::list_vaults()?;
                if vaults.is_empty() {
                    println!("  No vaults found. Create one with: alcove vault create <name>");
                } else {
                    for v in &vaults {
                        let link_indicator = if v.is_link { " \u{2192} (linked)" } else { "" };
                        println!("  {} ({} docs){}", v.name, v.doc_count, link_indicator);
                    }
                    println!("\n  {} vault(s) total", vaults.len());
                }
                Ok(())
            }
            VaultCommands::Remove { name } => {
                vault::remove_vault(&name)?;
                println!("  \u{2713} Removed vault '{}'", name);
                Ok(())
            }
            VaultCommands::Add { vault, source } => {
                let dest = vault::add_to_vault(&vault, &source)?;
                println!(
                    "  \u{2713} Added {} to vault '{}'",
                    dest.file_name().unwrap_or_default().to_string_lossy(),
                    vault
                );
                Ok(())
            }
            VaultCommands::Index { name } => {
                if let Some(name) = name {
                    let vault_path = vault::vaults_root().join(&name);
                    if !vault_path.is_dir() {
                        anyhow::bail!("Vault '{}' not found", name);
                    }
                    let result = index::build_vault_index(&vault_path)?;
                    let files = result["files"].as_u64().unwrap_or(0);
                    let vectors = result["vectors_indexed"].as_u64().unwrap_or(0);
                    let vec_status = result["vector_status"].as_str().unwrap_or("disabled");
                    let model = result["embedding_model"].as_str().unwrap_or("");
                    if vectors > 0 {
                        println!("  ✓ Indexed vault '{}' ({} files, {} vectors via {})", name, files, vectors, model);
                    } else if vec_status != "disabled" {
                        println!("  ✓ Indexed vault '{}' ({} files, vectors: {})", name, files, vec_status);
                    } else {
                        println!("  ✓ Indexed vault '{}' ({} files)", name, files);
                    }
                } else {
                    let result = index::build_all_vault_indexes()?;
                    let indexed = result["vaults_indexed"].as_u64().unwrap_or(0);
                    let failed = result["vaults_failed"].as_u64().unwrap_or(0);
                    if failed > 0 {
                        println!("  ✓ Indexed {} vault(s), {} failed", indexed, failed);
                    } else {
                        println!("  ✓ Indexed {} vault(s)", indexed);
                    }
                }
                Ok(())
            }
            VaultCommands::Rebuild { name } => {
                if let Some(name) = name {
                    let vault_path = vault::vaults_root().join(&name);
                    if !vault_path.is_dir() {
                        anyhow::bail!("Vault '{}' not found", name);
                    }
                    let result = index::rebuild_vault_index(&vault_path)?;
                    let files = result["files"].as_u64().unwrap_or(0);
                    let vectors = result["vectors_indexed"].as_u64().unwrap_or(0);
                    let model = result["embedding_model"].as_str().unwrap_or("");
                    if vectors > 0 {
                        println!("  \u{2713} Rebuilt vault '{}' ({} files, {} vectors via {})", name, files, vectors, model);
                    } else {
                        println!("  \u{2713} Rebuilt vault '{}' ({} files)", name, files);
                    }
                } else {
                    for v in vault::list_vaults()? {
                        let result = index::rebuild_vault_index(&v.path)?;
                        let files = result["files"].as_u64().unwrap_or(0);
                        let vectors = result["vectors_indexed"].as_u64().unwrap_or(0);
                        let model = result["embedding_model"].as_str().unwrap_or("");
                        if vectors > 0 {
                            println!("  \u{2713} Rebuilt vault '{}' ({} files, {} vectors via {})", v.name, files, vectors, model);
                        } else {
                            println!("  \u{2713} Rebuilt vault '{}' ({} files)", v.name, files);
                        }
                    }
                }
                Ok(())
            }
        },
        #[cfg(feature = "alcove-full")]
        Some(Commands::Model { subcmd }) => cli::cmd_model(subcmd),
        #[cfg(feature = "alcove-server")]
        Some(Commands::Enable) => launchd::enable(),
        #[cfg(feature = "alcove-server")]
        Some(Commands::Disable) => launchd::disable(),
        #[cfg(feature = "alcove-server")]
        Some(Commands::Start) => launchd::start(),
        #[cfg(feature = "alcove-server")]
        Some(Commands::Stop) => launchd::stop(),
        #[cfg(feature = "alcove-server")]
        Some(Commands::Restart) => launchd::restart(),
        Some(Commands::Token) => cli::cmd_token(),
        #[cfg(feature = "alcove-server")]
        Some(Commands::Serve { host, port, token }) => {
            let cfg = config::load_config();
            let docs_root = cfg
                .docs_root()
                .ok_or_else(|| anyhow::anyhow!("docs_root not configured. Run 'alcove setup' first."))?;
            
            // Resolve host: CLI flag > config.toml > default (127.0.0.1)
            let srv_cfg = cfg.server_config();
            let bind_host = host.as_deref().unwrap_or(&srv_cfg.host);
            // Resolve port: CLI flag > config.toml > default (8080)
            let bind_port = port.unwrap_or(srv_cfg.port);
            // Resolve token: CLI flag > config.toml > none
            let resolved_token = token.as_ref()
                .cloned()
                .or_else(|| {
                    cfg.server.as_ref().and_then(|s| s.token.clone())
                });

            println!("{}", console::style("Starting Alcove RAG server...").bold());
            println!(
                "  {} Docs root: {}",
                console::style("→").dim(),
                docs_root.display()
            );
            println!(
                "  {} Bind: {}:{}",
                console::style("→").dim(),
                bind_host, bind_port
            );
            println!();

            // Create tokio runtime for async server
            tokio::runtime::Runtime::new()
                .expect("Failed to create tokio runtime")
                .block_on(server::run_server(docs_root, bind_host, bind_port, resolved_token))
        }
    }
}

// ---------------------------------------------------------------------------
// MCP server — stdio JSON-RPC loop (hybrid: proxy or direct)
// ---------------------------------------------------------------------------

/// Check if the background HTTP server is alive and return its base URL.
fn detect_proxy_target() -> Option<String> {
    let cfg = config::load_config();
    let (host, port) = cfg
        .server
        .as_ref()
        .map(|s| (s.host.as_str(), s.port))
        .unwrap_or(("127.0.0.1", 8080));
    let base = format!("http://{host}:{port}");

    match ureq::get(&format!("{base}/health"))
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(2)))
        .build()
        .call()
    {
        Ok(resp) if resp.status() == 200 => {
            eprintln!("[alcove] proxy mode → {base}");
            Some(base)
        }
        _ => None,
    }
}

/// Forward a raw JSON-RPC line to the HTTP server's /mcp endpoint.
fn proxy_request(base: &str, line: &str, token: Option<&str>) -> Option<String> {
    let url = format!("{base}/mcp");
    let mut req = ureq::post(&url).header("Content-Type", "application/json");
    if let Some(t) = token {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }
    match req.send(line) {
        Ok(mut resp) if resp.status() == 200 => resp.body_mut().read_to_string().ok(),
        Ok(resp) if resp.status() == 204 => None, // notification, no response
        _ => {
            // If the request has an id, return a JSON-RPC error so the client isn't left hanging
            if let Some(err) = serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|v| v.get("id").map(|id| {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32603, "message": "Proxy request to background server failed"}
                    })
                }))
            {
                return Some(err.to_string());
            }
            None
        }
    }
}

fn serve() -> Result<()> {
    let proxy_base = detect_proxy_target();
    // Token: env var > config.toml
    let token: Option<String> = std::env::var("ALCOVE_TOKEN")
        .ok()
        .or_else(|| {
            config::load_config()
                .server
                .as_ref()
                .and_then(|s| s.token.clone())
        });

    // In direct mode, build BM25 index in background
    if proxy_base.is_none() {
        eprintln!("[alcove] direct mode (no background server detected)");
        std::thread::spawn(|| {
            if let Some(docs_root) = config::load_config().docs_root()
                && docs_root.is_dir()
            {
                let _ = index::build_index_bm25_only(&docs_root);
            }
        });
    }

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        // Proxy mode: forward to HTTP server
        if let Some(ref base) = proxy_base {
            if let Some(resp_body) = proxy_request(base, &line, token.as_deref()) {
                // Skip null responses (notifications)
                if resp_body.trim() != "null" {
                    writeln!(stdout, "{}", resp_body)?;
                    stdout.flush()?;
                }
            }
            continue;
        }

        // Direct mode: handle locally
        let req: mcp::RpcRequest = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let resp =
                    mcp::RpcResponse::err(None, -32700, format!("Failed to parse request: {e}"));
                writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
                stdout.flush()?;
                continue;
            }
        };

        if let Some(resp) = mcp::dispatch(req) {
            writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
            stdout.flush()?;
        }
    }

    Ok(())
}
