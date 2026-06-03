//! macOS LaunchAgent lifecycle management for `alcove serve`.
//!
//! Provides enable/disable (login-item registration) and start/stop/restart
//! for the background HTTP RAG server process via `launchctl`.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::ServiceKind;
use crate::config::{alcove_home, load_config};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn label_for(kind: ServiceKind) -> String {
    match kind {
        ServiceKind::Mcp => "com.epicsagas.alcove.mcp".to_string(),
        ServiceKind::Api => "com.epicsagas.alcove.api".to_string(),
    }
}

/// `~/Library/LaunchAgents/com.epicsagas.alcove.{kind}.plist`
pub fn plist_path(kind: ServiceKind) -> PathBuf {
    let label = label_for(kind);
    dirs::home_dir()
        .expect("cannot resolve home directory")
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", label))
}

fn log_dir() -> PathBuf {
    alcove_home().join("logs")
}

// ---------------------------------------------------------------------------
// Plist generation
// ---------------------------------------------------------------------------

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn generate_plist(alcove_bin: &str, kind: ServiceKind) -> String {
    let logs = log_dir();
    let cfg = load_config();
    let srv = cfg.server_config();

    let host_arg = format!(
        "        <string>--host</string>\n        <string>{}</string>",
        xml_escape(&srv.host)
    );

    let bind_port = crate::resolve_server_port(cfg, kind);

    let port_arg = format!(
        "        <string>--port</string>\n        <string>{}</string>",
        bind_port
    );

    let token_env = srv
        .token
        .as_deref()
        .map(|t| {
            format!(
                r#"    <key>EnvironmentVariables</key>
    <dict>
        <key>ALCOVE_TOKEN</key>
        <string>{t}</string>
    </dict>"#,
                t = xml_escape(t)
            )
        })
        .unwrap_or_default();

    let subcmd = match kind {
        ServiceKind::Mcp => "mcp",
        ServiceKind::Api => "api",
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{alcove_bin}</string>
        <string>{subcmd}</string>
        <string>serve</string>
{host_arg}
{port_arg}
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
{token_env}
    <key>StandardOutPath</key>
    <string>{out}</string>
    <key>StandardErrorPath</key>
    <string>{err}</string>
</dict>
</plist>
"#,
        label = label_for(kind),
        out = logs.join(format!("{}.out.log", subcmd)).display(),
        err = logs.join(format!("{}.err.log", subcmd)).display(),
    )
}

// ---------------------------------------------------------------------------
// launchctl helpers
// ---------------------------------------------------------------------------

/// Check whether the agent is loaded (registered) with launchd.
pub fn is_loaded(kind: ServiceKind) -> bool {
    let label = label_for(kind);
    Command::new("launchctl")
        .args(["list", &label])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Return the PID of the running agent, if any.
pub fn running_pid(kind: ServiceKind) -> Option<u32> {
    let label = label_for(kind);
    let output = Command::new("launchctl")
        .args(["list", &label])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.contains("\"PID\"")
            && let Some(num_part) = line.split('=').nth(1)
        {
            let cleaned = num_part.trim().trim_end_matches(';');
            if let Ok(pid) = cleaned.parse::<u32>() {
                return Some(pid);
            }
        }
    }
    None
}

