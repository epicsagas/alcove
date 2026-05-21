//! C++ language parser for code structure indexing.

use std::path::Path;

use tree_sitter::Parser;

use crate::code_index::ModuleInfo;
use crate::code_index::languages::{
    LanguageParser, child_text_by_field, find_child_by_kind, node_text,
};

pub struct CppParser;

impl LanguageParser for CppParser {
    fn language_name(&self) -> &str {
        "C++"
    }

    fn file_extensions(&self) -> &[&str] {
        &["cpp", "cc", "cxx", "hpp", "hxx", "h"]
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
            .set_language(&tree_sitter_cpp::LANGUAGE.into())
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
            language: "C++".to_string(),
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
            "function_definition" => {
                if let Some(sig) = extract_function_sig(&child, source) {
                    functions.push(sig);
                }
            }
            "class_specifier" => {
                if let Some(def) = extract_class_def(&child, source) {
                    types.push(def);
                }
                extract_class_methods(&child, source, functions);
            }
            "struct_specifier" => {
                if let Some(def) = extract_struct_def(&child, source) {
                    types.push(def);
                }
            }
            "enum_specifier" => {
                if let Some(name) = child_text_by_field(&child, source, "name") {
                    types.push(format!("enum {name}"));
                }
            }
            "namespace_definition" => {
                // Recurse into namespace
                extract_from_node(&child, source, functions, types, imports);
            }
            "template_declaration" => {
                extract_template(&child, source, functions, types);
            }
            "preproc_include" => {
                imports.push(node_text(&child, source));
            }
            "using_declaration" | "namespace_alias_definition" => {
                // Skip
            }
            "declaration" => {
                // Could be a function declaration or typedef
                extract_declaration(&child, source, functions, types);
            }
            "type_definition" => {
                types.push(node_text(&child, source));
            }
            "declaration_list" => {
                // Body of namespace_definition
                extract_from_node(&child, source, functions, types, imports);
            }
            _ => {}
        }
    }
}

fn extract_function_sig(node: &tree_sitter::Node, source: &str) -> Option<String> {
    // Skip static functions
    if is_static(node, source) {
        return None;
    }

    if let Some(body) = node.child_by_field_name("body") {
        let sig = source[node.start_byte()..body.start_byte()].trim().to_string();
        return Some(sig);
    }

    Some(node_text(node, source))
}

fn is_static(node: &tree_sitter::Node, source: &str) -> bool {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i as u32) {
            let kind = child.kind();
            if kind == "storage_class_specifier" {
                return node_text(&child, source) == "static";
            }
            if kind == "function_declarator" || kind == "identifier" || kind == "pointer_declarator" || kind == "reference_declarator" {
                break;
            }
        }
    }
    false
}

fn extract_class_def(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    let mut sig = format!("class {name}");

    if let Some(body) = find_child_by_kind(node, "field_declaration_list") {
        let methods = collect_class_method_names(&body, source);
        if !methods.is_empty() {
            sig.push_str(&format!(" {{ {} }}", methods.join(", ")));
        }
    }

    Some(sig)
}

fn extract_struct_def(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    Some(format!("struct {name}"))
}

fn collect_class_method_names(body: &tree_sitter::Node, source: &str) -> Vec<String> {
    let mut methods = Vec::new();
    for i in 0..body.named_child_count() {
        let child = body.named_child(i as u32).unwrap();
        match child.kind() {
            "function_definition" | "declaration" => {
                // Try to extract method name from the declarator
                if let Some(decl) = find_function_declarator(&child)
                    && let Some(name) = extract_declarator_name(&decl, source) {
                        methods.push(format!("{name}()"));
                    }
            }
            "access_specifier" | "constructor_declaration" | "constructor_definition" => {
                if (child.kind() == "constructor_definition" || child.kind() == "constructor_declaration")
                    && let Some(name) = child_text_by_field(&child, source, "name") {
                        methods.push(format!("{name}()"));
                    }
            }
            _ => {}
        }
    }
    methods
}

