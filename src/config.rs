use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Document tier classification constants
// ---------------------------------------------------------------------------

/// Doc-repo required docs — core project docs stored in alcove.
pub const DOC_REPO_REQUIRED: &[&str] = &[
    "PRD.md",
    "ARCHITECTURE.md",
    "PROGRESS.md",
    "DECISIONS.md",
    "CONVENTIONS.md",
    "SECRETS_MAP.md",
    "DEBT.md",
];

/// Doc-repo supplementary docs — recognized extras in alcove.
/// Dev-team-oriented docs that don't need public exposure.
pub const DOC_REPO_SUPPLEMENTARY: &[&str] = &[
    // Dev environment & onboarding
    "ENV_SETUP.md",
    "ONBOARDING.md",
    // Data model & specs
    "DATA_MODEL.md",
    "SCHEMA.md",
    // Operations (internal runbooks)
    "DEPLOYMENT.md",
    "RUNBOOK.md",
    "PLAYBOOK.md",
    "MONITORING.md",
    "INFRASTRUCTURE.md",
    "RELEASE.md",
    "RELEASE_PROCESS.md",
    "MIGRATION.md",
    "UPGRADING.md",
    // Quality & testing (internal)
    "TESTING.md",
    "BENCHMARK.md",
    "PERFORMANCE.md",
    "STYLE_GUIDE.md",
    // Internal reference
    "GLOSSARY.md",
    "TROUBLESHOOTING.md",
];

/// Project-repo docs — typically found in the project repository (GitHub).
/// Used to classify files when scanning the project repo, NOT to suggest
/// moving alcove files outward.
pub const PROJECT_REPO_FILES: &[&str] = &[
    // GitHub community health files
    "README.md",
    "CHANGELOG.md",
    "CONTRIBUTING.md",
    "SECURITY.md",
    "CODE_OF_CONDUCT.md",
    "LICENSE.md",
    "SUPPORT.md",
    "AUTHORS.md",
    "CONTRIBUTORS.md",
    "CODEOWNERS",
    // User-facing guides
    "QUICKSTART.md",
    "INSTALL.md",
    "API.md",
    "FAQ.md",
];

// ---------------------------------------------------------------------------
// Dynamic config from ~/.config/alcove/config.toml
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CategoryConfig {
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DiagramConfig {
    #[serde(default = "default_diagram_format")]
    pub format: String,
}

fn default_diagram_format() -> String {
    "mermaid".into()
}

fn default_index_buffer_mb() -> usize {
    15
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct IndexConfig {
    #[serde(default = "default_index_buffer_mb")]
    pub buffer_size_mb: usize,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            buffer_size_mb: default_index_buffer_mb(),
        }
    }
}

// ---------------------------------------------------------------------------
// Embedding config (alcove-full feature)
// ---------------------------------------------------------------------------

#[cfg(feature = "alcove-full")]
fn default_embedding_model() -> String {
    "MultilingualE5Small".into()
}

fn default_embedding_enabled() -> bool {
    #[cfg(feature = "alcove-full")]
    {
        true
    }
    #[cfg(not(feature = "alcove-full"))]
    {
        false
    }
}

#[cfg(feature = "alcove-full")]
fn default_embedding_auto_download() -> bool {
    true
}

#[cfg(feature = "alcove-full")]
fn default_embedding_cache_dir() -> String {
    dirs::cache_dir()
        .map(|p| p.join("alcove").join("models").to_string_lossy().to_string())
        .unwrap_or_else(|| "~/.cache/alcove/models".into())
}

#[cfg(feature = "alcove-full")]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EmbeddingConfig {
    /// Model name (e.g., "MultilingualE5Small", "SnowflakeArcticEmbedXS")
    #[serde(default = "default_embedding_model")]
    pub model: String,
    /// Auto-download model on first search
    #[serde(default = "default_embedding_auto_download")]
    pub auto_download: bool,
    /// Model cache directory
    #[serde(default = "default_embedding_cache_dir")]
    pub cache_dir: String,
    /// Enable embedding (false = BM25 only)
    #[serde(default = "default_embedding_enabled")]
    pub enabled: bool,
}

#[cfg(feature = "alcove-full")]
impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: default_embedding_model(),
            auto_download: default_embedding_auto_download(),
            cache_dir: default_embedding_cache_dir(),
            enabled: default_embedding_enabled(),
        }
    }
}