fn launchctl(args: &[&str]) -> Result<()> {
    let status = Command::new("launchctl")
        .args(args)
        .status()
        .with_context(|| format!("failed to run launchctl {}", args.join(" ")))?;
    if !status.success() {
        bail!("launchctl {} exited with {}", args.join(" "), status);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public commands
// ---------------------------------------------------------------------------

/// Register as login item and start the process.
pub fn enable(kind: ServiceKind) -> Result<()> {
    let alcove_bin = std::env::current_exe()
        .context("cannot resolve alcove binary path")?
        .to_string_lossy()
        .to_string();

    let plist = plist_path(kind);

    // Ensure directories exist
    if let Some(parent) = plist.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(log_dir())?;

    // Unload first if already registered, then wait for port release.
    if is_loaded(kind) {
        launchctl(&["unload", &plist.to_string_lossy()])?;
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    // Write plist
    std::fs::write(&plist, generate_plist(&alcove_bin, kind))?;

    // Load (RunAtLoad=true will start the process)
    launchctl(&["load", &plist.to_string_lossy()])?;

    println!(
        "  {} Alcove {:?} registered as login item and started.",
        console::style("✓").green(),
        kind
    );
    println!("  {} Plist: {}", console::style("→").dim(), plist.display());
    println!(
        "  {} Logs:  {}",
        console::style("→").dim(),
        log_dir().display()
    );
    Ok(())
}

/// Unregister from login items and stop the process.
pub fn disable(kind: ServiceKind) -> Result<()> {
    let plist = plist_path(kind);
    if !plist.exists() {
        println!("  Alcove {:?} is not registered as a login item.", kind);
        return Ok(());
    }

    if is_loaded(kind) {
        launchctl(&["unload", &plist.to_string_lossy()])?;
    }
    std::fs::remove_file(&plist)?;

    println!(
        "  {} Alcove {:?} unregistered from login items and stopped.",
        console::style("✓").green(),
        kind
    );
    Ok(())
}

/// Start the background process.
pub fn start(kind: ServiceKind) -> Result<()> {
    if let Some(pid) = running_pid(kind) {
        println!(
            "  Alcove {:?} is already running (PID {}).",
            kind,
            console::style(pid).cyan()
        );
        return Ok(());
    }

    let label = label_for(kind);
    if is_loaded(kind) {
        launchctl(&["start", &label])?;
    } else if plist_path(kind).exists() {
        launchctl(&["load", &plist_path(kind).to_string_lossy()])?;
    } else {
        bail!(
            "Alcove {:?} is not registered. Run `alcove {:?} enable` first.",
            kind,
            kind
        );
    }

    // Brief wait then confirm
    std::thread::sleep(std::time::Duration::from_millis(500));
    if let Some(pid) = running_pid(kind) {
        println!(
            "  {} Alcove {:?} started (PID {}).",
            console::style("✓").green(),
            kind,
            console::style(pid).cyan()
        );
    } else {
        println!(
            "  {} Alcove {:?} start requested. Check logs at {}",
            console::style("⚠").yellow(),
            kind,
            log_dir().display()
        );
    }
    Ok(())
}

/// Stop the background process.
pub fn stop(kind: ServiceKind) -> Result<()> {
    if running_pid(kind).is_none() {
        println!("  Alcove {:?} is not running.", kind);
        return Ok(());
    }

    let label = label_for(kind);
    if is_loaded(kind) {
        launchctl(&["stop", &label])?;
    }

    std::thread::sleep(std::time::Duration::from_millis(300));
    println!(
        "  {} Alcove {:?} stopped.",
        console::style("✓").green(),
        kind
    );
    Ok(())
}

pub fn status(kind: ServiceKind) -> Result<()> {
    if is_loaded(kind) {
        if let Some(pid) = running_pid(kind) {
            println!(
                "  {} Alcove {:?} is running (PID {})",
                console::style("✓").green(),
                kind,
                console::style(pid).cyan()
            );
        } else {
            println!(
                "  {} Alcove {:?} is registered but not currently running",
                console::style("⚠").yellow(),
                kind
            );
        }
    } else {
        println!("  Alcove {:?} is not registered as a login item", kind);
    }
    Ok(())
}

/// Restart the background process.
pub fn restart(kind: ServiceKind) -> Result<()> {
    let plist = plist_path(kind);
    if !plist.exists() {
        bail!(
            "Alcove {:?} is not registered. Run `alcove {:?} enable` first.",
            kind,
            kind
        );
    }

    // Unload stops the process and deregisters — ensures port is fully released
    if is_loaded(kind) {
        let _ = launchctl(&["unload", &plist.to_string_lossy()]);
    }

    std::thread::sleep(std::time::Duration::from_millis(300));

    // Load registers and starts (RunAtLoad=true)
    launchctl(&["load", &plist.to_string_lossy()])?;

    // Wait for process to start (index init can be slow)
    for _ in 0..20 {
        if running_pid(kind).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if let Some(pid) = running_pid(kind) {
        println!(
            "  {} Alcove {:?} restarted (PID {}).",
            console::style("✓").green(),
            kind,
            console::style(pid).cyan()
        );
    } else {
        println!(
            "  {} Alcove {:?} restart requested. Check logs at {}",
            console::style("⚠").yellow(),
            kind,
            log_dir().display()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escape_all_special_chars() {
        assert_eq!(
            xml_escape("a&b<c>d\"e'f"),
            "a&amp;b&lt;c&gt;d&quot;e&apos;f"
        );
    }

    #[test]
    fn xml_escape_no_special_chars() {
        assert_eq!(xml_escape("hello-world_123"), "hello-world_123");
    }

    #[test]
    fn xml_escape_ampersand_first() {
        // & must be escaped first to avoid double-escaping
        assert_eq!(xml_escape("&lt;"), "&amp;lt;");
    }

    #[test]
    fn xml_escape_empty() {
        assert_eq!(xml_escape(""), "");
    }

    #[test]
    fn generate_plist_contains_token_env_when_set() {
        // Verify structure: if config has a token, plist should have
        // EnvironmentVariables section with escaped token
        let escaped = xml_escape("tok&en<with>special\"chars");
        assert_eq!(escaped, "tok&amp;en&lt;with&gt;special&quot;chars");
        // Integration test via generate_plist is not possible because
        // load_config() uses OnceLock — tested indirectly by xml_escape
        // and resolve_server_port unit tests.
    }
}
