mod cli;
mod config;
mod embedding;
mod index;
mod mcp;
mod policy;
mod tools;

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
    /// Build or rebuild the full-text search index for ranked search
    Index,
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
        /// Search scope: project (default) or global
        #[arg(long, default_value = "project")]
        scope: String,
        /// Search mode: auto (default, ranked if index exists, else grep), grep, or ranked
        #[arg(long, default_value = "auto")]
        mode: String,
        /// Max results
        #[arg(long, default_value = "20")]
        limit: usize,
    },
    /// Manage embedding models for hybrid search (alcove-full feature)
    Model {
        #[command(subcommand)]
        subcmd: ModelCommands,
    },
}

#[derive(Subcommand)]
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
        Some(Commands::Doctor { format }) => cli::cmd_doctor(&format),
        Some(Commands::Search {
            query,
            scope,
            mode,
            limit,
        }) => cli::cmd_search(&query, &scope, &mode, limit),
        Some(Commands::Model { subcmd }) => cli::cmd_model(subcmd),
    }
}

// ---------------------------------------------------------------------------
// MCP server — stdio JSON-RPC loop
// ---------------------------------------------------------------------------

fn serve() -> Result<()> {
    // Background index build on server start
    std::thread::spawn(|| {
        if let Some(docs_root) = config::load_config().docs_root()
            && docs_root.is_dir()
        {
            let _ = index::build_index(&docs_root);
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
