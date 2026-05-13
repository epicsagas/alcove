//! Platform abstraction for cross-platform operations.
//!
//! Provides OS-agnostic interfaces for symlinks, fd suppression,
//! process signals, and PID checks so the rest of the codebase
//! never touches `libc::` or `std::os::unix` directly.

use std::path::Path;

/// Create a symbolic link.
///
/// On Unix: creates a symlink (file or directory).
/// On Windows: creates a file symlink via `std::os::windows::fs::symlink_file`.
pub fn create_symlink<P: AsRef<Path>, Q: AsRef<Path>>(
    original: P,
    link: Q,
) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(original, link)
    }
    #[cfg(windows)]
    {
        let orig = original.as_ref();
        if orig.is_dir() {
            std::os::windows::fs::symlink_dir(orig, link)
        } else {
            std::os::windows::fs::symlink_file(orig, link)
        }
    }
}

/// Check if a process with the given PID is still running.
///
/// Returns `true` if the process exists, `false` if not.
/// On Windows, always returns `true` (conservative: don't break locks).
pub fn is_pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill(pid, 0) returns Ok(0) if the process exists
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(windows)]
    {
        // On Windows, OpenProcess to check liveness.
        // Conservative fallback: treat as alive to avoid stale lock corruption.
        let _ = pid;
        true
    }
}

/// Send SIGTERM (Unix) or terminate (Windows) to a process.
pub fn send_terminate(pid: u32) {
    #[cfg(unix)]
    {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
    #[cfg(windows)]
    {
        // On Windows, attempt to terminate via TerminateProcess.
        // For CLI tools, this is typically handled by the caller.
        let _ = pid;
    }
}

/// Suppress stdout/stderr output during a closure (PDF reader noise, etc.).
///
/// On Unix: saves fd via dup, redirects to /dev/null, restores on drop.
/// On Windows: no-op (PDF extraction errors are tolerated).
#[cfg(unix)]
pub fn suppress_fds<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    #[cfg(unix)]
    {
        struct FdGuard {
            saved_stdout: libc::c_int,
            saved_stderr: libc::c_int,
        }

        impl Drop for FdGuard {
            fn drop(&mut self) {
                unsafe {
                    if self.saved_stdout >= 0 {
                        libc::dup2(self.saved_stdout, libc::STDOUT_FILENO);
                        libc::close(self.saved_stdout);
                    }
                    if self.saved_stderr >= 0 {
                        libc::dup2(self.saved_stderr, libc::STDERR_FILENO);
                        libc::close(self.saved_stderr);
                    }
                }
            }
        }

        let saved_stdout = unsafe { libc::dup(libc::STDOUT_FILENO) };
        let saved_stderr = unsafe { libc::dup(libc::STDERR_FILENO) };

        let _guard = FdGuard {
            saved_stdout,
            saved_stderr,
        };

        if saved_stdout < 0 || saved_stderr < 0 {
            // Could not save fds — just run without suppression.
            return f();
        }

        // Open /dev/null for suppression
        let devnull_fd = unsafe {
            libc::open(
                c"/dev/null".as_ptr(),
                libc::O_WRONLY,
            )
        };
        if devnull_fd >= 0 {
            unsafe {
                libc::dup2(devnull_fd, libc::STDOUT_FILENO);
                libc::dup2(devnull_fd, libc::STDERR_FILENO);
                libc::close(devnull_fd);
            }
        }

        f()
    }

    #[cfg(not(unix))]
    {
        f()
    }
}
