//! Swift language parser for code structure indexing.

use std::path::Path;

use tree_sitter::Parser;

use crate::code_index::ModuleInfo;
use crate::code_index::languages::{
    LanguageParser, child_text_by_field, find_child_by_kind, node_text,
};

pub struct SwiftParser;

impl LanguageParser for SwiftParser {
    fn language_name(&self) -> &str {
        "Swift"
    }

    fn file_extensions(&self) -> &[&str] {
        &["swift"]
    }

    fn module_path_for_file(&self, base: &Path, file: &Path) -> String {
        let rel = file.strip_prefix(base).unwrap_or(file);
        let s = rel.to_string_lossy();
        s.trim_end_matches(".swift").to_string()
    }

    fn parse_file(
        &self,
        source: &str,
        module_path: &str,
        parser: &mut Parser,
    ) -> Option<ModuleInfo> {
        parser
            .set_language(&tree_sitter_swift::LANGUAGE.into())
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
            language: "Swift".to_string(),
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
            // tree-sitter-swift uses class_declaration for class, struct, and enum
            "class_declaration" => {
                let keyword = get_declaration_keyword(&child, source);
                if let Some(name) = child_text_by_field(&child, source, "name") {
                    types.push(format!("{keyword} {name}"));
                }
                extract_class_methods(&child, source, functions);
            }
            "protocol_declaration" => {
                if let Some(name) = child_text_by_field(&child, source, "name") {
                    types.push(format!("protocol {name} {{ ... }}"));
                }
            }
            "import_declaration" => {
                imports.push(node_text(&child, source));
            }
            "extension_declaration" => {}
            "actor_declaration" => {
                if let Some(name) = child_text_by_field(&child, source, "name") {
                    types.push(format!("actor {name}"));
                }
            }
            _ => {
                extract_from_node(&child, source, functions, types, imports);
            }
        }
    }
}

/// Determine the keyword: "class", "struct", or "enum".
fn get_declaration_keyword(node: &tree_sitter::Node, source: &str) -> &'static str {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i as u32)
            && !child.is_named()
        {
            let text = node_text(&child, source);
            match text.as_str() {
                "struct" => return "struct",
                "enum" => return "enum",
                "class" => return "class",
                _ => {}
            }
        }
    }
    "class"
}

fn has_private_modifier(node: &tree_sitter::Node, source: &str) -> bool {
    if let Some(modifiers) = find_child_by_kind(node, "modifiers") {
        for j in 0..modifiers.child_count() {
            if let Some(child) = modifiers.child(j as u32) {
                let text = node_text(&child, source);
                if text == "private" || text == "fileprivate" {
                    return true;
                }
            }
        }
    }
    false
}

fn extract_function_sig(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    let mut sig = format!("func {name}(");

    // tree-sitter-swift uses "parameter" not "parameters"
    if let Some(params) = node.child_by_field_name("parameters") {
        sig.push_str(&node_text(&params, source));
    } else if let Some(params) = find_child_by_kind(node, "parameter") {
        sig.push_str(&node_text(&params, source));
    }
    sig.push(')');

    if let Some(ret) = node.child_by_field_name("return_type") {
        sig.push_str(&format!(" -> {}", node_text(&ret, source)));
    }

    Some(sig)
}

fn extract_class_methods(
    class_node: &tree_sitter::Node,
    source: &str,
    functions: &mut Vec<String>,
) {
    // tree-sitter-swift uses "class_body"
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
        SwiftParser.parse_file(source, module_path, &mut parser)
    }

    #[test]
    fn test_module_path() {
        let base = Path::new("/project/Sources");
        assert_eq!(
            SwiftParser.module_path_for_file(base, Path::new("/project/Sources/Server.swift")),
            "Server"
        );
    }

    #[test]
    fn test_extracts_public_declarations() {
        let source = r#"
import Foundation

public class SearchService {
    public func search(query: String) -> [Result] {
        return []
    }
    private func helper() {}
}

struct Point {
    var x: Double
    var y: Double
}

enum Status {
    case active
    case inactive
}

protocol Repository {
    func find(by id: String) -> Result?
}

func topLevelFunction(x: Int) -> Int {
    return x + 1
}
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.types.iter().any(|t| t.contains("class SearchService")));
        assert!(info.types.iter().any(|t| t.contains("struct Point")));
        assert!(info.types.iter().any(|t| t.contains("enum Status")));
        assert!(info.types.iter().any(|t| t.contains("protocol Repository")));
    }

    #[test]
    fn test_language_name() {
        assert_eq!(SwiftParser.language_name(), "Swift");
        assert_eq!(SwiftParser.file_extensions(), &["swift"]);
    }
}
