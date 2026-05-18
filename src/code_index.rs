//! AST-based code structure indexing using tree-sitter.
//!
//! Parses Rust source files and generates module-level markdown summaries
//! that integrate with the existing Tantivy search pipeline.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use tree_sitter::{Parser, Node};

/// Result of indexing a source directory.
#[derive(serde::Serialize)]
pub struct CodeIndexResult {
    pub modules_indexed: usize,
    pub files_skipped: usize,
}

/// Parsed info from a single Rust source file.
#[allow(dead_code)] // submodules/imports used in Phase 4 (call graph)
struct ModuleInfo {
    module_path: String,
    functions: Vec<String>,
    types: Vec<String>,
    submodules: Vec<String>,
    imports: Vec<String>,
}

// ── Public API ──────────────────────────────────────────────────────

/// Index a source directory and write CODE_INDEX.md into the project's docs.
pub fn index_code_structure(
    docs_root: &Path,
    project: &str,
    source_path: &Path,
) -> Result<CodeIndexResult> {
    let canonical = source_path.canonicalize()
        .with_context(|| format!("Source path does not exist: {}", source_path.display()))?;

    if !canonical.is_dir() {
        anyhow::bail!("Source path is not a directory: {}", source_path.display());
    }

    let (modules, skipped) = parse_source_dir(&canonical)?;
    let markdown = generate_code_index(&modules, &canonical);
    write_code_index(docs_root, project, &markdown)?;

    Ok(CodeIndexResult {
        modules_indexed: modules.len(),
        files_skipped: skipped,
    })
}

/// Parse and return markdown without writing (for testing / preview).
#[allow(dead_code)]
pub fn generate_markdown(source_path: &Path) -> Result<(String, usize)> {
    let (modules, _skipped) = parse_source_dir(source_path)?;
    let count = modules.len();
    Ok((generate_code_index(&modules, source_path), count))
}

// ── Directory Walking ───────────────────────────────────────────────

fn parse_source_dir(source_path: &Path) -> Result<(Vec<ModuleInfo>, usize)> {
    let mut modules = BTreeMap::new();
    let mut skipped = 0usize;
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .context("Failed to set Rust language for tree-sitter")?;

    walk_rust_files(source_path, source_path, &mut parser, &mut modules, &mut skipped)?;

    Ok((modules.into_values().collect(), skipped))
}

fn walk_rust_files(
    base: &Path,
    current: &Path,
    parser: &mut Parser,
    modules: &mut BTreeMap<String, ModuleInfo>,
    skipped: &mut usize,
) -> Result<()> {
    let entries = fs::read_dir(current).context("Failed to read source directory")?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // Skip target/, .git/, hidden dirs
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name == "target" || name == ".git" || name.starts_with('.') {
                continue;
            }
            walk_rust_files(base, &path, parser, modules, skipped)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            let source = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let module_path = file_to_module_path(base, &path);

            if let Some(info) = parse_rust_file(&source, &module_path, parser) {
                modules.insert(module_path, info);
            } else {
                *skipped += 1;
            }
        }
    }

    Ok(())
}

/// Convert a file path to a Rust module path.
/// `src/server.rs` → `server`, `src/index/builder.rs` → `index::builder`,
/// `src/index/mod.rs` → `index`
fn file_to_module_path(base: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(base).unwrap_or(file);
    let s = rel.to_string_lossy();

    // Handle mod.rs → parent dir name
    if s.ends_with("/mod.rs") {
        let parent = rel.parent().unwrap_or(Path::new(""));
        return parent.to_string_lossy().replace('/', "::");
    }

    // Strip .rs extension and convert / to ::
    s.trim_end_matches(".rs").replace('/', "::")
}

// ── AST Parsing ─────────────────────────────────────────────────────

fn parse_rust_file(source: &str, module_path: &str, parser: &mut Parser) -> Option<ModuleInfo> {
    let tree = parser.parse(source, None)?;
    let root = tree.root_node();

    let mut functions = Vec::new();
    let mut types = Vec::new();
    let mut submodules = Vec::new();
    let mut imports = Vec::new();

    for i in 0..root.named_child_count() {
        let child = root.named_child(i as u32).unwrap();
        let kind = child.kind();

        match kind {
            "function_item" => {
                if let Some(sig) = extract_function_signature(&child, source) {
                    functions.push(sig);
                }
            }
            "struct_item" | "enum_item" | "trait_item" | "type_alias" => {
                if let Some(def) = extract_type_definition(&child, source) {
                    types.push(def);
                }
            }
            "impl_item" => {
                // Impl items contain methods — extract public method signatures
                extract_impl_methods(&child, source, &mut functions);
            }
            "mod_item" => {
                if let Some(name) = extract_identifier(&child, source) {
                    submodules.push(name);
                }
            }
            "use_declaration" => {
                let text = node_text(&child, source);
                imports.push(text);
            }
            _ => {}
        }
    }

    Some(ModuleInfo {
        module_path: module_path.to_string(),
        functions,
        types,
        submodules,
        imports,
    })
}

