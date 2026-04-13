mod cli;
mod config;
mod embedding;
mod index;
mod lint;
#[cfg(feature = "alcove-server")]
mod launchd;
mod mcp;
mod policy;
mod promote;
mod tools;

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
                .block_on(server::run_server(docs_root, bind_host, bind_port, token))
        }
    }
}

// ---------------------------------------------------------------------------
// MCP server — stdio JSON-RPC loop
// ---------------------------------------------------------------------------

fn serve() -> Result<()> {
    // Background BM25-only index build on server start.
    // Vector embedding is deferred until the first hybrid search request
    // when the model is already loaded — avoids blocking server readiness
    // and unnecessary ONNX model loading on startup.
    std::thread::spawn(|| {
        if let Some(docs_root) = config::load_config().docs_root()
            && docs_root.is_dir()
        {
            let _ = index::build_index_bm25_only(&docs_root);
        }
    });

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

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
