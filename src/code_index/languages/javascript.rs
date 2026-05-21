//! JavaScript / JSX language parser for code structure indexing.

use std::path::Path;

use tree_sitter::Parser;

use crate::code_index::ModuleInfo;
use crate::code_index::languages::{
    LanguageParser, child_text_by_field, find_child_by_kind, node_text,
};

pub struct JavascriptParser;

impl LanguageParser for JavascriptParser {
    fn language_name(&self) -> &str {
        "JavaScript"
    }

    fn file_extensions(&self) -> &[&str] {
        &["js", "jsx", "mjs", "cjs"]
    }

    fn module_path_for_file(&self, base: &Path, file: &Path) -> String {
        let rel = file.strip_prefix(base).unwrap_or(file);
        let s = rel.to_string_lossy();

        if s.ends_with("/index.js") || s.ends_with("/index.jsx") || s.ends_with("/index.mjs") {
            let parent = rel.parent().unwrap_or(Path::new(""));
            return parent.to_string_lossy().to_string();
        }

        s.trim_end_matches(".js")
            .trim_end_matches(".jsx")
            .trim_end_matches(".mjs")
            .trim_end_matches(".cjs")
            .to_string()
    }

    fn parse_file(
        &self,
        source: &str,
        module_path: &str,
        parser: &mut Parser,
    ) -> Option<ModuleInfo> {
        parser
            .set_language(&tree_sitter_javascript::LANGUAGE.into())
            .ok()?;
        let tree = parser.parse(source, None)?;
        let root = tree.root_node();

        let mut functions = Vec::new();
        let mut types = Vec::new();
        let submodules = Vec::new();
        let mut imports = Vec::new();

        extract_declarations(&root, source, &mut functions, &mut types, &mut imports);

        Some(ModuleInfo {
            module_path: module_path.to_string(),
            language: "JavaScript".to_string(),
            functions,
            types,
            submodules,
            imports,
        })
    }
}

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
                    extract_single_decl(&decl, source, functions, types);
                }
            }
            "function_declaration" | "generator_function_declaration" => {
                extract_single_decl(&child, source, functions, types);
            }
            "class_declaration" => {
                extract_single_decl(&child, source, functions, types);
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
    functions: &mut Vec<String>,
    types: &mut Vec<String>,
) {
    match node.kind() {
        "function_declaration" | "generator_function_declaration" => {
            if let Some(sig) = extract_function_sig(node, source) {
                functions.push(sig);
            }
        }
        "class_declaration" => {
            if let Some(def) = extract_class_def(node, source) {
                types.push(def);
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            // Handle: export const foo = () => {}
            for j in 0..node.named_child_count() {
                let vc = node.named_child(j as u32).unwrap();
                if vc.kind() == "variable_declarator"
                    && let Some(name) = vc.child_by_field_name("name")
                {
                    let name_str = node_text(&name, source);
                    if let Some(value) = vc.child_by_field_name("value")
                        && (value.kind() == "arrow_function"
                            || value.kind() == "function_expression")
                    {
                        functions.push(format!("{name_str}()"));
                    }
                }
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

    Some(sig)
}

fn extract_class_def(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    let mut sig = format!("class {name}");

    // Check extends
    if let Some(heritage) = find_child_by_kind(node, "class_heritage") {
        sig.push_str(&format!(" {}", node_text(&heritage, source)));
    }

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
        if child.kind() == "method_definition"
            && let Some(name) = child.child_by_field_name("name")
        {
            methods.push(format!("{}()", node_text(&name, source)));
        }
    }
    methods
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str, module_path: &str) -> Option<ModuleInfo> {
        let mut parser = Parser::new();
        JavascriptParser.parse_file(source, module_path, &mut parser)
    }

    #[test]
    fn test_module_path() {
        let base = Path::new("/project/src");
        assert_eq!(
            JavascriptParser.module_path_for_file(base, Path::new("/project/src/server.js")),
            "server"
        );
        assert_eq!(
            JavascriptParser
                .module_path_for_file(base, Path::new("/project/src/components/index.jsx")),
            "components"
        );
    }

    #[test]
    fn test_extracts_functions() {
        let source = r#"
export function search(query) {
    return [];
}

function helper() {
    return 1;
}

export const fetchData = () => {
    return fetch(url);
};
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.functions.iter().any(|f| f.contains("search")));
        assert!(info.functions.iter().any(|f| f.contains("fetchData")));
    }

    #[test]
    fn test_extracts_classes() {
        let source = r#"
export class SearchEngine extends BaseEngine {
    search(query) { return []; }
    index(doc) {}
}
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.types.iter().any(|t| t.contains("class SearchEngine")));
    }

    #[test]
    fn test_language_name() {
        assert_eq!(JavascriptParser.language_name(), "JavaScript");
        assert_eq!(
            JavascriptParser.file_extensions(),
            &["js", "jsx", "mjs", "cjs"]
        );
    }
}
