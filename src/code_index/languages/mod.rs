//! Language parser trait and shared AST helpers.

use std::path::Path;

use tree_sitter::{Node, Parser};

use crate::code_index::ModuleInfo;

// ── Trait ────────────────────────────────────────────────────────────

/// Trait for language-specific AST parsing.
///
/// Each implementation is behind a feature flag (`lang-{name}`) and
/// registered in [`LanguageRegistry`](super::registry::LanguageRegistry).
pub trait LanguageParser: Send + Sync {
    /// Human-readable name (e.g. "Rust", "TypeScript").
    fn language_name(&self) -> &str;

    /// File extensions this parser handles, without the leading dot.
    fn file_extensions(&self) -> &[&str];

    /// Convert a file path to a language-appropriate module path.
    fn module_path_for_file(&self, base: &Path, file: &Path) -> String;

    /// Parse source code and extract module info.
    fn parse_file(
        &self,
        source: &str,
        module_path: &str,
        parser: &mut Parser,
    ) -> Option<ModuleInfo>;
}

// ── Shared AST helpers ───────────────────────────────────────────────

/// Get the text of a node from the source string.
pub fn node_text(node: &Node, source: &str) -> String {
    source[node.byte_range()].to_string()
}

/// Find a child node by its kind string.
pub fn find_child_by_kind<'a>(node: &Node<'a>, kind: &str) -> Option<Node<'a>> {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i as u32)
            && child.kind() == kind
        {
            return Some(child);
        }
    }
    None
}

/// Find a child node's text by kind.
pub fn find_child_text_by_kind(node: &Node, source: &str, kind: &str) -> Option<String> {
    find_child_by_kind(node, kind).map(|n| node_text(&n, source))
}

/// Check if a node has a direct child of a given kind.
pub fn has_child_kind(node: &Node, kind: &str) -> bool {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i as u32)
            && child.kind() == kind
        {
            return true;
        }
    }
    false
}

/// Extract text of a node's child by field name.
pub fn child_text_by_field(node: &Node, source: &str, field: &str) -> Option<String> {
    node.child_by_field_name(field)
        .map(|n| node_text(&n, source))
}

// ── Re-exports of enabled language parsers ───────────────────────────

#[cfg(feature = "lang-rust")]
mod rust;
#[cfg(feature = "lang-rust")]
pub use rust::RustParser;

#[cfg(feature = "lang-python")]
mod python;
#[cfg(feature = "lang-python")]
pub use python::PythonParser;

#[cfg(feature = "lang-typescript")]
mod typescript;
#[cfg(feature = "lang-typescript")]
pub use typescript::TypescriptParser;

#[cfg(feature = "lang-javascript")]
mod javascript;
#[cfg(feature = "lang-javascript")]
pub use javascript::JavascriptParser;

#[cfg(feature = "lang-go")]
mod go;
#[cfg(feature = "lang-go")]
pub use go::GoParser;

#[cfg(feature = "lang-java")]
mod java;
#[cfg(feature = "lang-java")]
pub use java::JavaParser;

#[cfg(feature = "lang-kotlin")]
mod kotlin;
#[cfg(feature = "lang-kotlin")]
pub use kotlin::KotlinParser;

#[cfg(feature = "lang-c")]
mod c;
#[cfg(feature = "lang-c")]
pub use c::CParser;

#[cfg(feature = "lang-cpp")]
mod cpp;
#[cfg(feature = "lang-cpp")]
pub use cpp::CppParser;

#[cfg(feature = "lang-swift")]
mod swift;
#[cfg(feature = "lang-swift")]
pub use swift::SwiftParser;

#[cfg(feature = "lang-ruby")]
mod ruby;
#[cfg(feature = "lang-ruby")]
pub use ruby::RubyParser;

#[cfg(feature = "lang-csharp")]
mod csharp;
#[cfg(feature = "lang-csharp")]
pub use csharp::CSharpParser;
