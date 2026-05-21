//! Generic source directory walker — language-agnostic file traversal.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use tree_sitter::Parser;

use super::languages::LanguageParser;
use super::registry::LanguageRegistry;
use crate::code_index::ModuleInfo;

/// Walk a source directory and parse all recognized files.
pub fn walk_source_dir(
    base: &Path,
    current: &Path,
    registry: &LanguageRegistry,
    modules: &mut BTreeMap<String, ModuleInfo>,
    skipped: &mut usize,
    language_filter: Option<&str>,
) -> Result<()> {
    let entries = fs::read_dir(current).context("Failed to read source directory")?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if should_skip_dir(&name) {
                continue;
            }
            walk_source_dir(base, &path, registry, modules, skipped, language_filter)?;
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let parser: &dyn LanguageParser = if let Some(lang) = language_filter {
                match registry.parser_for_name(lang) {
                    Some(p) => p,
                    None => continue,
                }
            } else {
                match registry.parser_for_extension(ext) {
                    Some(p) => p,
                    None => continue,
                }
            };

            // Skip files whose extension doesn't match the parser
            if !parser.file_extensions().contains(&ext) {
                continue;
            }

            let source = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let module_path = parser.module_path_for_file(base, &path);

            let mut ts_parser = Parser::new();
            if let Some(info) = parser.parse_file(&source, &module_path, &mut ts_parser) {
                modules.insert(module_path, info);
            } else {
                *skipped += 1;
            }
        }
    }

    Ok(())
}

/// Directories to skip during traversal.
fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        "target"
            | "node_modules"
            | ".git"
            | "vendor"
            | "__pycache__"
            | "build"
            | "dist"
            | ".build"
            | "Pods"
            | ".venv"
            | "venv"
            | ".next"
            | ".nuxt"
            | "coverage"
            | ".dart_tool"
            | "bazel-out"
            | ".gradle"
    ) || name.starts_with('.')
}
