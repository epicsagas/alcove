//! Rust language parser for code structure indexing.

use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::code_index::ModuleInfo;
use crate::code_index::languages::LanguageParser;
use crate::code_index::languages::{
    child_text_by_field, find_child_by_kind, find_child_text_by_kind, has_child_kind, node_text,
};

// ── Parser struct ────────────────────────────────────────────────────

pub struct RustParser;

impl LanguageParser for RustParser {
    fn language_name(&self) -> &str {
        "Rust"
    }

    fn file_extensions(&self) -> &[&str] {
        &["rs"]
    }

    fn module_path_for_file(&self, base: &Path, file: &Path) -> String {
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

    fn parse_file(
        &self,
        source: &str,
        module_path: &str,
        parser: &mut Parser,
    ) -> Option<ModuleInfo> {
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .ok()?;
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
            language: "Rust".to_string(),
            functions,
            types,
            submodules,
            imports,
        })
    }
}

// ── Extraction helpers ───────────────────────────────────────────────

fn extract_function_signature(node: &Node, source: &str) -> Option<String> {
    let is_public = has_child_kind(node, "visibility_modifier");

    let sig_text = if let Some(block) = node.child_by_field_name("body") {
        let end = block.start_byte();
        source[node.start_byte()..end].trim().to_string()
    } else {
        node_text(node, source)
    };

    if !is_public {
        return None;
    }

    Some(sig_text)
}

fn extract_type_definition(node: &Node, source: &str) -> Option<String> {
    let is_public = has_child_kind(node, "visibility_modifier");
    if !is_public {
        return None;
    }

    let compact = match node.kind() {
        "struct_item" => compact_struct(node, source),
        "enum_item" => compact_enum(node, source),
        "trait_item" => compact_trait(node, source),
        "type_alias" => node_text(node, source),
        _ => node_text(node, source),
    };

    Some(compact)
}

fn compact_struct(node: &Node, source: &str) -> String {
    let name = child_text_by_field(node, source, "name");
    let name_str = name.unwrap_or_default();

    if let Some(body) = find_child_by_kind(node, "field_declaration_list") {
        let fields = collect_struct_fields(&body, source);
        if fields.is_empty() {
            return format!("struct {name_str} {{}}");
        }
        let fields_str = fields.join(", ");
        return format!("struct {name_str} {{ {fields_str} }}");
    }

    node_text(node, source)
}

fn collect_struct_fields(body: &Node, source: &str) -> Vec<String> {
    let mut fields = Vec::new();
    for i in 0..body.named_child_count() {
        let child = body.named_child(i as u32).unwrap();
        if child.kind() == "field_declaration" {
            let field_name = find_child_text_by_kind(&child, source, "field_identifier");
            let field_type = child_text_by_field(&child, source, "type");
            if let (Some(n), Some(t)) = (field_name, field_type) {
                fields.push(format!("{n}: {t}"));
            }
        }
    }
    fields
}

fn compact_enum(node: &Node, source: &str) -> String {
    let name = child_text_by_field(node, source, "name");
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
            let vname = child_text_by_field(&child, source, "name");
            if let Some(name) = vname {
                variants.push(name);
            }
        }
    }
    variants
}

fn compact_trait(node: &Node, source: &str) -> String {
    let name = child_text_by_field(node, source, "name");
    let name_str = name.unwrap_or_default();

    let bounds = node
        .children_by_field_name("bounds", &mut node.walk())
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

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn parse(source: &str, module_path: &str) -> Option<ModuleInfo> {
        let mut parser = Parser::new();
        RustParser.parse_file(source, module_path, &mut parser)
    }

    #[test]
    fn test_module_path() {
        let base = Path::new("/project/src");
        assert_eq!(
            RustParser.module_path_for_file(base, Path::new("/project/src/server.rs")),
            "server"
        );
        assert_eq!(
            RustParser.module_path_for_file(base, Path::new("/project/src/index/builder.rs")),
            "index::builder"
        );
        assert_eq!(
            RustParser.module_path_for_file(base, Path::new("/project/src/index/mod.rs")),
            "index"
        );
    }

    #[test]
    fn test_extracts_public_fn() {
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

        let info = parse(source, "test_module").unwrap();
        assert_eq!(info.functions.len(), 1);
        assert!(info.functions[0].contains("pub fn register_tools"));
        assert!(!info.functions[0].contains("registry.add"));

        assert_eq!(info.types.len(), 1);
        assert!(info.types[0].contains("struct ToolRegistry"));
        assert!(info.types[0].contains("HashMap"));
    }

    #[test]
    fn test_extracts_enum() {
        let source = r#"
pub enum ProjectError {
    NotFound,
    Ambiguous(Vec<String>),
}
"#;

        let info = parse(source, "test").unwrap();
        assert_eq!(info.types.len(), 1);
        assert!(info.types[0].contains("enum ProjectError"));
        assert!(info.types[0].contains("NotFound"));
        assert!(info.types[0].contains("Ambiguous"));
    }

    #[test]
    fn test_language_name_and_extensions() {
        assert_eq!(RustParser.language_name(), "Rust");
        assert_eq!(RustParser.file_extensions(), &["rs"]);
    }
}
