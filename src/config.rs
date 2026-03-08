use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

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

#[derive(Debug, Deserialize, Clone)]
pub struct CategoryConfig {
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DiagramConfig {
    #[serde(default = "default_diagram_format")]
    pub format: String,
}

fn default_diagram_format() -> String {
    "mermaid".into()
}

#[derive(Debug, Deserialize, Clone)]
pub struct DocConfig {
    #[serde(default)]
    pub docs_root: Option<String>,
    #[serde(default)]
    pub core: Option<CategoryConfig>,
    #[serde(default)]
    pub team: Option<CategoryConfig>,
    #[serde(default)]
    pub public: Option<CategoryConfig>,
    #[serde(default)]
    pub diagram: Option<DiagramConfig>,
}

impl DocConfig {
    pub fn core_files(&self) -> Vec<String> {
        self.core.as_ref().map_or_else(
            || DOC_REPO_REQUIRED.iter().map(|s| s.to_string()).collect(),
            |c| c.files.clone(),
        )
    }

    pub fn team_files(&self) -> Vec<String> {
        self.team.as_ref().map_or_else(
            || {
                DOC_REPO_SUPPLEMENTARY
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            },
            |c| c.files.clone(),
        )
    }

    pub fn public_files(&self) -> Vec<String> {
        self.public.as_ref().map_or_else(
            || PROJECT_REPO_FILES.iter().map(|s| s.to_string()).collect(),
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
        }
    })
}

pub fn classify_tier(relative_path: &str) -> &'static str {
    let filename = Path::new(relative_path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");
    let lower = filename.to_lowercase();

    let cfg = load_config();

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
        let cfg = DocConfig {
            docs_root: None,
            core: None,
            team: None,
            public: None,
            diagram: None,
        };
        assert_eq!(cfg.core_files().len(), DOC_REPO_REQUIRED.len());
        assert_eq!(cfg.team_files().len(), DOC_REPO_SUPPLEMENTARY.len());
        assert_eq!(cfg.public_files().len(), PROJECT_REPO_FILES.len());
        assert_eq!(cfg.diagram_format(), "mermaid");
    }

    #[test]
    fn config_custom_core_files() {
        let cfg = DocConfig {
            docs_root: None,
            core: Some(CategoryConfig {
                files: vec!["CUSTOM.md".into(), "OTHER.md".into()],
            }),
            team: None,
            public: None,
            diagram: None,
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
            core: None,
            team: None,
            public: None,
            diagram: None,
        };
        assert_eq!(cfg.docs_root(), Some(PathBuf::from("/tmp/explicit")));
    }

    #[test]
    fn docs_root_returns_none_when_no_config_and_no_default_dir() {
        let cfg = DocConfig {
            docs_root: None,
            core: None,
            team: None,
            public: None,
            diagram: None,
        };
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
}
