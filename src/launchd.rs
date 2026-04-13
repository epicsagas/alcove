//! macOS LaunchAgent lifecycle management for `alcove serve`.
//!
//! Provides enable/disable (login-item registration) and start/stop/restart
//! for the background HTTP RAG server process via `launchctl`.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::config::alcove_home;

const LABEL: &str = "com.epicsagas.alcove";

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// `~/Library/LaunchAgents/com.epicsagas.alcove.plist`
pub fn plist_path() -> PathBuf {
    dirs::home_dir()
        .expect("cannot resolve home directory")
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

fn log_dir() -> PathBuf {
    alcove_home().join("logs")
}

// ---------------------------------------------------------------------------
// Plist generation
// ---------------------------------------------------------------------------

fn generate_plist(alcove_bin: &str) -> String {
    let logs = log_dir();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{alcove_bin}</string>
        <string>serve</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
    <key>StandardOutPath</key>
    <string>{out}</string>
    <key>StandardErrorPath</key>
    <string>{err}</string>
</dict>
</plist>
"#,
        out = logs.join("serve.out.log").display(),
        err = logs.join("serve.err.log").display(),
    )
}

// ---------------------------------------------------------------------------
// launchctl helpers
// ---------------------------------------------------------------------------

/// Check whether the agent is loaded (registered) with launchd.
pub fn is_loaded() -> bool {
    Command::new("launchctl")
        .args(["list", LABEL])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Return the PID of the running agent, if any.
pub fn running_pid() -> Option<u32> {
    let output = Command::new("launchctl")
        .args(["list", LABEL])
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
pub fn enable() -> Result<()> {
    let alcove_bin = std::env::current_exe()
        .context("cannot resolve alcove binary path")?
        .to_string_lossy()
        .to_string();

    let plist = plist_path();

    // Ensure directories exist
    if let Some(parent) = plist.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(log_dir())?;

    // Unload first if already registered
    if is_loaded() {
        let _ = launchctl(&["unload", &plist.to_string_lossy()]);
    }

    // Write plist
    std::fs::write(&plist, generate_plist(&alcove_bin))?;

    // Load (RunAtLoad=true will start the process)
    launchctl(&["load", &plist.to_string_lossy()])?;

    println!(
        "  {} Alcove registered as login item and started.",
        console::style("✓").green()
    );
    println!(
        "  {} Plist: {}",
        console::style("→").dim(),
        plist.display()
    );
    println!(
        "  {} Logs:  {}",
        console::style("→").dim(),
        log_dir().display()
    );
    Ok(())
}

/// Unregister from login items and stop the process.
pub fn disable() -> Result<()> {
    let plist = plist_path();
    if !plist.exists() {
        println!("  Alcove is not registered as a login item.");
        return Ok(());
    }

    if is_loaded() {
        launchctl(&["unload", &plist.to_string_lossy()])?;
    }
    std::fs::remove_file(&plist)?;

    println!(
        "  {} Alcove unregistered from login items and stopped.",
        console::style("✓").green()
    );
    Ok(())
}

/// Start the background process.
pub fn start() -> Result<()> {
    if let Some(pid) = running_pid() {
        println!(
            "  Alcove is already running (PID {}).",
            console::style(pid).cyan()
        );
        return Ok(());
    }

    if is_loaded() {
        launchctl(&["start", LABEL])?;
    } else if plist_path().exists() {
        launchctl(&["load", &plist_path().to_string_lossy()])?;
    } else {
        bail!("Alcove is not registered. Run `alcove enable` first.");
    }

    // Brief wait then confirm
    std::thread::sleep(std::time::Duration::from_millis(500));
    if let Some(pid) = running_pid() {
        println!(
            "  {} Alcove started (PID {}).",
            console::style("✓").green(),
            console::style(pid).cyan()
        );
    } else {
        println!(
            "  {} Alcove start requested. Check logs at {}",
            console::style("⚠").yellow(),
            log_dir().display()
        );
    }
    Ok(())
}

/// Stop the background process.
pub fn stop() -> Result<()> {
    if running_pid().is_none() {
        println!("  Alcove is not running.");
        return Ok(());
    }

    if is_loaded() {
        launchctl(&["stop", LABEL])?;
    }

    std::thread::sleep(std::time::Duration::from_millis(300));
    println!(
        "  {} Alcove stopped.",
        console::style("✓").green()
    );
    Ok(())
}

/// Restart the background process.
pub fn restart() -> Result<()> {
    if running_pid().is_some() {
        if is_loaded() {
            launchctl(&["stop", LABEL])?;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    if is_loaded() {
        launchctl(&["start", LABEL])?;
    } else if plist_path().exists() {
        launchctl(&["load", &plist_path().to_string_lossy()])?;
    } else {
        bail!("Alcove is not registered. Run `alcove enable` first.");
    }

    std::thread::sleep(std::time::Duration::from_millis(500));
    if let Some(pid) = running_pid() {
        println!(
            "  {} Alcove restarted (PID {}).",
            console::style("✓").green(),
            console::style(pid).cyan()
        );
    } else {
        println!(
            "  {} Alcove restart requested. Check logs at {}",
            console::style("⚠").yellow(),
            log_dir().display()
        );
    }
    Ok(())
}
