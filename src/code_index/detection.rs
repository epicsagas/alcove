//! Automatic language detection from project structure.

use std::collections::HashMap;
use std::path::Path;

use super::registry::LanguageRegistry;

/// Detect the primary language of a source directory.
///
/// Uses a three-tier fallback strategy:
/// 1. Project config files (Cargo.toml, tsconfig.json, etc.)
/// 2. File extension census (count extensions in top 2 levels)
/// 3. Fallback: return None, let the walker index all recognized files
pub fn detect_project_language(source_path: &Path, registry: &LanguageRegistry) -> Option<String> {
    detect_by_config_file(source_path, registry)
        .or_else(|| detect_by_extension_census(source_path, registry))
}

/// Tier 1: detect language from project configuration files.
fn detect_by_config_file(source_path: &Path, registry: &LanguageRegistry) -> Option<String> {
    let config_map: &[(&str, &str)] = &[
        ("Cargo.toml", "Rust"),
        ("pyproject.toml", "Python"),
        ("setup.py", "Python"),
        ("requirements.txt", "Python"),
        ("tsconfig.json", "TypeScript"),
        ("go.mod", "Go"),
        ("go.sum", "Go"),
        ("pom.xml", "Java"),
        ("build.gradle", "Java"),
        ("build.gradle.kts", "Kotlin"),
        ("settings.gradle.kts", "Kotlin"),
        ("Package.swift", "Swift"),
        ("Gemfile", "Ruby"),
        ("composer.json", "PHP"),
        ("mix.exs", "Elixir"),
        ("Cargo.lock", "Rust"), // secondary indicator
    ];

    for (filename, lang) in config_map {
        if source_path.join(filename).is_file() || source_path.join(format!("src/{filename}")).is_file() {
            // Only return if we have a parser for this language
            if registry.parser_for_name(lang).is_some() {
                return Some(lang.to_string());
            }
        }
    }

    // Check for .csproj / .sln files (C#)
    if let Ok(entries) = std::fs::read_dir(source_path) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if (name_str.ends_with(".csproj") || name_str.ends_with(".sln"))
                && registry.parser_for_name("C#").is_some()
            {
                return Some("C#".to_string());
            }
            if name_str == "CMakeLists.txt"
                && (registry.parser_for_name("C++").is_some()
                    || registry.parser_for_name("C").is_some())
            {
                // CMakeLists.txt could be C or C++ — prefer C++ if available
                if registry.parser_for_name("C++").is_some() {
                    return Some("C++".to_string());
                }
                return Some("C".to_string());
            }
            if name_str == "Makefile"
                && registry.parser_for_name("C").is_some()
                && registry.parser_for_name("C++").is_none()
            {
                return Some("C".to_string());
            }
        }
    }

    None
}

/// Tier 2: detect language by counting file extensions in top 2 directory levels.
fn detect_by_extension_census(source_path: &Path, registry: &LanguageRegistry) -> Option<String> {
    let mut lang_counts: HashMap<String, usize> = HashMap::new();

    count_extensions(source_path, &mut lang_counts, registry, 0);

    lang_counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .and_then(|(lang, count)| {
            if count > 0 {
                Some(lang)
            } else {
                None
            }
        })
}

fn count_extensions(
    dir: &Path,
    counts: &mut HashMap<String, usize>,
    registry: &LanguageRegistry,
    depth: usize,
) {
    if depth > 2 {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            count_extensions(&path, counts, registry, depth + 1);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str())
            && let Some(parser) = registry.parser_for_extension(ext)
        {
            *counts.entry(parser.language_name().to_string()).or_insert(0) += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use tempfile::TempDir;

    fn setup_project(files: &[(&str, &str)]) -> TempDir {
        let tmp = TempDir::new().unwrap();
        for (path, content) in files {
            let full = tmp.path().join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(full, content).unwrap();
        }
        tmp
    }

    #[test]
    #[cfg(feature = "lang-rust")]
    fn test_detect_rust_by_cargo_toml() {
        let tmp = setup_project(&[("Cargo.toml", "[package]\nname = \"test\"")]);
        let reg = super::super::registry::default_registry();
        let result = detect_project_language(tmp.path(), &reg);
        assert_eq!(result.as_deref(), Some("Rust"));
    }

    #[test]
    fn test_detect_nothing_for_empty() {
        let tmp = TempDir::new().unwrap();
        let reg = super::super::registry::default_registry();
        let result = detect_project_language(tmp.path(), &reg);
        assert!(result.is_none());
    }
}