#[cfg(feature = "alcove-full")]
impl EmbeddingConfig {
    /// Get the model name as a string
    pub fn model_name(&self) -> &str {
        &self.model
    }
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct DocConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core: Option<CategoryConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<CategoryConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public: Option<CategoryConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagram: Option<DiagramConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<IndexConfig>,
    /// Additional file extensions to include in the search index.
    /// Example: `extra_extensions = ["yaml", "json"]`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_extensions: Option<Vec<String>>,
    /// Embedding configuration for hybrid search (alcove-full feature)
    #[cfg(feature = "alcove-full")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<EmbeddingConfig>,
}


impl DocConfig {
    /// Return a new config where `self` values take precedence and `base` fills
    /// any unset fields.  Used to layer project-level config over global config.
    pub fn overlay(&self, base: &DocConfig) -> DocConfig {
        DocConfig {
            docs_root: self.docs_root.clone().or_else(|| base.docs_root.clone()),
            core: self.core.clone().or_else(|| base.core.clone()),
            team: self.team.clone().or_else(|| base.team.clone()),
            public: self.public.clone().or_else(|| base.public.clone()),
            diagram: self.diagram.clone().or_else(|| base.diagram.clone()),
            index: self.index.clone().or_else(|| base.index.clone()),
            extra_extensions: self.extra_extensions.clone().or_else(|| base.extra_extensions.clone()),
            #[cfg(feature = "alcove-full")]
            embedding: self.embedding.clone().or_else(|| base.embedding.clone()),
        }
    }

    pub fn core_files(&self) -> Vec<String> {
        self.core.as_ref().map_or_else(
            || DOC_REPO_REQUIRED.iter().map(std::string::ToString::to_string).collect(),
            |c| c.files.clone(),
        )
    }

    pub fn team_files(&self) -> Vec<String> {
        self.team.as_ref().map_or_else(
            || {
                DOC_REPO_SUPPLEMENTARY
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect()
            },
            |c| c.files.clone(),
        )
    }

    pub fn public_files(&self) -> Vec<String> {
        self.public.as_ref().map_or_else(
            || PROJECT_REPO_FILES.iter().map(std::string::ToString::to_string).collect(),
            |c| c.files.clone(),
        )
    }

    pub fn diagram_format(&self) -> String {
        self.diagram
            .as_ref()
            .map_or_else(default_diagram_format, |d| d.format.clone())
    }

    pub fn docs_root(&self) -> Option<PathBuf> {
        // 1. Explicit config value
        if let Some(ref root) = self.docs_root {
            return Some(PathBuf::from(root));
        }
        // 2. Fall back to default: ~/.config/alcove/docs
        let fallback = default_docs_root();
        if fallback.is_dir() {
            return Some(fallback);
        }
        None
    }

    pub fn index_buffer_bytes(&self) -> usize {
        self.index
            .as_ref()
            .map_or_else(default_index_buffer_mb, |i| i.buffer_size_mb)
            * 1_000_000
    }

    /// Returns true if the file should be included in the search index.
    /// Extends `is_doc_file` with any user-configured extra extensions.
    ///
    /// Extensions are matched case-insensitively and may optionally include a
    /// leading dot (e.g. `"yaml"` and `".yaml"` both match `.yaml` files).
    pub fn is_indexable(&self, path: &Path) -> bool {
        if is_doc_file(path) {
            return true;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        self.extra_extensions
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .any(|e| e.trim_start_matches('.').to_lowercase() == ext)
    }

    /// Get embedding configuration as TOML struct
    #[cfg(feature = "alcove-full")]
    pub fn embedding_config(&self) -> &Option<EmbeddingConfig> {
        &self.embedding
    }

    /// Get embedding configuration with defaults applied
    #[cfg(feature = "alcove-full")]
    pub fn embedding_config_with_defaults(&self) -> EmbeddingConfig {
        self.embedding.clone().unwrap_or_default()
    }
}

/// Default docs root: `~/.config/alcove/docs`
pub fn default_docs_root() -> PathBuf {
    if let Ok(home) = env::var("HOME") {
        PathBuf::from(home).join(".config/alcove/docs")
    } else {
        PathBuf::from("/nonexistent")
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(home) = env::var("HOME") {
        PathBuf::from(home).join(".config/alcove/config.toml")
    } else {
        PathBuf::from("/nonexistent")
    }
}

pub fn load_config() -> &'static DocConfig {
    static CONFIG: OnceLock<DocConfig> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let path = config_path();
        if path.exists()
            && let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(cfg) = toml::from_str::<DocConfig>(&content)
        {
            return cfg;
        }
        DocConfig {
            docs_root: None,
            core: None,
            team: None,
            public: None,
            diagram: None,
            index: None,
            extra_extensions: None,
            #[cfg(feature = "alcove-full")]
            embedding: None,
        }
    })
}

