//! TypeScript / TSX language parser for code structure indexing.

use std::path::Path;

use tree_sitter::Parser;

use crate::code_index::ModuleInfo;
use crate::code_index::languages::{
    LanguageParser, child_text_by_field, find_child_by_kind, node_text,
};

pub struct TypescriptParser;

impl LanguageParser for TypescriptParser {
    fn language_name(&self) -> &str {
        "TypeScript"
    }

    fn file_extensions(&self) -> &[&str] {
        &["ts", "tsx"]
    }

    fn module_path_for_file(&self, base: &Path, file: &Path) -> String {
        let rel = file.strip_prefix(base).unwrap_or(file);
        let s = rel.to_string_lossy();

        if s.ends_with("/index.ts") || s.ends_with("/index.tsx") {
            let parent = rel.parent().unwrap_or(Path::new(""));
            return parent.to_string_lossy().to_string();
        }

        s.trim_end_matches(".ts")
            .trim_end_matches(".tsx")
            .to_string()
    }

    fn parse_file(
        &self,
        source: &str,
        module_path: &str,
        parser: &mut Parser,
    ) -> Option<ModuleInfo> {
        let lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        parser.set_language(&lang).ok()?;
        let tree = parser.parse(source, None)?;
        let root = tree.root_node();

        let mut functions = Vec::new();
        let mut types = Vec::new();
        let submodules = Vec::new();
        let mut imports = Vec::new();

        extract_declarations(&root, source, &mut functions, &mut types, &mut imports);

        Some(ModuleInfo {
            module_path: module_path.to_string(),
            language: "TypeScript".to_string(),
            functions,
            types,
            submodules,
            imports,
        })
    }
}

/// Recursively extract declarations, handling export_statement wrappers.
fn extract_declarations(
    node: &tree_sitter::Node,
    source: &str,
    functions: &mut Vec<String>,
    types: &mut Vec<String>,
    imports: &mut Vec<String>,
) {
    for i in 0..node.named_child_count() {
        let child = node.named_child(i as u32).unwrap();
        match child.kind() {
            "export_statement" => {
                if let Some(decl) = child.child_by_field_name("declaration") {
                    extract_single_decl(&decl, source, true, functions, types);
                }
            }
            "function_declaration" | "generator_function_declaration" => {
                // Non-exported top-level function — still include for indexing
                extract_single_decl(&child, source, false, functions, types);
            }
            "class_declaration" | "abstract_class_declaration" => {
                extract_single_decl(&child, source, false, functions, types);
            }
            "interface_declaration" => {
                extract_single_decl(&child, source, false, functions, types);
            }
            "type_alias_declaration" => {
                extract_single_decl(&child, source, false, functions, types);
            }
            "enum_declaration" => {
                extract_single_decl(&child, source, false, functions, types);
            }
            "import_statement" => {
                imports.push(node_text(&child, source));
            }
            _ => {}
        }
    }
}

fn extract_single_decl(
    node: &tree_sitter::Node,
    source: &str,
    _exported: bool,
    functions: &mut Vec<String>,
    types: &mut Vec<String>,
) {
    match node.kind() {
        "function_declaration" | "generator_function_declaration" => {
            if let Some(sig) = extract_function_sig(node, source) {
                functions.push(sig);
            }
        }
        "class_declaration" | "abstract_class_declaration" => {
            if let Some(def) = extract_class_def(node, source) {
                types.push(def);
            }
        }
        "interface_declaration" => {
            if let Some(def) = extract_interface_def(node, source) {
                types.push(def);
            }
        }
        "type_alias_declaration" => {
            if let Some(name) = child_text_by_field(node, source, "name") {
                types.push(format!("type {name} = ..."));
            }
        }
        "enum_declaration" => {
            if let Some(def) = extract_enum_def(node, source) {
                types.push(def);
            }
        }
        _ => {}
    }
}

fn extract_function_sig(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    let mut sig = format!("function {name}(");

    if let Some(params) = node.child_by_field_name("parameters") {
        sig.push_str(&node_text(&params, source));
    }
    sig.push(')');

    if let Some(ret) = node.child_by_field_name("return_type") {
        sig.push_str(&format!(": {}", node_text(&ret, source)));
    }

    Some(sig)
}

