//! AST-based code structure indexing using tree-sitter.
//!
//! Parses source files and generates module-level markdown summaries
//! that integrate with the existing Tantivy search pipeline.
//!
//! Supports multiple programming languages via feature-gated parsers.
//! See `languages/` subdirectory for language-specific implementations.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};

pub mod languages;
pub mod markdown;
pub mod registry;
pub mod walker;

// ── Public types ─────────────────────────────────────────────────────

/// Result of indexing a source directory.
#[derive(serde::Serialize)]
pub struct CodeIndexResult {
    pub modules_indexed: usize,
    pub files_skipped: usize,
    pub languages_detected: Vec<String>,
}

/// Parsed info from a single source file.
pub struct ModuleInfo {
    pub module_path: String,
    pub language: String,
    pub functions: Vec<String>,
    pub types: Vec<String>,
    #[allow(dead_code)] // submodules used in Phase 4 (call graph)
    pub submodules: Vec<String>,
    #[allow(dead_code)] // imports used in Phase 4 (call graph)
    pub imports: Vec<String>,
}

// ── Public API ───────────────────────────────────────────────────────

/// Index a source directory and write CODE_INDEX.md into the project's docs.
/// Auto-detects the language from project configuration.
#[allow(dead_code)] // Public API — used via lib re-export and externally
pub fn index_code_structure(
    docs_root: &Path,
    project: &str,
    source_path: &Path,
) -> Result<CodeIndexResult> {
    index_code_structure_with_lang(docs_root, project, source_path, None)
}

/// Index with an optional explicit language filter.
pub fn index_code_structure_with_lang(
    docs_root: &Path,
    project: &str,
    source_path: &Path,
    language: Option<&str>,
) -> Result<CodeIndexResult> {
    let canonical = source_path
        .canonicalize()
        .with_context(|| format!("Source path does not exist: {}", source_path.display()))?;

    if !canonical.is_dir() {
        anyhow::bail!("Source path is not a directory: {}", source_path.display());
    }

    let reg = registry::default_registry();
    if reg.is_empty() {
        anyhow::bail!(
            "No language parsers enabled. Compile with at least one lang-* feature (e.g. lang-rust)."
        );
    }

    // Language filter: explicit → filter to that language only
    // No explicit language → index ALL recognized files by extension (monorepo-safe)
    let lang_filter = language;

    let mut modules = BTreeMap::new();
    let mut skipped = 0usize;
    walker::walk_source_dir(
        &canonical,
        &canonical,
        &reg,
        &mut modules,
        &mut skipped,
        lang_filter,
    )?;

    let languages_detected: Vec<String> = {
        let mut langs: Vec<String> = modules
            .values()
            .map(|m| m.language.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        langs.sort();
        langs
    };

    let module_list: Vec<ModuleInfo> = modules.into_values().collect();
    let md = markdown::generate_code_index(&module_list, &canonical);
    markdown::write_code_index(docs_root, project, &md)?;

    Ok(CodeIndexResult {
        modules_indexed: module_list.len(),
        files_skipped: skipped,
        languages_detected,
    })
}

/// Parse and return markdown without writing (for testing / preview).
#[allow(dead_code)] // Public API — used via lib re-export
pub fn generate_markdown(source_path: &Path) -> Result<(String, usize)> {
    generate_markdown_with_lang(source_path, None)
}

/// Parse and return markdown with an optional language filter.
#[allow(dead_code)] // Public API — used via lib re-export
pub fn generate_markdown_with_lang(
    source_path: &Path,
    language: Option<&str>,
) -> Result<(String, usize)> {
    let reg = registry::default_registry();
    let lang_filter = language;

    let mut modules = BTreeMap::new();
    let mut skipped = 0usize;
    walker::walk_source_dir(
        source_path,
        source_path,
        &reg,
        &mut modules,
        &mut skipped,
        lang_filter,
    )?;

    let count = modules.len();
    let module_list: Vec<ModuleInfo> = modules.into_values().collect();
    Ok((markdown::generate_code_index(&module_list, source_path), count))
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "lang-rust")]
    fn test_generate_markdown_rust() {
        // Use this crate's own src directory
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let (md, count) = generate_markdown(&source).unwrap();
        assert!(count > 0);
        assert!(md.contains("# Code Structure Index"));
        assert!(md.contains("Languages: Rust"));
    }
}