/// Classify a doc file path using a provided config (enables project-level overrides).
pub fn classify_tier_with(relative_path: &str, cfg: &DocConfig) -> &'static str {
    let filename = Path::new(relative_path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");
    let lower = filename.to_lowercase();

    if cfg.core_files().iter().any(|f| f.to_lowercase() == lower) {
        "doc-repo-required"
    } else if relative_path.starts_with("reports/") || relative_path.starts_with("reports\\") {
        "reference"
    } else if cfg.team_files().iter().any(|f| f.to_lowercase() == lower) {
        "doc-repo-supplementary"
    } else if cfg.public_files().iter().any(|f| f.to_lowercase() == lower) {
        "project-repo"
    } else {
        "unrecognized"
    }
}

/// Classify using the global config. Convenience wrapper around [`classify_tier_with`].
#[cfg(test)]
pub fn classify_tier(relative_path: &str) -> &'static str {
    classify_tier_with(relative_path, load_config())
}

// ---------------------------------------------------------------------------
// Project-level config: {project_repo}/alcove.toml
// ---------------------------------------------------------------------------

/// Path to the per-project config file inside the project repository.
pub fn project_config_path(project_repo: &Path) -> PathBuf {
    project_repo.join("alcove.toml")
}

/// Load per-project config from `{project_repo}/alcove.toml`.
/// Returns an empty `DocConfig` if the file doesn't exist or cannot be parsed.
pub fn load_project_config(project_repo: &Path) -> DocConfig {
    let path = project_config_path(project_repo);
    if path.exists()
        && let Ok(content) = std::fs::read_to_string(&path)
        && let Ok(cfg) = toml::from_str::<DocConfig>(&content)
    {
        return cfg;
    }
    DocConfig::default()
}

/// Return the effective config for a project.
///
/// Resolution order (highest priority first):
/// 1. `{project_repo}/alcove.toml`  (project repository root)
/// 2. `~/.config/alcove/config.toml` (global)
/// 3. Built-in defaults
pub fn effective_config(project_repo: &Path) -> DocConfig {
    load_project_config(project_repo).overlay(load_config())
}

/// Categorization hint for unrecognized files in alcove.
pub fn suggest_categorization(filename: &str) -> &'static str {
    let lower = filename.to_lowercase();

    if lower.contains("product")
        || lower.contains("requirement")
        || lower.contains("spec")
        || lower.contains("summary")
    {
        return "Related to PRD.md";
    }
    if lower.contains("design")
        || lower.contains("orchestration")
        || lower.contains("implementation")
    {
        return "Related to ARCHITECTURE.md";
    }
    if lower.contains("plan") || lower.contains("roadmap") || lower.contains("todo") {
        return "Related to PROGRESS.md";
    }
    if lower.contains("feasibility") || lower.contains("adr") || lower.contains("decision") {
        return "Related to DECISIONS.md";
    }
    if lower.contains("coding_standard") || lower.contains("code_style") {
        return "Related to CONVENTIONS.md";
    }
    if lower.contains("tech_debt") || lower.contains("technical_debt") {
        return "Related to DEBT.md";
    }
    if lower.contains("env_var") || lower.contains("secrets") {
        return "Related to SECRETS_MAP.md";
    }
    if lower.contains("audit")
        || lower.contains("benchmark")
        || lower.contains("analysis")
        || lower.contains("competitive")
        || lower.contains("comprehensive")
        || lower.contains("session")
        || lower.contains("report")
    {
        return "Candidate for reports/ folder";
    }

    "Uncategorized — ask user"
}