fn extract_function_signature(node: &Node, source: &str) -> Option<String> {
    let is_public = is_public(node);

    // Find the block (body) and take text up to it, or take the full node
    let sig_text = if let Some(block) = node.child_by_field_name("body") {
        let end = block.start_byte();
        source[node.start_byte()..end].trim().to_string()
    } else {
        // function_signature_item (trait) — no body
        node_text(node, source)
    };

    // Only include public functions in the index
    if !is_public {
        return None;
    }

    Some(sig_text)
}

fn extract_type_definition(node: &Node, source: &str) -> Option<String> {
    let is_public = is_public(node);
    if !is_public {
        return None;
    }

    let text = node_text(node, source);

    // For structs/enums with long bodies, produce a compact one-line form
    let compact = match node.kind() {
        "struct_item" => compact_struct(node, source),
        "enum_item" => compact_enum(node, source),
        "trait_item" => compact_trait(node, source),
        "type_alias" => node_text(node, source),
        _ => text,
    };

    Some(compact)
}

fn compact_struct(node: &Node, source: &str) -> String {
    let name = node.child_by_field_name("name").map(|n| node_text(&n, source));
    let name_str = name.unwrap_or_default();

    // field_declaration_list is the body of a struct
    if let Some(body) = find_child_by_kind(node, "field_declaration_list") {
        let fields = collect_struct_fields(&body, source);
        if fields.is_empty() {
            return format!("struct {name_str} {{}}");
        }
        let fields_str = fields.join(", ");
        return format!("struct {name_str} {{ {fields_str} }}");
    }

    // Tuple struct or unit struct
    node_text(node, source)
}

fn collect_struct_fields(body: &Node, source: &str) -> Vec<String> {
    let mut fields = Vec::new();
    for i in 0..body.named_child_count() {
        let child = body.named_child(i as u32).unwrap();
        if child.kind() == "field_declaration" {
            let field_name = find_child_text_by_kind(&child, source, "field_identifier");
            let field_type = child.child_by_field_name("type").map(|n| node_text(&n, source));
            if let (Some(n), Some(t)) = (field_name, field_type) {
                fields.push(format!("{n}: {t}"));
            }
        }
    }
    fields
}

fn compact_enum(node: &Node, source: &str) -> String {
    let name = node.child_by_field_name("name").map(|n| node_text(&n, source));
    let name_str = name.unwrap_or_default();

    if let Some(body) = find_child_by_kind(node, "enum_variant_list") {
        let variants = collect_enum_variants(&body, source);
        if variants.is_empty() {
            return format!("enum {name_str} {{}}");
        }
        let variants_str = variants.join(", ");
        return format!("enum {name_str} {{ {variants_str} }}");
    }

    node_text(node, source)
}

fn collect_enum_variants(body: &Node, source: &str) -> Vec<String> {
    let mut variants = Vec::new();
    for i in 0..body.named_child_count() {
        let child = body.named_child(i as u32).unwrap();
        if child.kind() == "enum_variant" {
            let vname = child.child_by_field_name("name").map(|n| node_text(&n, source));
            if let Some(name) = vname {
                variants.push(name);
            }
        }
    }
    variants
}

fn compact_trait(node: &Node, source: &str) -> String {
    let name = node.child_by_field_name("name").map(|n| node_text(&n, source));
    let name_str = name.unwrap_or_default();

    // Check for trait bounds
    let bounds = node.children_by_field_name("bounds", &mut node.walk())
        .filter_map(|n| {
            let text = node_text(&n, source);
            if text.is_empty() { None } else { Some(text) }
        })
        .collect::<Vec<_>>()
        .join(": ");

    if bounds.is_empty() {
        format!("trait {name_str} {{ ... }}")
    } else {
        format!("trait {name_str}: {bounds} {{ ... }}")
    }
}

fn extract_impl_methods(node: &Node, source: &str, functions: &mut Vec<String>) {
    let body = match node.child_by_field_name("body") {
        Some(b) => b,
        None => return,
    };

    for i in 0..body.named_child_count() {
        let child = body.named_child(i as u32).unwrap();
        if child.kind() == "function_item"
            && let Some(sig) = extract_function_signature(&child, source)
        {
            functions.push(sig);
        }
    }
}

fn extract_identifier(node: &Node, source: &str) -> Option<String> {
    for i in 0..node.child_count() {
        let child = node.child(i as u32)?;
        if child.kind() == "identifier" {
            return Some(node_text(&child, source));
        }
    }
    None
}

// ── Helpers ─────────────────────────────────────────────────────────

