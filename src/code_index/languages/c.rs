//! C language parser for code structure indexing.

use std::path::Path;

use tree_sitter::Parser;

use crate::code_index::ModuleInfo;
use crate::code_index::languages::{
    LanguageParser, child_text_by_field, find_child_by_kind, node_text,
};

pub struct CParser;

impl LanguageParser for CParser {
    fn language_name(&self) -> &str {
        "C"
    }

    fn file_extensions(&self) -> &[&str] {
        &["c", "h"]
    }

    fn module_path_for_file(&self, base: &Path, file: &Path) -> String {
        let rel = file.strip_prefix(base).unwrap_or(file);
        rel.to_string_lossy().to_string()
    }

    fn parse_file(
        &self,
        source: &str,
        module_path: &str,
        parser: &mut Parser,
    ) -> Option<ModuleInfo> {
        parser
            .set_language(&tree_sitter_c::LANGUAGE.into())
            .ok()?;
        let tree = parser.parse(source, None)?;
        let root = tree.root_node();

        let mut functions = Vec::new();
        let mut types = Vec::new();
        let submodules = Vec::new();
        let mut imports = Vec::new();

        for i in 0..root.named_child_count() {
            let child = root.named_child(i as u32).unwrap();

            match child.kind() {
                "function_definition" => {
                    if !is_static(&child, source)
                        && let Some(sig) = extract_function_sig(&child, source) {
                            functions.push(sig);
                        }
                }
                "struct_specifier" => {
                    if let Some(def) = extract_struct_def(&child, source) {
                        types.push(def);
                    }
                }
                "enum_specifier" => {
                    if let Some(def) = extract_enum_def(&child, source) {
                        types.push(def);
                    }
                }
                "union_specifier" => {
                    if let Some(name) = child_text_by_field(&child, source, "name") {
                        types.push(format!("union {name}"));
                    }
                }
                "type_definition" => {
                    extract_typedef(&child, source, &mut types);
                }
                "preproc_include" => {
                    imports.push(node_text(&child, source));
                }
                _ => {}
            }
        }

        Some(ModuleInfo {
            module_path: module_path.to_string(),
            language: "C".to_string(),
            functions,
            types,
            submodules,
            imports,
        })
    }
}

fn is_static(node: &tree_sitter::Node, source: &str) -> bool {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i as u32) {
            let kind = child.kind();
            if kind == "storage_class_specifier" {
                return node_text(&child, source) == "static";
            }
            if kind == "identifier"
                || kind == "pointer_declarator"
                || kind == "function_declarator"
            {
                break;
            }
        }
    }
    false
}

fn extract_function_sig(node: &tree_sitter::Node, source: &str) -> Option<String> {
    if is_static(node, source) {
        return None;
    }

    // Get the full declaration (up to the body)
    if let Some(body) = node.child_by_field_name("body") {
        let sig = source[node.start_byte()..body.start_byte()].trim().to_string();
        return Some(sig);
    }

    Some(node_text(node, source))
}

fn extract_struct_def(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    let mut sig = format!("struct {name}");

    if let Some(body) = find_child_by_kind(node, "field_declaration_list") {
        let fields = collect_struct_fields(&body, source);
        if !fields.is_empty() {
            sig.push_str(&format!(" {{ {} }}", fields.join(", ")));
        }
    }

    Some(sig)
}

fn collect_struct_fields(body: &tree_sitter::Node, source: &str) -> Vec<String> {
    let mut fields = Vec::new();
    for i in 0..body.named_child_count() {
        let child = body.named_child(i as u32).unwrap();
        if child.kind() == "field_declaration" {
            let text = node_text(&child, source).trim().to_string();
            fields.push(text);
        }
    }
    fields
}

fn extract_enum_def(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;

    if let Some(body) = find_child_by_kind(node, "enumerator_list") {
        let variants = collect_enum_variants(&body, source);
        if variants.is_empty() {
            return Some(format!("enum {name} {{}}"));
        }
        return Some(format!("enum {name} {{ {} }}", variants.join(", ")));
    }

    Some(format!("enum {name}"))
}

fn collect_enum_variants(body: &tree_sitter::Node, source: &str) -> Vec<String> {
    let mut variants = Vec::new();
    for i in 0..body.named_child_count() {
        let child = body.named_child(i as u32).unwrap();
        if child.kind() == "enumerator"
            && let Some(name) = child_text_by_field(&child, source, "name") {
                variants.push(name);
            }
    }
    variants
}

fn extract_typedef(node: &tree_sitter::Node, source: &str, types: &mut Vec<String>) {
    let text = node_text(node, source);
    types.push(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str, module_path: &str) -> Option<ModuleInfo> {
        let mut parser = Parser::new();
        CParser.parse_file(source, module_path, &mut parser)
    }

    #[test]
    fn test_module_path() {
        let base = Path::new("/project/src");
        assert_eq!(
            CParser.module_path_for_file(base, Path::new("/project/src/main.c")),
            "main.c"
        );
    }

    #[test]
    fn test_extracts_public_functions() {
        let source = r#"
#include <stdio.h>

int add(int a, int b) {
    return a + b;
}

static void helper(void) {
    printf("help\n");
}

void process_data(char *input) {
    // ...
}
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.functions.iter().any(|f| f.contains("add")));
        assert!(info.functions.iter().any(|f| f.contains("process_data")));
        assert!(!info.functions.iter().any(|f| f.contains("helper")));
    }

    #[test]
    fn test_extracts_structs_and_enums() {
        let source = r#"
struct Point {
    int x;
    int y;
};

enum Color {
    RED,
    GREEN,
    BLUE
};

typedef int (*Comparator)(const void *, const void *);
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.types.iter().any(|t| t.contains("struct Point")));
        assert!(info.types.iter().any(|t| t.contains("enum Color")));
    }

    #[test]
    fn test_language_name() {
        assert_eq!(CParser.language_name(), "C");
        assert_eq!(CParser.file_extensions(), &["c", "h"]);
    }
}
