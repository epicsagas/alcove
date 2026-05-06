use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Index directory helpers
// ---------------------------------------------------------------------------

pub(crate) fn index_dir(docs_root: &Path) -> PathBuf {
    docs_root.join(".alcove").join("index")
}

pub(crate) fn meta_path(docs_root: &Path) -> PathBuf {
    docs_root.join(".alcove").join("index_meta.json")
}

// ---------------------------------------------------------------------------
// Index lock — prevents concurrent build/search races per docs_root
// ---------------------------------------------------------------------------

/// Maximum age (in seconds) for a lock file before it is considered stale.
/// If the lock holder crashes, the lock will be auto-cleared after this duration.
const LOCK_STALE_SECS: u64 = 600; // 10 minutes

pub(crate) fn lock_file(docs_root: &Path) -> PathBuf {
    docs_root.join(".alcove").join(".index_lock")
}

pub(crate) fn try_acquire_lock(docs_root: &Path) -> bool {
    let lock_path = lock_file(docs_root);
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // If a stale lock exists, remove it first
    if lock_path.exists() && is_lock_stale(&lock_path) {
        let _ = std::fs::remove_file(&lock_path);
    }
    // create_new is atomic (O_CREAT | O_EXCL) — if two processes race past the
    // stale-check above, only one will succeed here.
    match std::fs::File::create_new(&lock_path) {
        Ok(mut f) => {
            // Write PID directly to the opened fd — no window with empty content.
            use std::io::Write;
            let _ = write!(f, "{}", std::process::id());
            true
        }
        Err(_) => false,
    }
}

pub(crate) fn release_lock(docs_root: &Path) {
    let _ = std::fs::remove_file(lock_file(docs_root));
}

pub(crate) fn is_locked(docs_root: &Path) -> bool {
    let path = lock_file(docs_root);
    if !path.exists() {
        return false;
    }
    // Treat stale locks as not locked
    if is_lock_stale(&path) {
        let _ = std::fs::remove_file(&path);
        return false;
    }
    true
}

/// A lock is stale if it is older than `LOCK_STALE_SECS` or its PID is no longer running.
fn is_lock_stale(lock_path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(lock_path) else {
        return false;
    };

    // Check age
    if let Ok(modified) = meta.modified()
        && let Ok(elapsed) = modified.elapsed()
        && elapsed.as_secs() > LOCK_STALE_SECS
    {
        return true;
    }

    // Check if PID is still alive (Unix: kill -0)
    #[cfg(unix)]
    {
        if let Ok(content) = std::fs::read_to_string(lock_path)
            && let Ok(pid) = content.trim().parse::<u32>()
        {
            let status = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            if let Ok(s) = status
                && !s.success()
            {
                return true; // Process doesn't exist
            }
        }
    }

    false
}
