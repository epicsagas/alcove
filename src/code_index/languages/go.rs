//! Go language parser for code structure indexing.

use std::path::Path;

use tree_sitter::Parser;

use crate::code_index::ModuleInfo;
use crate::code_index::languages::{LanguageParser, child_text_by_field, node_text};

pub struct GoParser;

impl LanguageParser for GoParser {
    fn language_name(&self) -> &str {
        "Go"
    }

    fn file_extensions(&self) -> &[&str] {
        &["go"]
    }

    fn module_path_for_file(&self, base: &Path, file: &Path) -> String {
        let rel = file.strip_prefix(base).unwrap_or(file);
        let s = rel.to_string_lossy();
        s.trim_end_matches(".go").to_string()
    }

    fn parse_file(
        &self,
        source: &str,
        module_path: &str,
        parser: &mut Parser,
    ) -> Option<ModuleInfo> {
        parser.set_language(&tree_sitter_go::LANGUAGE.into()).ok()?;
        let tree = parser.parse(source, None)?;
        let root = tree.root_node();

        let mut functions = Vec::new();
        let mut types = Vec::new();
        let submodules = Vec::new();
        let mut imports = Vec::new();

        for i in 0..root.named_child_count() {
            let child = root.named_child(i as u32).unwrap();

            match child.kind() {
                "function_declaration" => {
                    if let Some(sig) = extract_function_sig(&child, source) {
                        functions.push(sig);
                    }
                }
                "method_declaration" => {
                    if let Some(sig) = extract_method_sig(&child, source) {
                        functions.push(sig);
                    }
                }
                "type_declaration" => {
                    extract_type_decl(&child, source, &mut types);
                }
                "import_declaration" => {
                    imports.push(node_text(&child, source));
                }
                "package_clause" => {
                    // Could extract package name for context
                }
                _ => {}
            }
        }

        Some(ModuleInfo {
            module_path: module_path.to_string(),
            language: "Go".to_string(),
            functions,
            types,
            submodules,
            imports,
        })
    }
}

/// Check if a Go identifier is exported (starts with uppercase).
fn is_exported(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_uppercase())
}

fn extract_function_sig(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;

    if !is_exported(&name) {
        return None;
    }

    let mut sig = format!("func {name}(");

    if let Some(params) = node.child_by_field_name("parameters") {
        sig.push_str(&node_text(&params, source));
    }
    sig.push(')');

    if let Some(result) = node.child_by_field_name("result") {
        sig.push_str(&format!(" {}", node_text(&result, source)));
    }

    Some(sig)
}

fn extract_method_sig(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;

    if !is_exported(&name) {
        return None;
    }

    let receiver = node.child_by_field_name("receiver")?;
    let receiver_text = node_text(&receiver, source);

    let mut sig = format!("func {receiver_text} {name}(");

    if let Some(params) = node.child_by_field_name("parameters") {
        sig.push_str(&node_text(&params, source));
    }
    sig.push(')');

    if let Some(result) = node.child_by_field_name("result") {
        sig.push_str(&format!(" {}", node_text(&result, source)));
    }

    Some(sig)
}

fn extract_type_decl(node: &tree_sitter::Node, source: &str, types: &mut Vec<String>) {
    for i in 0..node.named_child_count() {
        let child = node.named_child(i as u32).unwrap();

        match child.kind() {
            "type_spec" => {
                if let Some(name) = child_text_by_field(&child, source, "name") {
                    if !is_exported(&name) {
                        continue;
                    }
                    if let Some(type_node) = child.child_by_field_name("type") {
                        let type_str = node_text(&type_node, source);
                        match type_node.kind() {
                            "struct_type" => {
                                let fields = collect_struct_fields(&type_node, source);
                                if fields.is_empty() {
                                    types.push(format!("struct {name} {{}}"));
                                } else {
                                    types
                                        .push(format!("struct {name} {{ {} }}", fields.join(", ")));
                                }
                            }
                            "interface_type" => {
                                let methods = collect_interface_methods(&type_node, source);
                                if methods.is_empty() {
                                    types.push(format!("interface {name} {{}}"));
                                } else {
                                    types.push(format!(
                                        "interface {name} {{ {} }}",
                                        methods.join(", ")
                                    ));
                                }
                            }
                            _ => {
                                types.push(format!("type {name} = {type_str}"));
                            }
                        }
                    }
                }
            }
            "type_alias" => {
                if let Some(name) = child_text_by_field(&child, source, "name")
                    && is_exported(&name)
                {
                    types.push(format!("type {name} = ..."));
                }
            }
            _ => {}
        }
    }
}

fn collect_struct_fields(node: &tree_sitter::Node, source: &str) -> Vec<String> {
    let mut fields = Vec::new();
    for i in 0..node.named_child_count() {
        let child = node.named_child(i as u32).unwrap();
        if child.kind() == "field_declaration_list" {
            for j in 0..child.named_child_count() {
                let field = child.named_child(j as u32).unwrap();
                if field.kind() == "field_declaration"
                    && let Some(names) = field.child_by_field_name("name")
                {
                    let name_str = node_text(&names, source);
                    if let Some(ftype) = field.child_by_field_name("type") {
                        fields.push(format!("{name_str}: {}", node_text(&ftype, source)));
                    }
                }
            }
        }
    }
    fields
}

fn collect_interface_methods(node: &tree_sitter::Node, source: &str) -> Vec<String> {
    let mut methods = Vec::new();
    for i in 0..node.named_child_count() {
        let child = node.named_child(i as u32).unwrap();
        if child.kind() == "method_elem"
            && let Some(name) = child_text_by_field(&child, source, "name")
        {
            methods.push(format!("{name}()"));
        }
    }
    methods
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str, module_path: &str) -> Option<ModuleInfo> {
        let mut parser = Parser::new();
        GoParser.parse_file(source, module_path, &mut parser)
    }

    #[test]
    fn test_module_path() {
        let base = Path::new("/project");
        assert_eq!(
            GoParser.module_path_for_file(base, Path::new("/project/main.go")),
            "main"
        );
        assert_eq!(
            GoParser.module_path_for_file(base, Path::new("/project/internal/server.go")),
            "internal/server"
        );
    }

    #[test]
    fn test_extracts_exported_functions() {
        let source = r#"
package main

func Search(query string) []Result {
    return nil
}

func internalHelper(x int) int {
    return x + 1
}

func (s *Server) HandleRequest(req Request) Response {
    return Response{}
}
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.functions.iter().any(|f| f.contains("Search")));
        assert!(info.functions.iter().any(|f| f.contains("HandleRequest")));
        assert!(!info.functions.iter().any(|f| f.contains("internalHelper")));
    }

    #[test]
    fn test_extracts_types() {
        let source = r#"
package main

type Server struct {
    Host string
    Port int
}

type Handler interface {
    ServeHTTP(w ResponseWriter, r *Request)
}

type privateType struct {
    x int
}
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.types.iter().any(|t| t.contains("struct Server")));
        assert!(info.types.iter().any(|t| t.contains("interface Handler")));
        assert!(!info.types.iter().any(|t| t.contains("privateType")));
    }

    #[test]
    fn test_language_name() {
        assert_eq!(GoParser.language_name(), "Go");
        assert_eq!(GoParser.file_extensions(), &["go"]);
    }
}
