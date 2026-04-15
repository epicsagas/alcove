use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use walkdir::WalkDir;

use crate::config::alcove_home;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct VaultInfo {
    pub name: String,
    pub path: PathBuf,
    pub is_link: bool,
    pub doc_count: usize,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Validate that `name` is a single normal path component without `_` or `.` prefix.
fn validate_vault_name(name: &str) -> Result<()> {
    let p = Path::new(name);
    let components: Vec<_> = p.components().collect();
    if components.len() != 1 || !matches!(components[0], Component::Normal(_)) {
        anyhow::bail!("Invalid vault name: must be a simple name without path separators");
    }
    if name.starts_with('_') || name.starts_with('.') {
        anyhow::bail!("Invalid vault name: must not start with '_' or '.'");
    }
    Ok(())
}

/// Count `.md` files recursively under a directory.
/// `follow_links(false)` prevents infinite loops from symlink cycles.
fn count_md_files(dir: &Path) -> usize {
    WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        })
        .count()
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns the vaults root directory: `~/.alcove/vaults/`.
pub fn vaults_root() -> PathBuf {
    alcove_home().join("vaults")
}

/// Create a new empty vault directory at `vaults_root/name`.
pub fn create_vault(name: &str) -> Result<PathBuf> {
    validate_vault_name(name)?;
    let vault_path = vaults_root().join(name);
    if vault_path.exists() {
        anyhow::bail!("Vault '{}' already exists", name);
    }
    fs::create_dir_all(&vault_path)
        .with_context(|| format!("Failed to create vault directory: {}", vault_path.display()))?;
    Ok(vault_path)
}

/// Create a symlink at `vaults_root/name` pointing to `target`.
pub fn link_vault(name: &str, target: &Path) -> Result<PathBuf> {
    validate_vault_name(name)?;
    if !target.exists() || !target.is_dir() {
        anyhow::bail!(
            "Target does not exist or is not a directory: {}",
            target.display()
        );
    }
    // Block symlinks to sensitive system directories
    let canonical = target.canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    let canonical_str = canonical.to_string_lossy();
    let blocked = ["/etc", "/usr", "/sys", "/proc", "/dev", "/sbin",
                   "/boot", "/root", "/run", "/var/run",
                   "/private/etc", "/private/var/run", "/private/var/db"];
    if blocked.iter().any(|b| canonical_str.starts_with(b)) {
        anyhow::bail!(
            "Refusing to link vault to sensitive system directory: {}",
            canonical.display()
        );
    }
    let link_path = vaults_root().join(name);
    if link_path.exists() || link_path.symlink_metadata().is_ok() {
        anyhow::bail!("Vault '{}' already exists", name);
    }
    // Ensure parent directory exists
    fs::create_dir_all(vaults_root())
        .with_context(|| "Failed to create vaults root directory")?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, &link_path)
        .with_context(|| format!("Failed to create symlink: {}", link_path.display()))?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(target, &link_path)
        .with_context(|| format!("Failed to create symlink: {}", link_path.display()))?;
    Ok(link_path)
}

/// List all vaults with metadata.
pub fn list_vaults() -> Result<Vec<VaultInfo>> {
    let root = vaults_root();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut vaults = Vec::new();
    for entry in fs::read_dir(&root).with_context(|| "Failed to read vaults directory")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.starts_with('_') {
            continue;
        }
        let path = entry.path();
        // Use fs::metadata to follow symlinks (DirEntry::metadata may not on all platforms)
        let metadata = fs::metadata(&path)?;
        if !metadata.is_dir() {
            continue;
        }
        let is_link = path.symlink_metadata()?.file_type().is_symlink();
        let doc_count = count_md_files(&path);
        vaults.push(VaultInfo {
            name,
            path,
            is_link,
            doc_count,
        });
    }
    vaults.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(vaults)
}

/// Remove a vault. Symlinks are removed (not their target); real dirs are removed entirely.
pub fn remove_vault(name: &str) -> Result<()> {
    validate_vault_name(name)?;
    let vault_path = vaults_root().join(name);
    let meta = vault_path
        .symlink_metadata()
        .with_context(|| format!("Vault '{}' does not exist", name))?;
    if meta.file_type().is_symlink() {
        fs::remove_file(&vault_path)
            .with_context(|| format!("Failed to remove symlink: {}", vault_path.display()))?;
    } else {
        fs::remove_dir_all(&vault_path)
            .with_context(|| format!("Failed to remove vault directory: {}", vault_path.display()))?;
    }
    Ok(())
}