fn extract_class_def(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    let prefix = if node.kind() == "abstract_class_declaration" {
        "abstract class"
    } else {
        "class"
    };

    let mut sig = format!("{prefix} {name}");

    // Check extends
    if let Some(extends) = find_child_by_kind(node, "extends_type_clause") {
        sig.push_str(&format!(" {}", node_text(&extends, source)));
    }

    // Check implements
    if let Some(implements) = find_child_by_kind(node, "implements_clause") {
        sig.push_str(&format!(" {}", node_text(&implements, source)));
    }

    // Collect method names from class_body
    if let Some(body) = node.child_by_field_name("body") {
        let methods = collect_class_methods(&body, source);
        if !methods.is_empty() {
            sig.push_str(&format!(" {{ {} }}", methods.join(", ")));
        }
    }

    Some(sig)
}

fn collect_class_methods(body: &tree_sitter::Node, source: &str) -> Vec<String> {
    let mut methods = Vec::new();
    for i in 0..body.named_child_count() {
        let child = body.named_child(i as u32).unwrap();
        match child.kind() {
            "method_definition" | "method_signature" => {
                if let Some(name) = child.child_by_field_name("name") {
                    let name_str = node_text(&name, source);
                    methods.push(format!("{name_str}()"));
                }
            }
            _ => {}
        }
    }
    methods
}

fn extract_interface_def(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    let mut sig = format!("interface {name}");

    if let Some(params) = node.child_by_field_name("type_parameters") {
        sig.push_str(&node_text(&params, source));
    }

    if let Some(extends) = find_child_by_kind(node, "extends_type_clause") {
        sig.push_str(&format!(" {}", node_text(&extends, source)));
    }

    sig.push_str(" { ... }");
    Some(sig)
}

fn extract_enum_def(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;

    if let Some(body) = node.child_by_field_name("body") {
        let variants = collect_enum_variants(&body, source);
        if variants.is_empty() {
            return Some(format!("enum {name} {{}}"));
        }
        return Some(format!("enum {name} {{ {} }}", variants.join(", ")));
    }

    Some(format!("enum {name} {{}}"))
}

fn collect_enum_variants(body: &tree_sitter::Node, source: &str) -> Vec<String> {
    let mut variants = Vec::new();
    for i in 0..body.named_child_count() {
        let child = body.named_child(i as u32).unwrap();
        if child.kind() == "enum_assignment" || child.kind() == "property_identifier" {
            variants.push(node_text(&child, source));
        }
    }
    variants
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str, module_path: &str) -> Option<ModuleInfo> {
        let mut parser = Parser::new();
        TypescriptParser.parse_file(source, module_path, &mut parser)
    }

    #[test]
    fn test_module_path() {
        let base = Path::new("/project/src");
        assert_eq!(
            TypescriptParser.module_path_for_file(base, Path::new("/project/src/server.ts")),
            "server"
        );
        assert_eq!(
            TypescriptParser.module_path_for_file(base, Path::new("/project/src/utils/helpers.ts")),
            "utils/helpers"
        );
        assert_eq!(
            TypescriptParser
                .module_path_for_file(base, Path::new("/project/src/components/index.ts")),
            "components"
        );
    }

    #[test]
    fn test_extracts_exported_functions() {
        let source = r#"
export function search(query: string): Result[] {
    return [];
}

function internalHelper(x: number): number {
    return x + 1;
}

export async function fetchData(url: string): Promise<Response> {
    return fetch(url);
}
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.functions.len() >= 2);
        assert!(info.functions.iter().any(|f| f.contains("search")));
        assert!(info.functions.iter().any(|f| f.contains("fetchData")));
    }

    #[test]
    fn test_extracts_interfaces_and_types() {
        let source = r#"
export interface SearchResult {
    title: string;
    score: number;
}

export type Status = "active" | "inactive";

export enum Color {
    Red,
    Green,
    Blue,
}
"#;
        let info = parse(source, "test").unwrap();
        assert!(
            info.types
                .iter()
                .any(|t| t.contains("interface SearchResult"))
        );
        assert!(info.types.iter().any(|t| t.contains("type Status")));
        assert!(info.types.iter().any(|t| t.contains("enum Color")));
    }

    #[test]
    fn test_extracts_classes() {
        let source = r#"
export class SearchEngine {
    search(query: string): Result[] { return []; }
    index(doc: Document): void {}
}

export abstract class BaseHandler {
    abstract handle(req: Request): Response;
}
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.types.iter().any(|t| t.contains("class SearchEngine")));
        assert!(
            info.types
                .iter()
                .any(|t| t.contains("abstract class BaseHandler"))
        );
    }

    #[test]
    fn test_language_name() {
        assert_eq!(TypescriptParser.language_name(), "TypeScript");
        assert_eq!(TypescriptParser.file_extensions(), &["ts", "tsx"]);
    }
}
