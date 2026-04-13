use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub struct PromoteOptions {
    pub source: PathBuf,
    pub project: Option<String>,
    pub copy: bool,
}

pub struct PromoteResult {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub project: String,
    pub action: &'static str,
}

// ---------------------------------------------------------------------------
// Auto-detect project
// ---------------------------------------------------------------------------

/// Return all project directory names directly under docs_root (non-hidden, non-underscore).
fn list_projects(docs_root: &Path) -> Vec<String> {
    fs::read_dir(docs_root)
        .ok()
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') || name.starts_with('_') {
                        None
                    } else {
                        Some(name)
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Score how well the source file matches a project name.
/// Returns a score ≥ 1 if there is any match, 0 for no match.
fn score_match(source_path: &Path, content: &str, project_name: &str) -> usize {
    let name_lower = project_name.to_lowercase();
    let mut score = 0usize;

    // File name contains project name
    if let Some(stem) = source_path.file_stem().and_then(|s| s.to_str()) {
        if stem.to_lowercase().contains(&name_lower) {
            score += 3;
        }
    }

    // Parent directory names in the source path contain project name
    for component in source_path.components() {
        let comp_str = component.as_os_str().to_string_lossy().to_lowercase();
        if comp_str.contains(&name_lower) {
            score += 2;
        }
    }

    // Content contains project name (case-insensitive)
    let content_lower = content.to_lowercase();
    let occurrences = content_lower.matches(&name_lower).count();
    score += occurrences.min(5); // cap at 5 to avoid swamping

    score
}

/// Determine target project for the given source file.
fn detect_project(docs_root: &Path, source: &Path, content: &str) -> String {
    let projects = list_projects(docs_root);

    let best = projects
        .iter()
        .map(|p| (p, score_match(source, content, p)))
        .filter(|(_, s)| *s > 0)
        .max_by_key(|(_, s)| *s);

    best.map(|(p, _)| p.clone())
        .unwrap_or_else(|| "inbox".to_string())
}

// ---------------------------------------------------------------------------
// Core promote function
// ---------------------------------------------------------------------------

pub fn promote(docs_root: &Path, opts: PromoteOptions) -> Result<PromoteResult> {
    let source = &opts.source;

    if !source.exists() {
        anyhow::bail!("Source file does not exist: {}", source.display());
    }

    let content = fs::read_to_string(source)
        .with_context(|| format!("Failed to read source file: {}", source.display()))?;

    let project = opts
        .project
        .clone()
        .unwrap_or_else(|| detect_project(docs_root, source, &content));

    // Prevent path traversal: project name must not contain separators or `..`
    if project.contains(std::path::MAIN_SEPARATOR)
        || project.contains('/')
        || project.contains("..")
        || project == "."
    {
        anyhow::bail!(
            "Invalid project name '{}': must not contain path separators or traversal sequences",
            project
        );
    }

    let target_dir = docs_root.join(&project);

    // Create target directory if needed (inbox may not exist yet)
    if !target_dir.exists() {
        fs::create_dir_all(&target_dir)
            .with_context(|| format!("Failed to create target directory: {}", target_dir.display()))?;
    }

    let filename = source
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Source path has no filename"))?;

    let destination = target_dir.join(filename);

    // Avoid overwriting unless source == destination
    if destination.exists() && destination.canonicalize()? != source.canonicalize()? {
        anyhow::bail!(
            "Destination already exists: {}. Remove it first or rename the source.",
            destination.display()
        );
    }

    if opts.copy {
        fs::copy(source, &destination).with_context(|| {
            format!(
                "Failed to copy {} → {}",
                source.display(),
                destination.display()
            )
        })?;
    } else {
        fs::rename(source, &destination).or_else(|_| -> anyhow::Result<()> {
            // rename fails across filesystems; fall back to copy+delete
            fs::copy(source, &destination)?;
            if let Err(e) = fs::remove_file(source) {
                // Undo the copy so the filesystem stays consistent
                let _ = fs::remove_file(&destination);
                return Err(anyhow::anyhow!(
                    "Failed to remove source file after copy: {}: {}",
                    source.display(),
                    e
                ));
            }
            Ok(())
        })?;
    }

    Ok(PromoteResult {
        source: source.clone(),
        destination,
        project,
        action: if opts.copy { "copied" } else { "moved" },
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, content: &str) -> PathBuf {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_promote_copy_explicit_project() {
        let tmp = TempDir::new().unwrap();
        let docs_root = tmp.path().join("docs");
        fs::create_dir_all(docs_root.join("myproject")).unwrap();

        let vault = tmp.path().join("vault");
        fs::create_dir_all(&vault).unwrap();
        let src = write(&vault, "note.md", "# Note\nContent about stuff.\n");

        let result = promote(
            &docs_root,
            PromoteOptions {
                source: src.clone(),
                project: Some("myproject".into()),
                copy: true,
            },
        )
        .unwrap();

        assert_eq!(result.project, "myproject");
        assert_eq!(result.action, "copied");
        assert!(result.destination.exists());
        assert!(src.exists(), "source should still exist after copy");
    }

    #[test]
    fn test_promote_move() {
        let tmp = TempDir::new().unwrap();
        let docs_root = tmp.path().join("docs");
        fs::create_dir_all(docs_root.join("myproject")).unwrap();

        let vault = tmp.path().join("vault");
        fs::create_dir_all(&vault).unwrap();
        let src = write(&vault, "note.md", "# Note\n");

        let result = promote(
            &docs_root,
            PromoteOptions {
                source: src.clone(),
                project: Some("myproject".into()),
                copy: false,
            },
        )
        .unwrap();

        assert_eq!(result.action, "moved");
        assert!(result.destination.exists());
        assert!(!src.exists(), "source should be gone after move");
    }

    #[test]
    fn test_promote_auto_detect_by_filename() {
        let tmp = TempDir::new().unwrap();
        let docs_root = tmp.path().join("docs");
        fs::create_dir_all(docs_root.join("alcove")).unwrap();
        fs::create_dir_all(docs_root.join("other")).unwrap();

        let vault = tmp.path().join("vault");
        fs::create_dir_all(&vault).unwrap();
        let src = write(&vault, "alcove-notes.md", "# Notes\n");

        let result = promote(
            &docs_root,
            PromoteOptions {
                source: src.clone(),
                project: None,
                copy: true,
            },
        )
        .unwrap();

        assert_eq!(result.project, "alcove");
    }

    #[test]
    fn test_promote_auto_detect_falls_back_to_inbox() {
        let tmp = TempDir::new().unwrap();
        let docs_root = tmp.path().join("docs");
        fs::create_dir_all(docs_root.join("someproject")).unwrap();

        let vault = tmp.path().join("vault");
        fs::create_dir_all(&vault).unwrap();
        let src = write(&vault, "random-thing.md", "# Random\nNo match.\n");

        let result = promote(
            &docs_root,
            PromoteOptions {
                source: src.clone(),
                project: None,
                copy: true,
            },
        )
        .unwrap();

        assert_eq!(result.project, "inbox");
        assert!(docs_root.join("inbox").is_dir());
    }

    #[test]
    fn test_promote_nonexistent_source_errors() {
        let tmp = TempDir::new().unwrap();
        let docs_root = tmp.path().join("docs");
        fs::create_dir_all(&docs_root).unwrap();

        let result = promote(
            &docs_root,
            PromoteOptions {
                source: PathBuf::from("/nonexistent/file.md"),
                project: Some("proj".into()),
                copy: true,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_promote_path_traversal_rejected() {
        let tmp = TempDir::new().unwrap();
        let docs_root = tmp.path().join("docs");
        fs::create_dir_all(&docs_root).unwrap();

        let vault = tmp.path().join("vault");
        fs::create_dir_all(&vault).unwrap();
        let src = write(&vault, "note.md", "# Note\n");

        for bad_project in &["../outside", "../../etc", "a/b", ".", ".."] {
            let result = promote(
                &docs_root,
                PromoteOptions {
                    source: src.clone(),
                    project: Some(bad_project.to_string()),
                    copy: true,
                },
            );
            assert!(
                result.is_err(),
                "should reject traversal project name: '{}'",
                bad_project
            );
        }
    }

    #[test]
    fn test_promote_destination_exists_errors() {
        let tmp = TempDir::new().unwrap();
        let docs_root = tmp.path().join("docs");
        fs::create_dir_all(docs_root.join("proj")).unwrap();

        let vault = tmp.path().join("vault");
        fs::create_dir_all(&vault).unwrap();
        let src = write(&vault, "note.md", "# Note\n");

        // Pre-create destination
        write(&docs_root, "proj/note.md", "# Existing\n");

        let result = promote(
            &docs_root,
            PromoteOptions {
                source: src.clone(),
                project: Some("proj".into()),
                copy: true,
            },
        );
        assert!(result.is_err(), "should error when destination already exists");
    }
}