/// Check if a file is a documentation file.
pub fn is_doc_file(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "md" | "txt" | "rst" => true,
        "yml" | "yaml" | "json" => {
            let filename = path
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("")
                .to_lowercase();
            filename.starts_with("openapi") || filename.starts_with("swagger")
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_core_files() {
        for name in DOC_REPO_REQUIRED {
            assert_eq!(
                classify_tier(name),
                "doc-repo-required",
                "failed for {name}"
            );
        }
    }

    #[test]
    fn classify_core_case_insensitive() {
        assert_eq!(classify_tier("prd.md"), "doc-repo-required");
        assert_eq!(classify_tier("architecture.md"), "doc-repo-required");
    }

    #[test]
    fn classify_supplementary_files() {
        for name in DOC_REPO_SUPPLEMENTARY {
            assert_eq!(
                classify_tier(name),
                "doc-repo-supplementary",
                "failed for {name}"
            );
        }
    }

    #[test]
    fn classify_project_repo_files() {
        let always_public = ["README.md", "CHANGELOG.md"];
        for name in always_public {
            assert_eq!(classify_tier(name), "project-repo", "failed for {name}");
        }
    }

    #[test]
    fn classify_reports_folder() {
        assert_eq!(classify_tier("reports/weekly.md"), "reference");
        assert_eq!(classify_tier("reports/audit-2026.md"), "reference");
    }

    #[test]
    fn classify_unknown_file() {
        assert_eq!(classify_tier("random-notes.md"), "unrecognized");
        assert_eq!(classify_tier("foo.md"), "unrecognized");
    }

    #[test]
    fn suggest_prd_related() {
        assert_eq!(
            suggest_categorization("product_overview.md"),
            "Related to PRD.md"
        );
        assert_eq!(
            suggest_categorization("requirements.md"),
            "Related to PRD.md"
        );
        assert_eq!(suggest_categorization("SPEC_v2.md"), "Related to PRD.md");
    }

    #[test]
    fn suggest_architecture_related() {
        assert_eq!(
            suggest_categorization("design_doc.md"),
            "Related to ARCHITECTURE.md"
        );
        assert_eq!(
            suggest_categorization("implementation_guide.md"),
            "Related to ARCHITECTURE.md"
        );
    }

    #[test]
    fn suggest_progress_related() {
        assert_eq!(
            suggest_categorization("roadmap.md"),
            "Related to PROGRESS.md"
        );
        assert_eq!(
            suggest_categorization("plan_q3.md"),
            "Related to PROGRESS.md"
        );
    }

    #[test]
    fn suggest_decisions_related() {
        assert_eq!(
            suggest_categorization("ADR-001.md"),
            "Related to DECISIONS.md"
        );
        assert_eq!(
            suggest_categorization("feasibility_study.md"),
            "Related to DECISIONS.md"
        );
    }

    #[test]
    fn suggest_reports_folder() {
        assert_eq!(
            suggest_categorization("benchmark_results.md"),
            "Candidate for reports/ folder"
        );
        assert_eq!(
            suggest_categorization("audit_2026.md"),
            "Candidate for reports/ folder"
        );
        assert_eq!(
            suggest_categorization("analysis_report.md"),
            "Candidate for reports/ folder"
        );
    }

    #[test]
    fn suggest_uncategorized() {
        assert_eq!(
            suggest_categorization("random.md"),
            "Uncategorized — ask user"
        );
    }

    #[test]
    fn doc_file_markdown() {
        assert!(is_doc_file(Path::new("README.md")));
        assert!(is_doc_file(Path::new("docs/PRD.md")));
    }

    #[test]
    fn doc_file_txt_rst() {
        assert!(is_doc_file(Path::new("notes.txt")));
        assert!(is_doc_file(Path::new("guide.rst")));
    }

    #[test]
    fn doc_file_openapi() {
        assert!(is_doc_file(Path::new("openapi.yaml")));
        assert!(is_doc_file(Path::new("swagger.json")));
        assert!(is_doc_file(Path::new("OPENAPI_v3.yml")));
    }

    #[test]
    fn doc_file_rejects_non_docs() {
        assert!(!is_doc_file(Path::new("main.rs")));
        assert!(!is_doc_file(Path::new("config.toml")));
        assert!(!is_doc_file(Path::new("data.json")));
        assert!(!is_doc_file(Path::new("style.css")));
    }

    #[test]
    fn config_defaults_when_empty() {
        let cfg = DocConfig::default();
        assert_eq!(cfg.core_files().len(), DOC_REPO_REQUIRED.len());
        assert_eq!(cfg.team_files().len(), DOC_REPO_SUPPLEMENTARY.len());
        assert_eq!(cfg.public_files().len(), PROJECT_REPO_FILES.len());
        assert_eq!(cfg.diagram_format(), "mermaid");
    }

    #[test]
    fn config_custom_core_files() {
        let cfg = DocConfig {
            core: Some(CategoryConfig {
                files: vec!["CUSTOM.md".into(), "OTHER.md".into()],
            }),
            ..DocConfig::default()
        };
        assert_eq!(cfg.core_files(), vec!["CUSTOM.md", "OTHER.md"]);
        assert_eq!(cfg.team_files().len(), DOC_REPO_SUPPLEMENTARY.len());
    }

    #[test]
    fn default_docs_root_contains_alcove_docs() {
        let path = default_docs_root();
        assert!(path.to_string_lossy().ends_with(".config/alcove/docs"));
    }

    #[test]
    fn docs_root_returns_explicit_value() {
        let cfg = DocConfig {
            docs_root: Some("/tmp/explicit".into()),
            ..DocConfig::default()
        };
        assert_eq!(cfg.docs_root(), Some(PathBuf::from("/tmp/explicit")));
    }

    #[test]
    fn docs_root_returns_none_when_no_config_and_no_default_dir() {
        let cfg = DocConfig::default();
        // If ~/.config/alcove/docs doesn't exist, returns None
        // (it may or may not exist on the test machine, so just verify it's a valid Option)
        let result = cfg.docs_root();
        if let Some(ref p) = result {
            assert!(p.is_dir());
        }
    }

    #[test]
    fn classify_reports_backslash() {
        assert_eq!(classify_tier("reports\\weekly.md"), "reference");
    }

    #[test]
    fn suggest_conventions_related() {
        assert_eq!(
            suggest_categorization("coding_standard.md"),
            "Related to CONVENTIONS.md"
        );
        assert_eq!(
            suggest_categorization("code_style_guide.md"),
            "Related to CONVENTIONS.md"
        );
    }

    #[test]
    fn suggest_debt_related() {
        assert_eq!(
            suggest_categorization("tech_debt_tracker.md"),
            "Related to DEBT.md"
        );
        assert_eq!(
            suggest_categorization("technical_debt_backlog.md"),
            "Related to DEBT.md"
        );
    }

    #[test]
    fn suggest_secrets_related() {
        assert_eq!(
            suggest_categorization("env_vars_list.md"),
            "Related to SECRETS_MAP.md"
        );
        assert_eq!(
            suggest_categorization("secrets_rotation.md"),
            "Related to SECRETS_MAP.md"
        );
    }

    #[test]
    fn is_doc_file_no_extension() {
        assert!(!is_doc_file(Path::new("Makefile")));
        assert!(!is_doc_file(Path::new("LICENSE")));
    }

    #[test]
    fn config_parse_toml() {
        let toml_str = r#"
            docs_root = "/tmp/docs"
            [core]
            files = ["A.md", "B.md"]
            [diagram]
            format = "ascii"
        "#;
        let cfg: DocConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.docs_root, Some("/tmp/docs".into()));
        assert_eq!(cfg.core_files(), vec!["A.md", "B.md"]);
        assert_eq!(cfg.diagram_format(), "ascii");
    }

    // -- project-level config --

    #[test]
    fn overlay_project_overrides_base() {
        let base = DocConfig {
            diagram: Some(DiagramConfig { format: "mermaid".into() }),
            core: Some(CategoryConfig { files: vec!["BASE.md".into()] }),
            ..DocConfig::default()
        };
        let project = DocConfig {
            diagram: Some(DiagramConfig { format: "plantuml".into() }),
            ..DocConfig::default()
        };
        let merged = project.overlay(&base);
        // project value wins
        assert_eq!(merged.diagram_format(), "plantuml");
        // base value fills unset field
        assert_eq!(merged.core_files(), vec!["BASE.md"]);
    }

    #[test]
    fn overlay_empty_project_uses_base() {
        let base = DocConfig {
            core: Some(CategoryConfig { files: vec!["PRD.md".into()] }),
            diagram: Some(DiagramConfig { format: "ascii".into() }),
            ..DocConfig::default()
        };
        let merged = DocConfig::default().overlay(&base);
        assert_eq!(merged.core_files(), vec!["PRD.md"]);
        assert_eq!(merged.diagram_format(), "ascii");
    }

    #[test]
    fn classify_tier_with_custom_config() {
        let cfg = DocConfig {
            core: Some(CategoryConfig { files: vec!["CUSTOM.md".into()] }),
            ..DocConfig::default()
        };
        assert_eq!(classify_tier_with("CUSTOM.md", &cfg), "doc-repo-required");
        // A standard core file is NOT in custom list, so it's unrecognized
        assert_eq!(classify_tier_with("PRD.md", &cfg), "unrecognized");
    }

    #[test]
    fn load_project_config_missing_file_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        // No alcove.toml in tmp → returns defaults
        let cfg = load_project_config(tmp.path());
        assert_eq!(cfg.core_files().len(), DOC_REPO_REQUIRED.len());
    }

    #[test]
    fn load_project_config_reads_toml() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("alcove.toml"),
            r#"[diagram]