fn find_function_declarator<'a>(node: &tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i as u32) {
            if child.kind() == "function_declarator" {
                return Some(child);
            }
            // Recurse into pointer_declarator
            if (child.kind() == "pointer_declarator" || child.kind() == "reference_declarator")
                && let Some(inner) = find_function_declarator(&child) {
                    return Some(inner);
                }
        }
    }
    None
}

fn extract_declarator_name(node: &tree_sitter::Node, source: &str) -> Option<String> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i as u32) {
            match child.kind() {
                "identifier" | "field_identifier" | "qualified_identifier" => {
                    return Some(node_text(&child, source));
                }
                "pointer_declarator" | "reference_declarator" => {
                    if let Some(name) = extract_declarator_name(&child, source) {
                        return Some(name);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn extract_template(
    node: &tree_sitter::Node,
    source: &str,
    functions: &mut Vec<String>,
    types: &mut Vec<String>,
) {
    // Template usually wraps a declaration or function_definition
    for i in 0..node.named_child_count() {
        let child = node.named_child(i as u32).unwrap();
        match child.kind() {
            "function_definition" => {
                if let Some(sig) = extract_function_sig(&child, source) {
                    functions.push(format!("template<> {sig}"));
                }
            }
            "class_specifier" => {
                if let Some(def) = extract_class_def(&child, source) {
                    types.push(format!("template<> {def}"));
                }
            }
            "struct_specifier" => {
                if let Some(name) = child_text_by_field(&child, source, "name") {
                    types.push(format!("template<> struct {name}"));
                }
            }
            _ => {}
        }
    }
}

fn extract_declaration(
    node: &tree_sitter::Node,
    source: &str,
    functions: &mut Vec<String>,
    _types: &mut Vec<String>,
) {
    let text = node_text(node, source);
    if text.contains('(') && text.contains(')') && !text.contains('=') && !is_static(node, source) {
        functions.push(text.trim().to_string());
    }
}

fn extract_class_methods(
    class_node: &tree_sitter::Node,
    source: &str,
    functions: &mut Vec<String>,
) {
    if let Some(body) = find_child_by_kind(class_node, "field_declaration_list") {
        for i in 0..body.named_child_count() {
            let child = body.named_child(i as u32).unwrap();
            match child.kind() {
                "function_definition" if !is_static(&child, source) => {
                    if let Some(sig) = extract_function_sig(&child, source) {
                        functions.push(sig);
                    }
                }
                "declaration" | "field_declaration" => {
                    let text = node_text(&child, source);
                    if text.contains('(') && text.contains(')') && !is_static(&child, source) {
                        functions.push(text.trim().to_string());
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str, module_path: &str) -> Option<ModuleInfo> {
        let mut parser = Parser::new();
        CppParser.parse_file(source, module_path, &mut parser)
    }

    #[test]
    fn test_module_path() {
        let base = Path::new("/project/src");
        assert_eq!(
            CppParser.module_path_for_file(base, Path::new("/project/src/main.cpp")),
            "main.cpp"
        );
    }

    #[test]
    fn test_extracts_classes_and_functions() {
        let source = r#"
#include <vector>
#include <string>

class SearchEngine {
public:
    std::vector<Result> search(const std::string& query);
    void index(Document doc);
private:
    static void helper();
};

namespace utils {
    std::string normalize(const std::string& input) {
        return input;
    }
}

template<typename T>
class Container {
public:
    void add(T item);
};
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.types.iter().any(|t| t.contains("class SearchEngine")));
        assert!(info.types.iter().any(|t| t.contains("class Container")));
        assert!(info.functions.iter().any(|f| f.contains("search")));
        assert!(info.functions.iter().any(|f| f.contains("normalize")));
        assert!(!info.functions.iter().any(|f| f.contains("helper")));
    }

    #[test]
    fn test_language_name() {
        assert_eq!(CppParser.language_name(), "C++");
        assert!(CppParser.file_extensions().contains(&"cpp"));
        assert!(CppParser.file_extensions().contains(&"hpp"));
    }
}