/// Copy a source file or directory into the vault. Returns the destination path.
///
/// - Files are copied directly into the vault root.
/// - Directories are copied recursively, preserving structure.
pub fn add_to_vault(name: &str, source: &Path) -> Result<PathBuf> {
    validate_vault_name(name)?;
    if !source.exists() {
        anyhow::bail!("Source does not exist: {}", source.display());
    }
    let vault_dir = vaults_root().join(name);
    if !vault_dir.exists() {
        anyhow::bail!("Vault '{}' does not exist", name);
    }
    let filename = source
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Source path has no filename"))?;
    let dest = vault_dir.join(filename);
    if dest.exists() {
        anyhow::bail!("Destination already exists: {}", dest.display());
    }
    if source.is_dir() {
        copy_dir_recursive(source, &dest)?;
    } else {
        fs::copy(source, &dest)
            .with_context(|| format!("Failed to copy file to vault: {}", dest.display()))?;
    }
    Ok(dest)
}

/// Recursively copy a directory tree from `src` to `dst`.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)
        .with_context(|| format!("Failed to create directory: {}", dst.display()))?;
    for entry in fs::read_dir(src)
        .with_context(|| format!("Failed to read directory: {}", src.display()))?
    {
        let entry = entry?;
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dst_path = dst.join(&file_name);
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)
                .with_context(|| format!("Failed to copy: {}", src_path.display()))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Override vaults_root by setting ALCOVE_HOME env var and using a helper
    /// that creates the vaults structure under a temp dir.
    /// Since we can't easily override alcove_home(), we test the internal
    /// functions directly with explicit paths.
    fn setup_vaults_dir(tmp: &TempDir) -> PathBuf {
        let vaults = tmp.path().join("vaults");
        fs::create_dir_all(&vaults).unwrap();
        vaults
    }

    // -- validate_vault_name --

    #[test]
    fn test_validate_vault_name_valid() {
        assert!(validate_vault_name("my-vault").is_ok());
        assert!(validate_vault_name("vault123").is_ok());
    }

    #[test]
    fn test_validate_vault_name_rejects_invalid() {
        // path traversal
        assert!(validate_vault_name("../escape").is_err());
        // nested path
        assert!(validate_vault_name("a/b").is_err());
        // dot
        assert!(validate_vault_name(".").is_err());
        // dot-dot
        assert!(validate_vault_name("..").is_err());
        // underscore prefix
        assert!(validate_vault_name("_hidden").is_err());
        // dot prefix
        assert!(validate_vault_name(".hidden").is_err());
    }

    // -- create_vault --
    // We test the underlying logic by calling create_vault_in helper.

    fn create_vault_in(vaults_dir: &Path, name: &str) -> Result<PathBuf> {
        validate_vault_name(name)?;
        let vault_path = vaults_dir.join(name);
        if vault_path.exists() {
            anyhow::bail!("Vault '{}' already exists", name);
        }
        fs::create_dir_all(&vault_path)?;
        Ok(vault_path)
    }

    #[test]
    fn test_create_vault_creates_directory() {
        let tmp = TempDir::new().unwrap();
        let vaults = setup_vaults_dir(&tmp);
        let path = create_vault_in(&vaults, "testvault").unwrap();
        assert!(path.is_dir());
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "testvault");
    }

    #[test]
    fn test_create_vault_rejects_invalid_names() {
        let tmp = TempDir::new().unwrap();
        let vaults = setup_vaults_dir(&tmp);
        for bad in &["../escape", "a/b", ".", "..", "_hidden", ".hidden"] {
            assert!(
                create_vault_in(&vaults, bad).is_err(),
                "should reject vault name: '{bad}'"
            );
        }
    }

    #[test]
    fn test_create_vault_errors_on_duplicate() {
        let tmp = TempDir::new().unwrap();
        let vaults = setup_vaults_dir(&tmp);
        create_vault_in(&vaults, "dup").unwrap();
        assert!(create_vault_in(&vaults, "dup").is_err());
    }

    // -- link_vault --

    fn link_vault_in(vaults_dir: &Path, name: &str, target: &Path) -> Result<PathBuf> {
        validate_vault_name(name)?;
        if !target.exists() || !target.is_dir() {
            anyhow::bail!(
                "Target does not exist or is not a directory: {}",
                target.display()
            );
        }
        let link_path = vaults_dir.join(name);
        if link_path.exists() || link_path.symlink_metadata().is_ok() {
            anyhow::bail!("Vault '{}' already exists", name);
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, &link_path)?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(target, &link_path)?;
        Ok(link_path)
    }

    #[test]
    fn test_link_vault_creates_symlink() {
        let tmp = TempDir::new().unwrap();
        let vaults = setup_vaults_dir(&tmp);
        let target = tmp.path().join("external");
        fs::create_dir_all(&target).unwrap();

        let link = link_vault_in(&vaults, "linked", &target).unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(link.is_dir());
    }

    #[test]
    fn test_link_vault_errors_if_target_doesnt_exist() {
        let tmp = TempDir::new().unwrap();
        let vaults = setup_vaults_dir(&tmp);
        let nonexistent = tmp.path().join("nope");
        assert!(link_vault_in(&vaults, "bad", &nonexistent).is_err());
    }

    // -- list_vaults --

    fn list_vaults_in(vaults_dir: &Path) -> Result<Vec<VaultInfo>> {
        if !vaults_dir.exists() {
            return Ok(Vec::new());
        }
        let mut vaults = Vec::new();
        for entry in fs::read_dir(vaults_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.starts_with('_') {
                continue;
            }
            let path = entry.path();
            // Use fs::metadata to follow symlinks (DirEntry::metadata may not on all platforms)
            let metadata = fs::metadata(&path)?;
            if !metadata.is_dir() {
                continue;
            }
            let is_link = path.symlink_metadata()?.file_type().is_symlink();
            let doc_count = count_md_files(&path);
            vaults.push(VaultInfo {
                name,
                path,
                is_link,
                doc_count,
            });
        }
        vaults.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(vaults)
    }

    #[test]
    fn test_list_vaults_returns_correct_info() {
        let tmp = TempDir::new().unwrap();
        let vaults = setup_vaults_dir(&tmp);

        // Create a real vault with 2 md files
        let real_vault = vaults.join("docs");
        fs::create_dir_all(&real_vault).unwrap();
        fs::write(real_vault.join("a.md"), "# A").unwrap();
        fs::write(real_vault.join("b.md"), "# B").unwrap();
        fs::write(real_vault.join("c.txt"), "not md").unwrap();

        // Create an external dir with 1 md file and link it
        let ext = tmp.path().join("external");
        fs::create_dir_all(&ext).unwrap();
        fs::write(ext.join("x.md"), "# X").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&ext, vaults.join("linked")).unwrap();

        let list = list_vaults_in(&vaults).unwrap();

        let real = list.iter().find(|v| v.name == "docs").unwrap();
        assert!(!real.is_link);
        assert_eq!(real.doc_count, 2);

        #[cfg(unix)]
        {
            let linked = list.iter().find(|v| v.name == "linked").unwrap();
            assert!(linked.is_link);
            assert_eq!(linked.doc_count, 1);
        }
    }

    // -- remove_vault --

    fn remove_vault_in(vaults_dir: &Path, name: &str) -> Result<()> {
        validate_vault_name(name)?;
        let vault_path = vaults_dir.join(name);
        let meta = vault_path
            .symlink_metadata()
            .with_context(|| format!("Vault '{}' does not exist", name))?;
        if meta.file_type().is_symlink() {
            fs::remove_file(&vault_path)?;
        } else {
            fs::remove_dir_all(&vault_path)?;
        }
        Ok(())
    }

    #[test]
    fn test_remove_vault_symlink_removes_only_link() {
        let tmp = TempDir::new().unwrap();
        let vaults = setup_vaults_dir(&tmp);
        let target = tmp.path().join("external");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("keep.md"), "# Keep").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, vaults.join("linked")).unwrap();
        #[cfg(unix)]
        {
            remove_vault_in(&vaults, "linked").unwrap();
            assert!(!vaults.join("linked").exists());
            // Target should still exist
            assert!(target.join("keep.md").exists());
        }
    }

    #[test]
    fn test_remove_vault_real_dir_removes_directory() {
        let tmp = TempDir::new().unwrap();
        let vaults = setup_vaults_dir(&tmp);
        let vault_path = vaults.join("removeme");
        fs::create_dir_all(&vault_path).unwrap();
        fs::write(vault_path.join("file.md"), "# File").unwrap();

        remove_vault_in(&vaults, "removeme").unwrap();
        assert!(!vault_path.exists());
    }

    // -- add_to_vault --

    fn add_to_vault_in(vaults_dir: &Path, name: &str, source: &Path) -> Result<PathBuf> {
        validate_vault_name(name)?;
        if !source.exists() || !source.is_file() {
            anyhow::bail!(
                "Source does not exist or is not a file: {}",
                source.display()
            );
        }
        let vault_dir = vaults_dir.join(name);
        if !vault_dir.exists() {
            anyhow::bail!("Vault '{}' does not exist", name);
        }
        let filename = source
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("Source path has no filename"))?;
        let dest = vault_dir.join(filename);
        if dest.exists() {
            anyhow::bail!("Destination already exists: {}", dest.display());
        }
        fs::copy(source, &dest)?;
        Ok(dest)
    }

    #[test]
    fn test_add_to_vault_copies_file() {
        let tmp = TempDir::new().unwrap();
        let vaults = setup_vaults_dir(&tmp);
        let vault_path = vaults.join("myvault");
        fs::create_dir_all(&vault_path).unwrap();

        let source = tmp.path().join("note.md");
        fs::write(&source, "# My Note\nContent here.").unwrap();

        let dest = add_to_vault_in(&vaults, "myvault", &source).unwrap();
        assert!(dest.exists());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "# My Note\nContent here.");
        // Source should still exist (it's a copy)
        assert!(source.exists());
    }
}
