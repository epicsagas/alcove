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

/// Maximum age (in seconds) for a lock file whose holder PID cannot be read.
/// A lock with a readable PID is stale only when that process is gone: a full
/// embedding run on a large vault takes hours, and expiring a live holder's
/// lock by age let a second `alcove index` run concurrently against the same
/// vector store.
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

/// Remove the lock only if this process holds it, so a finishing run never
/// deletes a lock that another process acquired after treating ours as stale.
pub(crate) fn release_lock(docs_root: &Path) {
    let path = lock_file(docs_root);
    if lock_pid(&path) == Some(std::process::id()) {
        let _ = std::fs::remove_file(path);
    }
}

fn lock_pid(lock_path: &Path) -> Option<u32> {
    std::fs::read_to_string(lock_path)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
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

/// A lock is stale if its PID is no longer running. Only a lock without a
/// readable PID falls back to the `LOCK_STALE_SECS` age limit.
fn is_lock_stale(lock_path: &Path) -> bool {
    if let Some(pid) = lock_pid(lock_path) {
        return !crate::platform::is_pid_alive(pid);
    }
    std::fs::metadata(lock_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age.as_secs() > LOCK_STALE_SECS)
}
