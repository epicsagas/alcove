//! Language registry — maps file extensions to language parsers.

use std::collections::HashMap;

use super::languages::LanguageParser;

/// Registry of enabled language parsers.
pub struct LanguageRegistry {
    parsers: Vec<Box<dyn LanguageParser>>,
    extension_map: HashMap<String, usize>,
}

impl LanguageRegistry {
    pub fn new() -> Self {
        Self {
            parsers: Vec::new(),
            extension_map: HashMap::new(),
        }
    }

    pub fn register(&mut self, parser: impl LanguageParser + 'static) {
        let idx = self.parsers.len();
        for ext in parser.file_extensions() {
            self.extension_map.insert(format!(".{ext}"), idx);
        }
        self.parsers.push(Box::new(parser));
    }

    pub fn parser_for_extension(&self, ext: &str) -> Option<&dyn LanguageParser> {
        let key = format!(".{ext}");
        self.extension_map
            .get(&key)
            .map(|&idx| self.parsers[idx].as_ref())
    }

    pub fn parser_for_name(&self, name: &str) -> Option<&dyn LanguageParser> {
        let lower = name.to_lowercase();
        self.parsers
            .iter()
            .find(|p| {
                let lang = p.language_name().to_lowercase();
                lang == lower || is_alias(&lang, &lower)
            })
            .map(|p| p.as_ref())
    }

    #[allow(dead_code)] // Phase 2+ will use this
    pub fn all_parsers(&self) -> &[Box<dyn LanguageParser>] {
        &self.parsers
    }

    pub fn is_empty(&self) -> bool {
        self.parsers.is_empty()
    }

    /// Names of all registered languages.
    #[allow(dead_code)] // Phase 2+ will use this
    pub fn language_names(&self) -> Vec<&str> {
        self.parsers.iter().map(|p| p.language_name()).collect()
    }
}

/// Check common aliases: cpp↔c++, csharp↔c#, typescript↔ts, javascript↔js, etc.
fn is_alias(lang: &str, query: &str) -> bool {
    matches!(
        (lang, query),
        ("c++", "cpp") | ("cpp", "c++")
            | ("c#", "csharp") | ("csharp", "c#")
            | ("typescript", "ts") | ("ts", "typescript")
            | ("javascript", "js") | ("js", "javascript")
            | ("python", "py") | ("py", "python")
            | ("kotlin", "kt") | ("kt", "kotlin")
            | ("ruby", "rb") | ("rb", "ruby")
    )
}

impl Default for LanguageRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the default registry from compile-time enabled features.
pub fn default_registry() -> LanguageRegistry {
    let mut reg = LanguageRegistry::new();

    #[cfg(feature = "lang-rust")]
    reg.register(crate::code_index::languages::RustParser);

    #[cfg(feature = "lang-python")]
    reg.register(crate::code_index::languages::PythonParser);

    #[cfg(feature = "lang-typescript")]
    reg.register(crate::code_index::languages::TypescriptParser);

    #[cfg(feature = "lang-javascript")]
    reg.register(crate::code_index::languages::JavascriptParser);

    #[cfg(feature = "lang-go")]
    reg.register(crate::code_index::languages::GoParser);

    #[cfg(feature = "lang-java")]
    reg.register(crate::code_index::languages::JavaParser);

    #[cfg(feature = "lang-kotlin")]
    reg.register(crate::code_index::languages::KotlinParser);

    #[cfg(feature = "lang-c")]
    reg.register(crate::code_index::languages::CParser);

    #[cfg(feature = "lang-cpp")]
    reg.register(crate::code_index::languages::CppParser);

    #[cfg(feature = "lang-swift")]
    reg.register(crate::code_index::languages::SwiftParser);

    #[cfg(feature = "lang-ruby")]
    reg.register(crate::code_index::languages::RubyParser);

    #[cfg(feature = "lang-csharp")]
    reg.register(crate::code_index::languages::CSharpParser);

    reg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_empty() {
        let reg = LanguageRegistry::new();
        assert!(reg.is_empty());
        assert!(reg.parser_for_extension("rs").is_none());
    }

    #[test]
    #[cfg(feature = "lang-rust")]
    fn test_default_registry_has_rust() {
        let reg = default_registry();
        assert!(!reg.is_empty());
        assert!(reg.parser_for_extension("rs").is_some());
        assert!(reg.parser_for_name("rust").is_some());
        assert!(reg.parser_for_name("Rust").is_some());
        assert!(reg.language_names().contains(&"Rust"));
    }
}
