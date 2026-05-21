//! Python language parser for code structure indexing.

use std::path::Path;

use tree_sitter::Parser;

use crate::code_index::ModuleInfo;
use crate::code_index::languages::{
    LanguageParser, child_text_by_field, node_text,
};

pub struct PythonParser;

impl LanguageParser for PythonParser {
    fn language_name(&self) -> &str {
        "Python"
    }

    fn file_extensions(&self) -> &[&str] {
        &["py", "pyi"]
    }

    fn module_path_for_file(&self, base: &Path, file: &Path) -> String {
        let rel = file.strip_prefix(base).unwrap_or(file);
        let s = rel.to_string_lossy();

        // Handle __init__.py → parent package name
        if s.ends_with("/__init__.py") {
            let parent = rel.parent().unwrap_or(Path::new(""));
            return parent.to_string_lossy().replace('/', ".");
        }

        s.trim_end_matches(".py")
            .trim_end_matches(".pyi")
            .replace('/', ".")
    }

    fn parse_file(
        &self,
        source: &str,
        module_path: &str,
        parser: &mut Parser,
    ) -> Option<ModuleInfo> {
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
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
                    if let Some(sig) = extract_function_sig(&child, source) {
                        functions.push(sig);
                    }
                }
                "class_definition" => {
                    if let Some(def) = extract_class_def(&child, source) {
                        types.push(def);
                    }
                }
                "decorated_definition" => {
                    extract_decorated(&child, source, &mut functions, &mut types);
                }
                "import_statement" | "import_from_statement" => {
                    imports.push(node_text(&child, source));
                }
                _ => {}
            }
        }

        Some(ModuleInfo {
            module_path: module_path.to_string(),
            language: "Python".to_string(),
            functions,
            types,
            submodules,
            imports,
        })
    }
}

/// Check if a name is "private" (starts with _ but isn't a dunder like __init__).
fn is_private_name(name: &str) -> bool {
    if !name.starts_with('_') {
        return false;
    }
    // Dunders like __init__, __str__ are considered "special" not private
    if name.starts_with("__") && name.ends_with("__") && name.len() > 4 {
        return false;
    }
    true
}

fn extract_function_sig(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;

    // Skip private functions
    if is_private_name(&name) {
        return None;
    }

    let mut sig = format!("def {name}(");

    if let Some(params) = node.child_by_field_name("parameters") {
        sig.push_str(&node_text(&params, source));
    }
    sig.push(')');

    if let Some(ret) = node.child_by_field_name("return_type") {
        sig.push_str(&format!(" -> {}", node_text(&ret, source)));
    }

    Some(sig)
}

fn extract_class_def(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;

    if is_private_name(&name) {
        return None;
    }

    let mut sig = format!("class {name}");

    if let Some(superclasses) = node.child_by_field_name("superclasses") {
        sig.push_str(&format!("({})", node_text(&superclasses, source)));
    }

    // Extract method names from body
    if let Some(body) = node.child_by_field_name("body") {
        let methods = collect_class_methods(&body, source);
        if !methods.is_empty() {
            sig.push_str(&format!(": {{ {} }}", methods.join(", ")));
        }
    }

    Some(sig)
}

fn collect_class_methods(body: &tree_sitter::Node, source: &str) -> Vec<String> {
    let mut methods = Vec::new();
    for i in 0..body.named_child_count() {
        let child = body.named_child(i as u32).unwrap();
        match child.kind() {
            "function_definition" => {
                if let Some(name) = child_text_by_field(&child, source, "name")
                    && !is_private_name(&name) {
                        methods.push(format!("{name}()"));
                    }
            }
            "decorated_definition" => {
                if let Some(definition) = child.child_by_field_name("definition")
                    && definition.kind() == "function_definition"
                        && let Some(name) = child_text_by_field(&definition, source, "name")
                            && !is_private_name(&name) {
                                methods.push(format!("{name}()"));
                            }
            }
            _ => {}
        }
    }
    methods
}

fn extract_decorated(
    node: &tree_sitter::Node,
    source: &str,
    functions: &mut Vec<String>,
    types: &mut Vec<String>,
) {
    if let Some(definition) = node.child_by_field_name("definition") {
        match definition.kind() {
            "function_definition" => {
                if let Some(sig) = extract_function_sig(&definition, source) {
                    // Prepend decorator info
                    let decorators: Vec<String> = (0..node.named_child_count())
                        .filter_map(|i| {
                            let c = node.named_child(i as u32).unwrap();
                            if c.kind() == "decorator" {
                                Some(node_text(&c, source))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if decorators.is_empty() {
                        functions.push(sig);
                    } else {
                        functions.push(format!("{} {}", decorators.join(" "), sig));
                    }
                }
            }
            "class_definition" => {
                if let Some(def) = extract_class_def(&definition, source) {
                    types.push(def);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str, module_path: &str) -> Option<ModuleInfo> {
        let mut parser = Parser::new();
        PythonParser.parse_file(source, module_path, &mut parser)
    }

    #[test]
    fn test_module_path() {
        let base = Path::new("/project/src");
        assert_eq!(
            PythonParser.module_path_for_file(base, Path::new("/project/src/server.py")),
            "server"
        );
        assert_eq!(
            PythonParser.module_path_for_file(base, Path::new("/project/src/utils/helpers.py")),
            "utils.helpers"
        );
        assert_eq!(
            PythonParser.module_path_for_file(base, Path::new("/project/src/pkg/__init__.py")),
            "pkg"
        );
    }

    #[test]
    fn test_extracts_public_functions() {
        let source = r#"
def search(query: str) -> list[Result]:
    return []

def _internal_helper(x: int) -> int:
    return x + 1

async def fetch_data(url: str) -> Response:
    return await fetch(url)
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.functions.iter().any(|f| f.contains("search")));
        assert!(info.functions.iter().any(|f| f.contains("fetch_data")));
        assert!(!info.functions.iter().any(|f| f.contains("_internal_helper")));
    }

    #[test]
    fn test_extracts_classes() {
        let source = r#"
class SearchEngine:
    def search(self, query: str) -> list:
        return []
    def _score(self, doc: Document) -> float:
        return 0.0

class _InternalClass:
    pass

class Child(BaseMixin, Serializable):
    def process(self) -> None:
        pass
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.types.iter().any(|t| t.contains("class SearchEngine")));
        assert!(info.types.iter().any(|t| t.contains("search")));
        assert!(info.types.iter().any(|t| t.contains("class Child")));
        assert!(!info.types.iter().any(|t| t.contains("_InternalClass")));
    }

    #[test]
    fn test_language_name() {
        assert_eq!(PythonParser.language_name(), "Python");
        assert_eq!(PythonParser.file_extensions(), &["py", "pyi"]);
    }
}
