//! Ruby language parser for code structure indexing.

use std::path::Path;

use tree_sitter::Parser;

use crate::code_index::ModuleInfo;
use crate::code_index::languages::{LanguageParser, child_text_by_field, node_text};

pub struct RubyParser;

impl LanguageParser for RubyParser {
    fn language_name(&self) -> &str {
        "Ruby"
    }

    fn file_extensions(&self) -> &[&str] {
        &["rb"]
    }

    fn module_path_for_file(&self, base: &Path, file: &Path) -> String {
        let rel = file.strip_prefix(base).unwrap_or(file);
        let s = rel.to_string_lossy();
        s.trim_end_matches(".rb").replace('/', "::")
    }

    fn parse_file(
        &self,
        source: &str,
        module_path: &str,
        parser: &mut Parser,
    ) -> Option<ModuleInfo> {
        parser
            .set_language(&tree_sitter_ruby::LANGUAGE.into())
            .ok()?;
        let tree = parser.parse(source, None)?;
        let root = tree.root_node();

        let mut functions = Vec::new();
        let mut types = Vec::new();
        let submodules = Vec::new();
        let mut imports = Vec::new();

        extract_from_node(
            &root,
            source,
            &mut functions,
            &mut types,
            &mut imports,
            &mut Visibility::Public,
        );

        Some(ModuleInfo {
            module_path: module_path.to_string(),
            language: "Ruby".to_string(),
            functions,
            types,
            submodules,
            imports,
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Visibility {
    Public,
    Private,
    Protected,
}

fn extract_from_node(
    node: &tree_sitter::Node,
    source: &str,
    functions: &mut Vec<String>,
    types: &mut Vec<String>,
    imports: &mut Vec<String>,
    visibility: &mut Visibility,
) {
    let mut current_vis = *visibility;

    for i in 0..node.named_child_count() {
        let child = node.named_child(i as u32).unwrap();

        match child.kind() {
            "method" => {
                if current_vis == Visibility::Public
                    && let Some(sig) = extract_method_sig(&child, source) {
                        functions.push(sig);
                    }
            }
            "singleton_method" => {
                if current_vis == Visibility::Public
                    && let Some(sig) = extract_singleton_method_sig(&child, source) {
                        functions.push(sig);
                    }
            }
            "class" => {
                if let Some(def) = extract_class_def(&child, source, "class") {
                    types.push(def);
                }
                // Reset visibility for class body
                let mut inner_vis = Visibility::Public;
                if let Some(body) = child.child_by_field_name("body") {
                    extract_from_node(&body, source, functions, types, imports, &mut inner_vis);
                }
            }
            "module" => {
                if let Some(name) = child_text_by_field(&child, source, "name") {
                    types.push(format!("module {name}"));
                }
                let mut inner_vis = Visibility::Public;
                if let Some(body) = child.child_by_field_name("body") {
                    extract_from_node(&body, source, functions, types, imports, &mut inner_vis);
                }
            }
            "call" => {
                let text = node_text(&child, source);
                // Check for visibility modifiers first (single-word calls)
                match text.as_str() {
                    "private" => current_vis = Visibility::Private,
                    "protected" => current_vis = Visibility::Protected,
                    "public" => current_vis = Visibility::Public,
                    _ => {
                        // Check for require/require_relative
                        if text.starts_with("require") {
                            imports.push(text);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn extract_method_sig(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    let mut sig = format!("def {name}");

    if let Some(params) = node.child_by_field_name("parameters") {
        sig.push_str(&format!("({})", node_text(&params, source)));
    }

    Some(sig)
}

fn extract_singleton_method_sig(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    let mut sig = format!("def self.{name}");

    if let Some(params) = node.child_by_field_name("parameters") {
        sig.push_str(&format!("({})", node_text(&params, source)));
    }

    Some(sig)
}

fn extract_class_def(node: &tree_sitter::Node, source: &str, keyword: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    let mut sig = format!("{keyword} {name}");

    // Check for superclass
    if let Some(superclass) = node.child_by_field_name("superclass") {
        sig.push_str(&format!(" < {}", node_text(&superclass, source)));
    }

    Some(sig)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str, module_path: &str) -> Option<ModuleInfo> {
        let mut parser = Parser::new();
        RubyParser.parse_file(source, module_path, &mut parser)
    }

    #[test]
    fn test_module_path() {
        let base = Path::new("/project/lib");
        assert_eq!(
            RubyParser.module_path_for_file(base, Path::new("/project/lib/server.rb")),
            "server"
        );
        assert_eq!(
            RubyParser.module_path_for_file(base, Path::new("/project/lib/app/models/user.rb")),
            "app::models::user"
        );
    }

    #[test]
    fn test_extracts_public_methods() {
        let source = r#"
require 'json'

class SearchService < BaseService
    def search(query)
        []
    end

    def self.build(params)
        new(params)
    end

    private

    def internal_helper
        # ...
    end
end

module Utils
    def format_result(data)
        data.to_json
    end
end

def standalone_function(x)
    x + 1
end
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.types.iter().any(|t| t.contains("class SearchService")));
        assert!(info.types.iter().any(|t| t.contains("module Utils")));
        assert!(info.functions.iter().any(|f| f.contains("search")));
        assert!(info.functions.iter().any(|f| f.contains("standalone_function")));
    }

    #[test]
    fn test_language_name() {
        assert_eq!(RubyParser.language_name(), "Ruby");
        assert_eq!(RubyParser.file_extensions(), &["rb"]);
    }
}