format = "plantuml"
[core]
files = ["SPEC.md", "DESIGN.md"]
"#,
        )
        .unwrap();

        let cfg = load_project_config(tmp.path());
        assert_eq!(cfg.diagram_format(), "plantuml");
        assert_eq!(cfg.core_files(), vec!["SPEC.md", "DESIGN.md"]);
    }

    #[test]
    fn effective_config_project_overrides_are_applied() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("alcove.toml"),
            r#"[diagram]
format = "plantuml"
"#,
        )
        .unwrap();

        let cfg = effective_config(tmp.path());
        assert_eq!(cfg.diagram_format(), "plantuml");
        assert_eq!(cfg.core_files().len(), DOC_REPO_REQUIRED.len());
    }

    #[test]
    fn project_config_path_is_in_project_dir() {
        let path = project_config_path(Path::new("/my/project"));
        assert_eq!(path, PathBuf::from("/my/project/alcove.toml"));
    }

    // -- is_indexable --

    #[test]
    fn is_indexable_falls_back_to_is_doc_file() {
        let cfg = DocConfig::default();
        assert!(cfg.is_indexable(Path::new("README.md")));
        assert!(cfg.is_indexable(Path::new("notes.txt")));
        assert!(!cfg.is_indexable(Path::new("main.rs")));
        assert!(!cfg.is_indexable(Path::new("config.toml")));
    }

    #[test]
    fn is_indexable_respects_extra_extensions() {
        let cfg = DocConfig {
            extra_extensions: Some(vec!["toml".into(), "yaml".into()]),
            ..DocConfig::default()
        };
        assert!(cfg.is_indexable(Path::new("config.toml")));
        assert!(cfg.is_indexable(Path::new("schema.yaml")));
        assert!(!cfg.is_indexable(Path::new("main.rs")));
    }

    #[test]
    fn is_indexable_handles_dot_prefixed_extensions() {
        let cfg = DocConfig {
            extra_extensions: Some(vec![".toml".into(), ".yaml".into()]),
            ..DocConfig::default()
        };
        assert!(cfg.is_indexable(Path::new("config.toml")));
        assert!(cfg.is_indexable(Path::new("schema.yaml")));
    }

    #[test]
    fn is_indexable_case_insensitive() {
        let cfg = DocConfig {
            extra_extensions: Some(vec!["TOML".into()]),
            ..DocConfig::default()
        };
        assert!(cfg.is_indexable(Path::new("config.toml")));
        assert!(cfg.is_indexable(Path::new("config.TOML")));
    }

    #[test]
    fn is_indexable_no_double_count_for_md() {
        // .md is already handled by is_doc_file; extra_extensions has no effect
        let cfg = DocConfig {
            extra_extensions: Some(vec!["md".into()]),
            ..DocConfig::default()
        };
        assert!(cfg.is_indexable(Path::new("README.md")));
    }

    #[test]
    fn extra_extensions_overlay_project_wins() {
        let base = DocConfig {
            extra_extensions: Some(vec!["toml".into()]),
            ..DocConfig::default()
        };
        let project = DocConfig {
            extra_extensions: Some(vec!["yaml".into()]),
            ..DocConfig::default()
        };
        let merged = project.overlay(&base);
        let exts = merged.extra_extensions.unwrap_or_default();
        assert_eq!(exts, vec!["yaml"]);
    }

    #[test]
    fn extra_extensions_overlay_base_fills_when_project_unset() {
        let base = DocConfig {
            extra_extensions: Some(vec!["toml".into()]),
            ..DocConfig::default()
        };
        let merged = DocConfig::default().overlay(&base);
        let exts = merged.extra_extensions.unwrap_or_default();
        assert_eq!(exts, vec!["toml"]);
    }

    #[test]
    fn extra_extensions_toml_deserialization() {
        let toml = r#"extra_extensions = ["yaml", "json"]"#;
        let cfg: DocConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.extra_extensions.unwrap_or_default(), vec!["yaml", "json"]);
    }
}
