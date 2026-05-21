//! Kotlin language parser for code structure indexing.

use std::path::Path;

use tree_sitter::Parser;

use crate::code_index::ModuleInfo;
use crate::code_index::languages::{
    LanguageParser, child_text_by_field, find_child_by_kind, node_text,
};

pub struct KotlinParser;

impl LanguageParser for KotlinParser {
    fn language_name(&self) -> &str {
        "Kotlin"
    }

    fn file_extensions(&self) -> &[&str] {
        &["kt", "kts"]
    }

    fn module_path_for_file(&self, base: &Path, file: &Path) -> String {
        let rel = file.strip_prefix(base).unwrap_or(file);
        let s = rel.to_string_lossy();
        s.trim_end_matches(".kt")
            .trim_end_matches(".kts")
            .replace('/', ".")
    }

    fn parse_file(
        &self,
        source: &str,
        module_path: &str,
        parser: &mut Parser,
    ) -> Option<ModuleInfo> {
        parser
            .set_language(&tree_sitter_kotlin_ng::LANGUAGE.into())
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
            language: "Kotlin".to_string(),
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
            "function_declaration" => {
                if !has_private_modifier(&child, source)
                    && let Some(sig) = extract_function_sig(&child, source)
                {
                    functions.push(sig);
                }
            }
            "class_declaration" => {
                let keyword = get_class_keyword(&child, source);
                if !has_private_modifier(&child, source) {
                    if let Some(name) = child_text_by_field(&child, source, "name") {
                        let mut sig = format!("{keyword} {name}");
                        if let Some(type_params) = child.child_by_field_name("type_parameters") {
                            sig.push_str(&node_text(&type_params, source));
                        }
                        if let Some(superclass) = child.child_by_field_name("superclass") {
                            sig.push_str(&format!(" : {}", node_text(&superclass, source)));
                        }
                        types.push(sig);
                    }
                    extract_class_methods(&child, source, functions);
                }
            }
            "object_declaration" => {
                if !has_private_modifier(&child, source)
                    && let Some(name) = child_text_by_field(&child, source, "name")
                {
                    types.push(format!("object {name}"));
                }
            }
            "property_declaration" => {}
            "import" => {
                imports.push(node_text(&child, source));
            }
            "package_header" => {}
            _ => {
                extract_from_node(&child, source, functions, types, imports);
            }
        }
    }
}

/// Determine the keyword for a class_declaration: "class", "interface", or "enum class".
/// tree-sitter-kotlin-ng uses `class_declaration` for all three, distinguished by
/// the first child keyword and presence of a `class_modifier` with value `enum`.
fn get_class_keyword(node: &tree_sitter::Node, source: &str) -> &'static str {
    // Check if it's an enum class (has class_modifier "enum" in modifiers)
    if is_enum_class(node, source) {
        return "enum class";
    }
    // Check the first child keyword
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i as u32) {
            let text = node_text(&child, source);
            match text.as_str() {
                "interface" => return "interface",
                "class" => return "class",
                _ => {}
            }
        }
    }
    "class"
}

fn is_enum_class(node: &tree_sitter::Node, source: &str) -> bool {
    if let Some(modifiers) = find_child_by_kind(node, "modifiers") {
        for j in 0..modifiers.child_count() {
            if let Some(cm) = modifiers.child(j as u32)
                && cm.kind() == "class_modifier"
            {
                let text = node_text(&cm, source);
                if text.contains("enum") {
                    return true;
                }
            }
        }
    }
    false
}

fn has_private_modifier(node: &tree_sitter::Node, source: &str) -> bool {
    if let Some(modifiers) = find_child_by_kind(node, "modifiers") {
        for j in 0..modifiers.child_count() {
            if let Some(child) = modifiers.child(j as u32) {
                if child.kind() == "private" {
                    return true;
                }
                let text = node_text(&child, source);
                if text == "private" || text == "internal" {
                    return true;
                }
            }
        }
    }
    false
}

fn extract_function_sig(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    let mut sig = format!("fun {name}(");

    // tree-sitter-kotlin-ng uses "function_value_parameters" not "parameters"
    if let Some(params) = node.child_by_field_name("parameters") {
        sig.push_str(&node_text(&params, source));
    } else {
        // Try to find function_value_parameters by kind
        if let Some(params) = find_child_by_kind(node, "function_value_parameters") {
            sig.push_str(&node_text(&params, source));
        }
    }
    sig.push(')');

    if let Some(ret) = node.child_by_field_name("return_type") {
        sig.push_str(&format!(": {}", node_text(&ret, source)));
    }

    Some(sig)
}

fn extract_class_methods(
    class_node: &tree_sitter::Node,
    source: &str,
    functions: &mut Vec<String>,
) {
    // tree-sitter-kotlin-ng uses "class_body"
    let body = class_node
        .child_by_field_name("body")
        .or_else(|| find_child_by_kind(class_node, "class_body"));
    if let Some(body) = body {
        for i in 0..body.named_child_count() {
            let child = body.named_child(i as u32).unwrap();
            if child.kind() == "function_declaration"
                && !has_private_modifier(&child, source)
                && let Some(sig) = extract_function_sig(&child, source)
            {
                functions.push(sig);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str, module_path: &str) -> Option<ModuleInfo> {
        let mut parser = Parser::new();
        KotlinParser.parse_file(source, module_path, &mut parser)
    }

    #[test]
    fn test_module_path() {
        let base = Path::new("/project/src/main/kotlin");
        assert_eq!(
            KotlinParser.module_path_for_file(
                base,
                Path::new("/project/src/main/kotlin/com/example/Server.kt")
            ),
            "com.example.Server"
        );
    }

    #[test]
    fn test_extracts_public_declarations() {
        let source = r#"
package com.example

import kotlin.collections.List

class SearchService : BaseService() {
    fun search(query: String): List<Result> {
        return emptyList()
    }
    private fun internalHelper() {}
}

interface Repository {
    fun findById(id: String): Result?
}

object Config {
    const val VERSION = "1.0"
}

enum class Status {
    ACTIVE, INACTIVE
}

fun topLevelFunction(x: Int): Int = x + 1

private fun hiddenFunction() {}
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.types.iter().any(|t| t.contains("class SearchService")));
        assert!(info.types.iter().any(|t| t.contains("interface Repository")));
        assert!(info.types.iter().any(|t| t.contains("object Config")));
        assert!(info.types.iter().any(|t| t.contains("enum class Status")));
        assert!(info.functions.iter().any(|f| f.contains("topLevelFunction")));
        assert!(info.functions.iter().any(|f| f.contains("search")));
        assert!(!info.functions.iter().any(|f| f.contains("internalHelper")));
        assert!(!info.functions.iter().any(|f| f.contains("hiddenFunction")));
    }

    #[test]
    fn test_language_name() {
        assert_eq!(KotlinParser.language_name(), "Kotlin");
        assert_eq!(KotlinParser.file_extensions(), &["kt", "kts"]);
    }
}
