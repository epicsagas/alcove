//! Java language parser for code structure indexing.

use std::path::Path;

use tree_sitter::Parser;

use crate::code_index::ModuleInfo;
use crate::code_index::languages::{LanguageParser, child_text_by_field, node_text};

pub struct JavaParser;

impl LanguageParser for JavaParser {
    fn language_name(&self) -> &str {
        "Java"
    }

    fn file_extensions(&self) -> &[&str] {
        &["java"]
    }

    fn module_path_for_file(&self, base: &Path, file: &Path) -> String {
        let rel = file.strip_prefix(base).unwrap_or(file);
        let s = rel.to_string_lossy();
        s.trim_end_matches(".java").replace('/', ".")
    }

    fn parse_file(
        &self,
        source: &str,
        module_path: &str,
        parser: &mut Parser,
    ) -> Option<ModuleInfo> {
        parser
            .set_language(&tree_sitter_java::LANGUAGE.into())
            .ok()?;
        let tree = parser.parse(source, None)?;
        let root = tree.root_node();

        let mut functions = Vec::new();
        let mut types = Vec::new();
        let submodules = Vec::new();
        let mut imports = Vec::new();

        extract_from_node(&root, source, &mut functions, &mut types, &mut imports);

        Some(ModuleInfo {
            module_path: module_path.to_string(),
            language: "Java".to_string(),
            functions,
            types,
            submodules,
            imports,
        })
    }
}

fn extract_from_node(
    node: &tree_sitter::Node,
    source: &str,
    functions: &mut Vec<String>,
    types: &mut Vec<String>,
    imports: &mut Vec<String>,
) {
    for i in 0..node.named_child_count() {
        let child = node.named_child(i as u32).unwrap();

        match child.kind() {
            "class_declaration" => {
                if is_public(&child) {
                    if let Some(def) = extract_class_def(&child, source) {
                        types.push(def);
                    }
                    extract_class_methods(&child, source, functions);
                }
            }
            "interface_declaration" => {
                if is_public(&child)
                    && let Some(def) = extract_interface_def(&child, source)
                {
                    types.push(def);
                }
            }
            "enum_declaration" => {
                if is_public(&child)
                    && let Some(def) = extract_enum_def(&child, source)
                {
                    types.push(def);
                }
            }
            "record_declaration" => {
                if is_public(&child)
                    && let Some(name) = child_text_by_field(&child, source, "name")
                {
                    types.push(format!("record {name}(...)"));
                }
            }
            "import_declaration" => {
                imports.push(node_text(&child, source));
            }
            "method_declaration" => {
                // Top-level method (rare but valid in some contexts)
                if is_public(&child)
                    && let Some(sig) = extract_method_sig(&child, source)
                {
                    functions.push(sig);
                }
            }
            _ => {
                // Recurse into nested structures
                extract_from_node(&child, source, functions, types, imports);
            }
        }
    }
}

/// Check if a declaration has a "public" modifier.
fn is_public(node: &tree_sitter::Node) -> bool {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i as u32)
            && child.kind() == "modifiers"
        {
            for j in 0..child.child_count() {
                if let Some(modifier) = child.child(j as u32)
                    && !modifier.is_named()
                    && modifier.kind() == "public"
                {
                    return true;
                }
            }
        }
    }
    false
}

fn extract_class_def(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    let mut sig = format!("class {name}");

    if let Some(type_params) = node.child_by_field_name("type_parameters") {
        sig.push_str(&node_text(&type_params, source));
    }

    if let Some(superclass) = node.child_by_field_name("superclass") {
        sig.push_str(&format!(" extends {}", node_text(&superclass, source)));
    }

    if let Some(interfaces) = node.child_by_field_name("interfaces") {
        sig.push_str(&format!(" {}", node_text(&interfaces, source)));
    }

    Some(sig)
}

fn extract_interface_def(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    let mut sig = format!("interface {name}");

    if let Some(type_params) = node.child_by_field_name("type_parameters") {
        sig.push_str(&node_text(&type_params, source));
    }

    sig.push_str(" { ... }");
    Some(sig)
}

fn extract_enum_def(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;

    if let Some(body) = node.child_by_field_name("body") {
        let variants = collect_enum_constants(&body, source);
        if variants.is_empty() {
            return Some(format!("enum {name} {{}}"));
        }
        return Some(format!("enum {name} {{ {} }}", variants.join(", ")));
    }

    Some(format!("enum {name} {{}}"))
}

fn collect_enum_constants(body: &tree_sitter::Node, source: &str) -> Vec<String> {
    let mut variants = Vec::new();
    for i in 0..body.named_child_count() {
        let child = body.named_child(i as u32).unwrap();
        if child.kind() == "enum_constant"
            && let Some(name) = child_text_by_field(&child, source, "name")
        {
            variants.push(name);
        }
    }
    variants
}

fn extract_class_methods(
    class_node: &tree_sitter::Node,
    source: &str,
    functions: &mut Vec<String>,
) {
    if let Some(body) = class_node.child_by_field_name("body") {
        for i in 0..body.named_child_count() {
            let child = body.named_child(i as u32).unwrap();
            if child.kind() == "method_declaration"
                && is_public(&child)
                && let Some(sig) = extract_method_sig(&child, source)
            {
                functions.push(sig);
            }
        }
    }
}

fn extract_method_sig(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;

    let mut sig = String::new();

    // Return type
    if let Some(ret_type) = node.child_by_field_name("type") {
        sig.push_str(&node_text(&ret_type, source));
        sig.push(' ');
    }

    sig.push_str(&name);
    sig.push('(');

    if let Some(params) = node.child_by_field_name("parameters") {
        sig.push_str(&node_text(&params, source));
    }
    sig.push(')');

    Some(sig)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str, module_path: &str) -> Option<ModuleInfo> {
        let mut parser = Parser::new();
        JavaParser.parse_file(source, module_path, &mut parser)
    }

    #[test]
    fn test_module_path() {
        let base = Path::new("/project/src/main/java");
        assert_eq!(
            JavaParser.module_path_for_file(
                base,
                Path::new("/project/src/main/java/com/example/Server.java")
            ),
            "com.example.Server"
        );
    }

    #[test]
    fn test_extracts_public_classes() {
        let source = r#"
package com.example;

import java.util.List;

public class SearchService {
    public List<Result> search(String query) {
        return null;
    }

    private void internalHelper() {}
}

class PackagePrivateClass {
    public void method() {}
}
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.types.iter().any(|t| t.contains("class SearchService")));
        assert!(!info.types.iter().any(|t| t.contains("PackagePrivateClass")));
        assert!(info.functions.iter().any(|f| f.contains("search")));
        assert!(!info.functions.iter().any(|f| f.contains("internalHelper")));
    }

    #[test]
    fn test_extracts_enums() {
        let source = r#"
public enum Color {
    RED, GREEN, BLUE
}
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.types.iter().any(|t| t.contains("enum Color")));
        assert!(info.types.iter().any(|t| t.contains("RED")));
    }

    #[test]
    fn test_language_name() {
        assert_eq!(JavaParser.language_name(), "Java");
        assert_eq!(JavaParser.file_extensions(), &["java"]);
    }
}
