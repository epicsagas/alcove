//! C# language parser for code structure indexing.

use std::path::Path;

use tree_sitter::Parser;

use crate::code_index::ModuleInfo;
use crate::code_index::languages::{
    LanguageParser, child_text_by_field, find_child_by_kind, node_text,
};

pub struct CSharpParser;

impl LanguageParser for CSharpParser {
    fn language_name(&self) -> &str {
        "C#"
    }

    fn file_extensions(&self) -> &[&str] {
        &["cs"]
    }

    fn module_path_for_file(&self, base: &Path, file: &Path) -> String {
        let rel = file.strip_prefix(base).unwrap_or(file);
        let s = rel.to_string_lossy();
        s.trim_end_matches(".cs").replace('/', ".")
    }

    fn parse_file(
        &self,
        source: &str,
        module_path: &str,
        parser: &mut Parser,
    ) -> Option<ModuleInfo> {
        parser
            .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
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
            language: "C#".to_string(),
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
                if is_public(&child, source) {
                    if let Some(def) = extract_class_def(&child, source, "class") {
                        types.push(def);
                    }
                    extract_class_members(&child, source, functions);
                }
            }
            "interface_declaration" => {
                if is_public(&child, source)
                    && let Some(name) = child_text_by_field(&child, source, "name")
                {
                    types.push(format!("interface {name} {{ ... }}"));
                }
            }
            "struct_declaration" => {
                if is_public(&child, source)
                    && let Some(name) = child_text_by_field(&child, source, "name")
                {
                    types.push(format!("struct {name}"));
                }
            }
            "enum_declaration" => {
                if is_public(&child, source)
                    && let Some(def) = extract_enum_def(&child, source)
                {
                    types.push(def);
                }
            }
            "record_declaration" => {
                if is_public(&child, source)
                    && let Some(name) = child_text_by_field(&child, source, "name")
                {
                    types.push(format!("record {name}"));
                }
            }
            "namespace_declaration" => {
                // Recurse into namespace
                extract_from_node(&child, source, functions, types, imports);
            }
            "using_directive" => {
                imports.push(node_text(&child, source));
            }
            "method_declaration" => {
                if is_public(&child, source)
                    && let Some(sig) = extract_method_sig(&child, source)
                {
                    functions.push(sig);
                }
            }
            "constructor_declaration" => {
                if is_public(&child, source)
                    && let Some(name) = child_text_by_field(&child, source, "name")
                {
                    let mut sig = format!("{name}(");
                    if let Some(params) = child.child_by_field_name("parameters") {
                        sig.push_str(&node_text(&params, source));
                    }
                    sig.push(')');
                    functions.push(sig);
                }
            }
            _ => {
                // Recurse for nested declarations
                extract_from_node(&child, source, functions, types, imports);
            }
        }
    }
}

fn is_public(node: &tree_sitter::Node, source: &str) -> bool {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i as u32)
            && child.kind() == "modifier"
        {
            let text = node_text(&child, source);
            if text == "public" {
                return true;
            }
        }
    }
    false
}

fn extract_class_def(node: &tree_sitter::Node, source: &str, keyword: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;
    let mut sig = format!("{keyword} {name}");

    if let Some(type_params) = node.child_by_field_name("type_parameters") {
        sig.push_str(&node_text(&type_params, source));
    }

    // Check for base list
    if let Some(base_list) = find_child_by_kind(node, "base_list") {
        sig.push_str(&format!(" : {}", node_text(&base_list, source)));
    }

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

    Some(format!("enum {name}"))
}

fn collect_enum_variants(body: &tree_sitter::Node, source: &str) -> Vec<String> {
    let mut variants = Vec::new();
    for i in 0..body.named_child_count() {
        let child = body.named_child(i as u32).unwrap();
        if child.kind() == "enum_member_declaration"
            && let Some(name) = child_text_by_field(&child, source, "name")
        {
            variants.push(name);
        }
    }
    variants
}

fn extract_method_sig(node: &tree_sitter::Node, source: &str) -> Option<String> {
    let name = child_text_by_field(node, source, "name")?;

    let mut sig = String::new();

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

fn extract_class_members(
    class_node: &tree_sitter::Node,
    source: &str,
    functions: &mut Vec<String>,
) {
    if let Some(body) = class_node.child_by_field_name("body") {
        for i in 0..body.named_child_count() {
            let child = body.named_child(i as u32).unwrap();
            match child.kind() {
                "method_declaration" => {
                    if is_public(&child, source)
                        && let Some(sig) = extract_method_sig(&child, source)
                    {
                        functions.push(sig);
                    }
                }
                "constructor_declaration" => {
                    if is_public(&child, source)
                        && let Some(name) = child_text_by_field(&child, source, "name")
                    {
                        let mut sig = format!("{name}(");
                        if let Some(params) = child.child_by_field_name("parameters") {
                            sig.push_str(&node_text(&params, source));
                        }
                        sig.push(')');
                        functions.push(sig);
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
        CSharpParser.parse_file(source, module_path, &mut parser)
    }

    #[test]
    fn test_module_path() {
        let base = Path::new("/project/Services");
        assert_eq!(
            CSharpParser
                .module_path_for_file(base, Path::new("/project/Services/SearchService.cs")),
            "SearchService"
        );
    }

    #[test]
    fn test_extracts_public_classes() {
        let source = r#"
using System;
using System.Collections.Generic;

namespace MyApp.Services
{
    public class SearchService : ISearchService
    {
        public List<Result> Search(string query)
        {
            return new List<Result>();
        }

        private void InternalHelper()
        {
        }

        public SearchService(string config)
        {
        }
    }

    public interface ISearchService
    {
        List<Result> Search(string query);
    }

    public enum Status
    {
        Active,
        Inactive
    }

    class PackagePrivateClass
    {
        public void Method() {}
    }
}
"#;
        let info = parse(source, "test").unwrap();
        assert!(info.types.iter().any(|t| t.contains("class SearchService")));
        assert!(
            info.types
                .iter()
                .any(|t| t.contains("interface ISearchService"))
        );
        assert!(info.types.iter().any(|t| t.contains("enum Status")));
        assert!(!info.types.iter().any(|t| t.contains("PackagePrivateClass")));
        assert!(info.functions.iter().any(|f| f.contains("Search")));
        assert!(info.functions.iter().any(|f| f.contains("SearchService(")));
        assert!(!info.functions.iter().any(|f| f.contains("InternalHelper")));
    }

    #[test]
    fn test_language_name() {
        assert_eq!(CSharpParser.language_name(), "C#");
        assert_eq!(CSharpParser.file_extensions(), &["cs"]);
    }
}