fn node_text(node: &Node, source: &str) -> String {
    source[node.byte_range()].to_string()
}

fn find_child_by_kind<'a>(node: &Node<'a>, kind: &str) -> Option<Node<'a>> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i as u32)
            && child.kind() == kind
        {
            return Some(child);
        }
    }
    None
}

fn find_child_text_by_kind(node: &Node, source: &str, kind: &str) -> Option<String> {
    find_child_by_kind(node, kind).map(|n| node_text(&n, source))
}

fn is_public(node: &Node) -> bool {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i as u32)
            && child.kind() == "visibility_modifier"
        {
            return true;
        }
    }
    false
}

// ── Markdown Generation ─────────────────────────────────────────────

fn generate_code_index(modules: &[ModuleInfo], source_path: &Path) -> String {
    let mut out = String::new();

    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let rev = get_git_rev(source_path).unwrap_or_else(|| "unknown".into());

    out.push_str("# Code Structure Index\n\n");
    out.push_str("> Auto-generated by alcove. Do not edit manually.\n");
    out.push_str(&format!("> Last updated: {timestamp}\n"));
    out.push_str(&format!("> Source: {} (rev: {rev})\n\n", source_path.display()));

    for module in modules {
        if module.functions.is_empty() && module.types.is_empty() {
            continue;
        }

        out.push_str(&format!("## Module: {}\n\n", module.module_path));

        if !module.types.is_empty() {
            out.push_str("### Types\n");
            for t in &module.types {
                out.push_str(&format!("- `{t}`\n"));
            }
            out.push('\n');
        }

        if !module.functions.is_empty() {
            out.push_str("### Functions\n");
            for f in &module.functions {
                out.push_str(&format!("- `{f}`\n"));
            }
            out.push('\n');
        }
    }

    out
}

fn get_git_rev(source_path: &Path) -> Option<String> {
    use std::process::Command;
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(source_path)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn write_code_index(docs_root: &Path, project: &str, content: &str) -> Result<()> {
    let project_dir = docs_root.join(project);
    fs::create_dir_all(&project_dir)
        .context("Failed to create project docs directory")?;

    let index_path = project_dir.join("CODE_INDEX.md");
    fs::write(&index_path, content)
        .with_context(|| format!("Failed to write {}", index_path.display()))?;

    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_to_module_path() {
        let base = Path::new("/project/src");
        assert_eq!(
            file_to_module_path(base, Path::new("/project/src/server.rs")),
            "server"
        );
        assert_eq!(
            file_to_module_path(base, Path::new("/project/src/index/builder.rs")),
            "index::builder"
        );
        assert_eq!(
            file_to_module_path(base, Path::new("/project/src/index/mod.rs")),
            "index"
        );
    }

    #[test]
    fn test_parse_rust_file_extracts_public_fn() {
        let source = r#"
pub fn register_tools(registry: &mut ToolRegistry) -> Result<()> {
    registry.add("search", handle_search);
    Ok(())
}

fn internal_helper(x: i32) -> i32 {
    x + 1
}

pub struct ToolRegistry {
    tools: HashMap<String, String>,
}

enum PrivateEnum { A, B }
"#;

        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();

        let result = parse_rust_file(source, "test_module", &mut parser);
        assert!(result.is_some());
        let info = result.unwrap();

        assert_eq!(info.functions.len(), 1);
        assert!(info.functions[0].contains("pub fn register_tools"));
        assert!(!info.functions[0].contains("registry.add"));

        assert_eq!(info.types.len(), 1);
        assert!(info.types[0].contains("struct ToolRegistry"));
        assert!(info.types[0].contains("HashMap"));
    }

    #[test]
    fn test_parse_rust_file_enum() {
        let source = r#"
pub enum ProjectError {
    NotFound,
    Ambiguous(Vec<String>),
}
"#;

        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();

        let info = parse_rust_file(source, "test", &mut parser).unwrap();
        assert_eq!(info.types.len(), 1);
        assert!(info.types[0].contains("enum ProjectError"));
        assert!(info.types[0].contains("NotFound"));
        assert!(info.types[0].contains("Ambiguous"));
    }

    #[test]
    fn test_generate_markdown_format() {
        let source = r#"
use std::collections::HashMap;

pub fn search(query: &str) -> Vec<Result> {
    vec![]
}

pub struct SearchIndex {
    entries: HashMap<String, String>,
}
"#;

        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();

        let info = parse_rust_file(source, "search", &mut parser).unwrap();
        let modules = vec![info];
        let md = generate_code_index(&modules, Path::new("/test"));

        assert!(md.contains("## Module: search"));
        assert!(md.contains("### Functions"));
        assert!(md.contains("### Types"));
        assert!(md.contains("`pub fn search"));
        assert!(md.contains("`struct SearchIndex"));
    }
}
