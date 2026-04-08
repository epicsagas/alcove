use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::load_config;

// ---------------------------------------------------------------------------
// Policy schema (parsed from policy.toml)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
pub struct PolicyFile {
    pub policy: Policy,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Policy {
    #[serde(default = "default_enforce")]
    pub enforce: String,
    #[serde(default = "default_version")]
    #[allow(dead_code)] // Part of policy schema, used in tests and reserved for future use
    pub version: String,
    #[serde(default)]
    pub required: Vec<RequiredDoc>,
    #[serde(default)]
    pub naming: Option<NamingPolicy>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RequiredDoc {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub sections: Vec<RequiredSection>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RequiredSection {
    pub heading: String,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)]
    pub min_items: Option<usize>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct NamingPolicy {
    #[serde(default = "default_case")]
    pub case: String,
    #[serde(default = "default_extension")]
    pub extension: String,
    #[serde(default = "default_max_length")]
    pub max_length: usize,
}

fn default_enforce() -> String {
    "warn".into()
}
fn default_version() -> String {
    "1".into()
}
fn default_true() -> bool {
    true
}
fn default_case() -> String {
    "free".into()
}
fn default_extension() -> String {
    ".md".into()
}
fn default_max_length() -> usize {
    50
}

// ---------------------------------------------------------------------------
// Policy resolution
// ---------------------------------------------------------------------------

/// Load policy with priority: project-level > team-level > built-in default.
pub fn load_policy(docs_root: &Path, project_name: &str) -> PolicyFile {
    // 1. Project-level: DOCS_ROOT/<project>/.alcove/policy.toml
    let project_policy = docs_root.join(project_name).join(".alcove/policy.toml");
    if let Some(p) = try_load_policy(&project_policy) {
        return p;
    }

    // 2. Team-level: DOCS_ROOT/.alcove/policy.toml
    let team_policy = docs_root.join(".alcove/policy.toml");
    if let Some(p) = try_load_policy(&team_policy) {
        return p;
    }

    // 3. Built-in default from config
    default_policy()
}

fn try_load_policy(path: &Path) -> Option<PolicyFile> {
    let content = std::fs::read_to_string(path).ok()?;
    toml::from_str::<PolicyFile>(&content).ok()
}

/// Generate a default policy from the current alcove config.
fn default_policy() -> PolicyFile {
    let cfg = load_config();
    let required: Vec<RequiredDoc> = cfg
        .core_files()
        .into_iter()
        .map(|name| RequiredDoc {
            name,
            aliases: Vec::new(),
            description: None,
            location: None,
            sections: Vec::new(),
        })
        .collect();

    PolicyFile {
        policy: Policy {
            enforce: "warn".into(),
            version: "1".into(),
            required,
            naming: None,
        },
    }
}

/// Location of the resolved policy for display.
pub fn policy_source(docs_root: &Path, project_name: &str) -> &'static str {
    let project_policy = docs_root.join(project_name).join(".alcove/policy.toml");
    if project_policy.exists() {
        return "project";
    }
    let team_policy = docs_root.join(".alcove/policy.toml");
    if team_policy.exists() {
        return "team";
    }
    "default"
}

// ---------------------------------------------------------------------------
// Validation engine
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ValidationResult {
    pub file: String,
    pub status: FileStatus,
    pub sections: Vec<SectionResult>,
    pub reason: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FileStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug)]
pub struct SectionResult {
    pub heading: String,
    pub status: FileStatus,
    pub detail: Option<String>,
}

impl FileStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            FileStatus::Pass => "pass",
            FileStatus::Warn => "warn",
            FileStatus::Fail => "fail",
        }
    }
}

/// Get policy version information
#[cfg(test)]
pub fn policy_version(policy: &PolicyFile) -> &str {
    &policy.policy.version
}

/// Check if naming policy is configured
#[cfg(test)]
pub fn has_naming_policy(policy: &PolicyFile) -> bool {
    policy.policy.naming.is_some()
}

pub fn validate_doc_name(policy: &PolicyFile, doc_name: &str) -> Result<(), String> {
    use std::path::Path;
    
    if let Some(naming) = &policy.policy.naming {
        let path = Path::new(doc_name);
        let name_without_ext = path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(doc_name);
        
        // Check extension matches (use the extension field)
        let actual_ext = path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let expected_ext = naming.extension.trim_start_matches('.');
        if !actual_ext.is_empty() && actual_ext != expected_ext {
            return Err(format!(
                "Document must have .{} extension",
                naming.extension
            ));
        }
        
        // Check max length
        if name_without_ext.len() > naming.max_length {
            return Err(format!(
                "Document name exceeds maximum length of {} characters",
                naming.max_length
            ));
        }

        // Validate case (basic check)
        match naming.case.as_str() {
            "kebab-case" => {
                // Must be lowercase letters, numbers, hyphens, or underscores
                for c in name_without_ext.chars() {
                    if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-' && c != '_' {
                        return Err(String::from(
                            "Document name must be in kebab-case (lowercase letters, numbers, hyphens, underscores)"
                        ));
                    }
                }
            }
            "snake_case" => {
                // Must be lowercase letters, numbers, or underscores
                for c in name_without_ext.chars() {
                    if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '_' {
                        return Err(String::from(
                            "Document name must be in snake_case (lowercase letters, numbers, underscores)"
                        ));
                    }
                }
            }
            "free" => {}
            _ => {}
        }
    }

    Ok(())
}

/// Run validation against a policy.
pub fn validate(
    docs_root: &Path,
    project_name: &str,
    repo_path: Option<&Path>,
) -> (PolicyFile, Vec<ValidationResult>) {
    let policy = load_policy(docs_root, project_name);
    let project_root = docs_root.join(project_name);
    let mut results = Vec::new();

    for req in &policy.policy.required {
        // Validate document name against naming policy
        if let Err(e) = validate_doc_name(&policy, &req.name) {
            results.push(ValidationResult {
                file: req.name.clone(),
                status: FileStatus::Fail,
                sections: Vec::new(),
                reason: Some(e),
            });
            continue;
        }

        let result = validate_required_doc(req, &project_root, repo_path);
        results.push(result);
    }

    (policy, results)
}

fn validate_required_doc(
    req: &RequiredDoc,
    project_root: &Path,
    repo_path: Option<&Path>,
) -> ValidationResult {
    // Use description in validation (for informational purposes)
    let _description_ref = req.description.as_ref();

    let is_project_repo = req.location.as_deref() == Some("project-repo");

    // Find the file (check primary name + aliases)
    let found_path = find_doc_file(req, project_root, repo_path, is_project_repo);

    let Some(path) = found_path else {
        return ValidationResult {
            file: req.name.clone(),
            status: FileStatus::Fail,
            sections: Vec::new(),
            reason: Some("file_not_found".into()),
        };
    };

    // File exists — check size
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let has_placeholder = content.contains("ProjectName");

    if has_placeholder {
        return ValidationResult {
            file: req.name.clone(),
            status: FileStatus::Warn,
            sections: Vec::new(),
            reason: Some("template_unfilled".into()),
        };
    }

    if content.len() < 100 {
        return ValidationResult {
            file: req.name.clone(),
            status: FileStatus::Warn,
            sections: Vec::new(),
            reason: Some("minimal_content".into()),
        };
    }

    // Validate sections if defined
    let sections = validate_sections(&content, &req.sections);
    let has_section_fail = sections.iter().any(|s| s.status == FileStatus::Fail);
    let has_section_warn = sections.iter().any(|s| s.status == FileStatus::Warn);

    let status = if has_section_fail {
        FileStatus::Fail
    } else if has_section_warn {
        FileStatus::Warn
    } else {
        FileStatus::Pass
    };

    ValidationResult {
        file: req.name.clone(),
        status,
        sections,
        reason: None,
    }
}

fn find_doc_file(
    req: &RequiredDoc,
    project_root: &Path,
    repo_path: Option<&Path>,
    is_project_repo: bool,
) -> Option<PathBuf> {
    let search_root = if is_project_repo {
        repo_path?
    } else {
        project_root
    };

    // Check primary name
    let primary = search_root.join(&req.name);
    if primary.exists() && primary.is_file() {
        return Some(primary);
    }

    // Check aliases
    for alias in &req.aliases {
        let alias_path = search_root.join(alias);
        if alias_path.exists() && alias_path.is_file() {
            return Some(alias_path);
        }
    }

    None
}

fn validate_sections(content: &str, sections: &[RequiredSection]) -> Vec<SectionResult> {
    let mut results = Vec::new();

    for section in sections {
        let heading_lower = section.heading.to_lowercase();
        let found = content
            .lines()
            .any(|line| line.trim().to_lowercase().starts_with(&heading_lower));

        if !found && section.required {
            results.push(SectionResult {
                heading: section.heading.clone(),
                status: FileStatus::Fail,
                detail: Some("section_missing".into()),
            });
            continue;
        }

        if !found {
            // Optional section, not found — skip
            continue;
        }

        // Check min_items if specified
        if let Some(min) = section.min_items {
            let count = count_list_items_after_heading(content, &section.heading);
            if count < min {
                results.push(SectionResult {
                    heading: section.heading.clone(),
                    status: FileStatus::Warn,
                    detail: Some(format!("{count} items, min: {min}")),
                });
                continue;
            }
        }

        results.push(SectionResult {
            heading: section.heading.clone(),
            status: FileStatus::Pass,
            detail: None,
        });
    }

    results
}

/// Count markdown list items (- or *) between this heading and the next heading of same/higher level.
fn count_list_items_after_heading(content: &str, heading: &str) -> usize {
    let heading_lower = heading.to_lowercase();
    let heading_level = heading.chars().take_while(|c| *c == '#').count();

    let mut in_section = false;
    let mut count = 0;

    for line in content.lines() {
        let trimmed = line.trim();
        let trimmed_lower = trimmed.to_lowercase();

        if !in_section {
            if trimmed_lower.starts_with(&heading_lower) {
                in_section = true;
            }
            continue;
        }

        // Check if we've hit another heading of same/higher level
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            if level <= heading_level {
                break;
            }
        }

        // Count list items
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            count += 1;
        }
    }

    count
}

// ---------------------------------------------------------------------------
// JSON output (for MCP tool)
// ---------------------------------------------------------------------------

pub fn validation_to_json(
    policy: &PolicyFile,
    results: &[ValidationResult],
    source: &str,
) -> Value {
    let overall = if results.iter().any(|r| r.status == FileStatus::Fail) {
        "fail"
    } else if results.iter().any(|r| r.status == FileStatus::Warn) {
        "warn"
    } else {
        "pass"
    };

    let file_results: Vec<Value> = results
        .iter()
        .map(|r| {
            let mut entry = json!({
                "file": r.file,
                "status": r.status.as_str(),
            });

            if let Some(ref reason) = r.reason {
                entry["reason"] = json!(reason);
            }

            if !r.sections.is_empty() {
                let sections: Vec<Value> = r
                    .sections
                    .iter()
                    .map(|s| {
                        let mut sec = json!({
                            "heading": s.heading,
                            "status": s.status.as_str(),
                        });
                        if let Some(ref detail) = s.detail {
                            sec["detail"] = json!(detail);
                        }
                        sec
                    })
                    .collect();
                entry["sections"] = json!(sections);
            }

            entry
        })
        .collect();

    let pass_count = results
        .iter()
        .filter(|r| r.status == FileStatus::Pass)
        .count();
    let warn_count = results
        .iter()
        .filter(|r| r.status == FileStatus::Warn)
        .count();
    let fail_count = results
        .iter()
        .filter(|r| r.status == FileStatus::Fail)
        .count();

    json!({
        "status": overall,
        "enforce": policy.policy.enforce,
        "policy_source": source,
        "results": file_results,
        "summary": {
            "total": results.len(),
            "pass": pass_count,
            "warn": warn_count,
            "fail": fail_count,
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_with_policy(policy_toml: &str) -> (TempDir, String) {
        let tmp = TempDir::new().unwrap();
        let project = "testproj";
        let project_dir = tmp.path().join(project);
        fs::create_dir_all(&project_dir).unwrap();

        // Write team-level policy
        let policy_dir = tmp.path().join(".alcove");
        fs::create_dir_all(&policy_dir).unwrap();
        fs::write(policy_dir.join("policy.toml"), policy_toml).unwrap();

        (tmp, project.to_string())
    }

    // -- Policy loading --

    #[test]
    fn load_default_policy_from_config() {
        let tmp = TempDir::new().unwrap();
        let policy = load_policy(tmp.path(), "nonexistent");
        assert_eq!(policy.policy.enforce, "warn");
        assert!(!policy.policy.required.is_empty());
        // Test version field exists
        assert!(!policy.policy.version.is_empty());
    }

    #[test]
    fn load_team_policy() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            enforce = "strict"
            version = "1.0"
            [[policy.required]]
            name = "PRD.md"
            description = "Product Requirements Document"

            [policy.naming]
            case = "kebab-case"
            extension = "md"
            max_length = 100
        "###,
        );
        let policy = load_policy(tmp.path(), &project);
        assert_eq!(policy.policy.enforce, "strict");
        assert_eq!(policy.policy.required.len(), 1);
        // Test version and description fields
        assert_eq!(policy.policy.version, "1.0");
        assert_eq!(policy.policy.required[0].description, Some("Product Requirements Document".to_string()));
        // Test naming policy fields
        assert!(policy.policy.naming.is_some());
        let naming = policy.policy.naming.as_ref().unwrap();
        assert_eq!(naming.case, "kebab-case");
        assert_eq!(naming.extension, "md");
        assert_eq!(naming.max_length, 100);

        // Test policy_version function
        assert_eq!(policy_version(&policy), "1.0");

        // Test has_naming_policy function
        assert!(has_naming_policy(&policy));

        // Test validate_doc_name function
        assert!(validate_doc_name(&policy, "test-doc.md").is_ok());
        assert!(validate_doc_name(&policy, "TestDoc.md").is_err());
    }

    #[test]
    fn project_policy_overrides_team() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            enforce = "strict"
            [[policy.required]]
            name = "PRD.md"
        "###,
        );
        // Create project-level policy
        let proj_policy_dir = tmp.path().join(&project).join(".alcove");
        fs::create_dir_all(&proj_policy_dir).unwrap();
        fs::write(
            proj_policy_dir.join("policy.toml"),
            r###"
            [policy]
            enforce = "warn"
            [[policy.required]]
            name = "CUSTOM.md"
        "###,
        )
        .unwrap();

        let policy = load_policy(tmp.path(), &project);
        assert_eq!(policy.policy.enforce, "warn");
        assert_eq!(policy.policy.required[0].name, "CUSTOM.md");
    }

    // -- File existence validation --

    #[test]
    fn validate_missing_file() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            enforce = "strict"
            [[policy.required]]
            name = "PRD.md"
        "###,
        );
        let (_, results) = validate(tmp.path(), &project, None);
        assert_eq!(results[0].status, FileStatus::Fail);
        assert_eq!(results[0].reason.as_deref(), Some("file_not_found"));
    }

    #[test]
    fn validate_existing_file() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            [[policy.required]]
            name = "PRD.md"
        "###,
        );
        let content = format!("# PRD\n\n{}", "x".repeat(200));
        fs::write(tmp.path().join(&project).join("PRD.md"), content).unwrap();

        let (_, results) = validate(tmp.path(), &project, None);
        assert_eq!(results[0].status, FileStatus::Pass);
    }

    #[test]
    fn validate_template_unfilled() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            [[policy.required]]
            name = "PRD.md"
        "###,
        );
        let content = format!("# ProjectName PRD\n\n{}", "x".repeat(200));
        fs::write(tmp.path().join(&project).join("PRD.md"), content).unwrap();

        let (_, results) = validate(tmp.path(), &project, None);
        assert_eq!(results[0].status, FileStatus::Warn);
        assert_eq!(results[0].reason.as_deref(), Some("template_unfilled"));
    }

    #[test]
    fn validate_minimal_content() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            [[policy.required]]
            name = "PRD.md"
        "###,
        );
        fs::write(tmp.path().join(&project).join("PRD.md"), "# PRD\n").unwrap();

        let (_, results) = validate(tmp.path(), &project, None);
        assert_eq!(results[0].status, FileStatus::Warn);
        assert_eq!(results[0].reason.as_deref(), Some("minimal_content"));
    }

    // -- Alias support --

    #[test]
    fn validate_finds_alias() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            [[policy.required]]
            name = "PRD.md"
            aliases = ["prd.md", "PRODUCT.md"]
        "###,
        );
        let content = format!("# Product\n\n{}", "x".repeat(200));
        fs::write(tmp.path().join(&project).join("PRODUCT.md"), content).unwrap();

        let (_, results) = validate(tmp.path(), &project, None);
        assert_eq!(results[0].status, FileStatus::Pass);
    }

    // -- Section validation --

    #[test]
    fn validate_sections_pass() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            [[policy.required]]
            name = "PRD.md"
              [[policy.required.sections]]
              heading = "## Overview"
              required = true
              [[policy.required.sections]]
              heading = "## Goals"
              required = true
        "###,
        );
        let content = "# PRD\n\n## Overview\n\nSome overview text here.\n\n## Goals\n\nSome goals text here and more.\n\nExtra content to pass minimal check.";
        fs::write(tmp.path().join(&project).join("PRD.md"), content).unwrap();

        let (_, results) = validate(tmp.path(), &project, None);
        assert_eq!(results[0].status, FileStatus::Pass);
        assert_eq!(results[0].sections.len(), 2);
    }

    #[test]
    fn validate_section_missing() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            [[policy.required]]
            name = "PRD.md"
              [[policy.required.sections]]
              heading = "## Overview"
              required = true
              [[policy.required.sections]]
              heading = "## Missing Section"
              required = true
        "###,
        );
        let content = format!(
            "# PRD\n\n## Overview\n\nSome content here.\n\n{}\n\nMore content.",
            "x".repeat(100)
        );
        fs::write(tmp.path().join(&project).join("PRD.md"), content).unwrap();

        let (_, results) = validate(tmp.path(), &project, None);
        assert_eq!(results[0].status, FileStatus::Fail);
        assert!(results[0]
            .sections
            .iter()
            .any(|s| s.status == FileStatus::Fail));
    }

    #[test]
    fn validate_section_min_items() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            [[policy.required]]
            name = "PRD.md"
              [[policy.required.sections]]
              heading = "## Features"
              required = true
              min_items = 3
        "###,
        );
        let content = format!(
            "# PRD\n\n## Features\n\n- Feature 1\n- Feature 2\n\n{}",
            "x".repeat(100)
        );
        fs::write(tmp.path().join(&project).join("PRD.md"), content).unwrap();

        let (_, results) = validate(tmp.path(), &project, None);
        // 2 items < 3 required → warn
        let section = &results[0].sections[0];
        assert_eq!(section.status, FileStatus::Warn);
        assert!(section.detail.as_ref().unwrap().contains("2 items, min: 3"));
    }

    #[test]
    fn validate_section_min_items_pass() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            [[policy.required]]
            name = "PRD.md"
              [[policy.required.sections]]
              heading = "## Features"
              required = true
              min_items = 2
        "###,
        );
        let content = "# PRD\n\n## Features\n\n- Feature 1\n- Feature 2\n- Feature 3\n\nExtra content to be over 100 bytes easily here.";
        fs::write(tmp.path().join(&project).join("PRD.md"), content).unwrap();

        let (_, results) = validate(tmp.path(), &project, None);
        assert_eq!(results[0].sections[0].status, FileStatus::Pass);
    }

    // -- Project-repo location --

    #[test]
    fn validate_project_repo_location() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            [[policy.required]]
            name = "CHANGELOG.md"
            location = "project-repo"
        "###,
        );
        let repo = TempDir::new().unwrap();
        let content = format!("# Changelog\n\n{}", "x".repeat(200));
        fs::write(repo.path().join("CHANGELOG.md"), content).unwrap();

        let (_, results) = validate(tmp.path(), &project, Some(repo.path()));
        assert_eq!(results[0].status, FileStatus::Pass);
    }

    #[test]
    fn validate_project_repo_missing() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            [[policy.required]]
            name = "CHANGELOG.md"
            location = "project-repo"
        "###,
        );
        // No repo path provided
        let (_, results) = validate(tmp.path(), &project, None);
        assert_eq!(results[0].status, FileStatus::Fail);
    }

    // -- JSON output --

    #[test]
    fn validation_json_output() {
        let policy = PolicyFile {
            policy: Policy {
                enforce: "strict".into(),
                version: "1".into(),
                required: vec![],
                naming: None,
            },
        };
        let results = vec![
            ValidationResult {
                file: "PRD.md".into(),
                status: FileStatus::Pass,
                sections: vec![],
                reason: None,
            },
            ValidationResult {
                file: "ARCH.md".into(),
                status: FileStatus::Fail,
                sections: vec![],
                reason: Some("file_not_found".into()),
            },
        ];
        let json = validation_to_json(&policy, &results, "team");
        assert_eq!(json["status"], "fail");
        assert_eq!(json["enforce"], "strict");
        assert_eq!(json["summary"]["pass"], 1);
        assert_eq!(json["summary"]["fail"], 1);
        assert_eq!(json["policy_source"], "team");
    }

    // -- count_list_items_after_heading --

    #[test]
    fn count_items_basic() {
        let content = "# Title\n\n## List\n\n- a\n- b\n- c\n\n## Next\n";
        assert_eq!(count_list_items_after_heading(content, "## List"), 3);
    }

    #[test]
    fn count_items_stops_at_same_level() {
        let content = "## A\n\n- x\n\n## B\n\n- y\n";
        assert_eq!(count_list_items_after_heading(content, "## A"), 1);
    }

    #[test]
    fn count_items_includes_subheadings() {
        let content = "## A\n\n### Sub\n\n- x\n- y\n\n## B\n";
        assert_eq!(count_list_items_after_heading(content, "## A"), 2);
    }

    #[test]
    fn count_items_asterisk_lists() {
        let content = "## A\n\n* x\n* y\n";
        assert_eq!(count_list_items_after_heading(content, "## A"), 2);
    }

    // -- Edge case: load_policy with malformed TOML --

    #[test]
    fn load_policy_malformed_toml_returns_default() {
        let tmp = TempDir::new().unwrap();
        let project = "testproj";
        let project_dir = tmp.path().join(project);
        fs::create_dir_all(&project_dir).unwrap();

        // Write malformed TOML at team level
        let policy_dir = tmp.path().join(".alcove");
        fs::create_dir_all(&policy_dir).unwrap();
        fs::write(policy_dir.join("policy.toml"), "{{{{ not valid toml !@#$").unwrap();

        let policy = load_policy(tmp.path(), project);
        // Should fall through to built-in default
        assert_eq!(policy.policy.enforce, "warn");
        assert!(!policy.policy.required.is_empty());
    }

    // -- Edge case: load_policy with empty file --

    #[test]
    fn load_policy_empty_file_returns_default() {
        let tmp = TempDir::new().unwrap();
        let project = "testproj";
        let project_dir = tmp.path().join(project);
        fs::create_dir_all(&project_dir).unwrap();

        let policy_dir = tmp.path().join(".alcove");
        fs::create_dir_all(&policy_dir).unwrap();
        fs::write(policy_dir.join("policy.toml"), "").unwrap();

        let policy = load_policy(tmp.path(), project);
        // Empty TOML cannot deserialize to PolicyFile (missing [policy] table) → default
        assert_eq!(policy.policy.enforce, "warn");
        assert!(!policy.policy.required.is_empty());
    }

    // -- count_list_items_after_heading: no list items (just text) --

    #[test]
    fn count_items_no_list_items_just_text() {
        let content = "## Section\n\nJust some paragraph text.\nAnother line of text.\n\n## Next\n";
        assert_eq!(count_list_items_after_heading(content, "## Section"), 0);
    }

    // -- count_list_items_after_heading: heading not found --

    #[test]
    fn count_items_heading_not_found() {
        let content = "## Existing\n\n- item\n";
        assert_eq!(count_list_items_after_heading(content, "## NonExistent"), 0);
    }

    // -- count_list_items_after_heading: mixed - and * list items --

    #[test]
    fn count_items_mixed_dash_and_asterisk() {
        let content =
            "## Mixed\n\n- dash item\n* star item\n- another dash\n* another star\n\n## End\n";
        assert_eq!(count_list_items_after_heading(content, "## Mixed"), 4);
    }

    // -- count_list_items_after_heading: numbered list items should NOT count --

    #[test]
    fn count_items_numbered_list_not_counted() {
        let content = "## Numbered\n\n1. first\n2. second\n3. third\n- actual item\n\n## End\n";
        // Only the `- actual item` should count (numbered items are not - or *)
        assert_eq!(count_list_items_after_heading(content, "## Numbered"), 1);
    }

    // -- validate: file with all sections passing --

    #[test]
    fn validate_all_sections_passing() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            enforce = "strict"
            [[policy.required]]
            name = "PRD.md"
              [[policy.required.sections]]
              heading = "## Overview"
              required = true
              min_items = 2
              [[policy.required.sections]]
              heading = "## Goals"
              required = true
              min_items = 1
        "###,
        );
        let content = "\
# PRD

## Overview

- Point one
- Point two
- Point three

## Goals

- Goal one

Extra content to ensure we are over 100 bytes threshold for minimal content check easily.";
        fs::write(tmp.path().join(&project).join("PRD.md"), content).unwrap();

        let (_, results) = validate(tmp.path(), &project, None);
        assert_eq!(results[0].status, FileStatus::Pass);
        assert_eq!(results[0].sections.len(), 2);
        assert!(results[0]
            .sections
            .iter()
            .all(|s| s.status == FileStatus::Pass));
    }

    // -- validate: enforce = "strict" vs "relaxed" appears in JSON output --

    #[test]
    fn validate_enforce_strict_in_json_output() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            enforce = "strict"
            [[policy.required]]
            name = "PRD.md"
        "###,
        );
        let content = format!("# PRD\n\n{}", "x".repeat(200));
        fs::write(tmp.path().join(&project).join("PRD.md"), content).unwrap();

        let (policy, results) = validate(tmp.path(), &project, None);
        let source = policy_source(tmp.path(), &project);
        let json = validation_to_json(&policy, &results, source);
        assert_eq!(json["enforce"], "strict");
    }

    #[test]
    fn validate_enforce_relaxed_in_json_output() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            enforce = "relaxed"
            [[policy.required]]
            name = "PRD.md"
        "###,
        );
        let content = format!("# PRD\n\n{}", "x".repeat(200));
        fs::write(tmp.path().join(&project).join("PRD.md"), content).unwrap();

        let (policy, results) = validate(tmp.path(), &project, None);
        let source = policy_source(tmp.path(), &project);
        let json = validation_to_json(&policy, &results, source);
        assert_eq!(json["enforce"], "relaxed");
    }

    // -- policy_source: test all 3 sources --

    #[test]
    fn policy_source_returns_project() {
        let tmp = TempDir::new().unwrap();
        let project = "testproj";
        let proj_policy_dir = tmp.path().join(project).join(".alcove");
        fs::create_dir_all(&proj_policy_dir).unwrap();
        fs::write(proj_policy_dir.join("policy.toml"), "[policy]").unwrap();

        assert_eq!(policy_source(tmp.path(), project), "project");
    }

    #[test]
    fn policy_source_returns_team() {
        let tmp = TempDir::new().unwrap();
        let project = "testproj";
        fs::create_dir_all(tmp.path().join(project)).unwrap();
        let team_dir = tmp.path().join(".alcove");
        fs::create_dir_all(&team_dir).unwrap();
        fs::write(team_dir.join("policy.toml"), "[policy]").unwrap();

        assert_eq!(policy_source(tmp.path(), project), "team");
    }

    #[test]
    fn policy_source_returns_default() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(policy_source(tmp.path(), "noproj"), "default");
    }

    // -- validation_to_json: verify JSON structure has all expected fields --

    #[test]
    fn validation_to_json_has_all_fields() {
        let policy = PolicyFile {
            policy: Policy {
                enforce: "warn".into(),
                version: "1".into(),
                required: vec![],
                naming: None,
            },
        };
        let results = vec![
            ValidationResult {
                file: "A.md".into(),
                status: FileStatus::Pass,
                sections: vec![SectionResult {
                    heading: "## Intro".into(),
                    status: FileStatus::Pass,
                    detail: None,
                }],
                reason: None,
            },
            ValidationResult {
                file: "B.md".into(),
                status: FileStatus::Warn,
                sections: vec![],
                reason: Some("minimal_content".into()),
            },
            ValidationResult {
                file: "C.md".into(),
                status: FileStatus::Fail,
                sections: vec![],
                reason: Some("file_not_found".into()),
            },
        ];
        let json = validation_to_json(&policy, &results, "default");

        // Top-level fields
        assert!(json.get("status").is_some());
        assert!(json.get("enforce").is_some());
        assert!(json.get("policy_source").is_some());
        assert!(json.get("results").is_some());
        assert!(json.get("summary").is_some());

        // Summary sub-fields
        let summary = &json["summary"];
        assert_eq!(summary["total"], 3);
        assert_eq!(summary["pass"], 1);
        assert_eq!(summary["warn"], 1);
        assert_eq!(summary["fail"], 1);

        // Overall status should be "fail" (worst of pass/warn/fail)
        assert_eq!(json["status"], "fail");
        assert_eq!(json["policy_source"], "default");

        // Results array structure
        let arr = json["results"].as_array().unwrap();
        assert_eq!(arr.len(), 3);
        // First result has sections
        assert!(arr[0].get("sections").is_some());
        let sections = arr[0]["sections"].as_array().unwrap();
        assert_eq!(sections[0]["heading"], "## Intro");
        assert_eq!(sections[0]["status"], "pass");
        // Second result has reason
        assert_eq!(arr[1]["reason"], "minimal_content");
        // Third result has reason
        assert_eq!(arr[2]["reason"], "file_not_found");
    }

    // -- validate: naming policy deserialization --

    #[test]
    fn load_policy_with_naming_section() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            enforce = "strict"

            [policy.naming]
            case = "kebab"
            extension = ".md"
            max_length = 30

            [[policy.required]]
            name = "PRD.md"
        "###,
        );
        let policy = load_policy(tmp.path(), &project);
        assert_eq!(policy.policy.enforce, "strict");
        let naming = policy.policy.naming.as_ref().unwrap();
        assert_eq!(naming.case, "kebab");
        assert_eq!(naming.extension, ".md");
        assert_eq!(naming.max_length, 30);
    }

    // -- validate: file exists but is completely empty (0 bytes) --

    #[test]
    fn validate_empty_zero_byte_file() {
        let (tmp, project) = setup_with_policy(
            r###"
            [policy]
            [[policy.required]]
            name = "PRD.md"
        "###,
        );
        fs::write(tmp.path().join(&project).join("PRD.md"), "").unwrap();

        let (_, results) = validate(tmp.path(), &project, None);
        // Empty file has 0 bytes < 100 → minimal_content warning
        assert_eq!(results[0].status, FileStatus::Warn);
        assert_eq!(results[0].reason.as_deref(), Some("minimal_content"));
    }

    #[test]
    fn count_list_items_stops_at_same_level_heading() {
        let content = "## Features\n- item1\n- item2\n## Other\n- not counted";
        let count = count_list_items_after_heading(content, "## Features");
        assert_eq!(count, 2);
    }

    #[test]
    fn count_list_items_includes_sub_headings() {
        let content = "## Features\n- item1\n### Sub\n- item2\n- item3\n## Other\n- nope";
        let count = count_list_items_after_heading(content, "## Features");
        assert_eq!(count, 3, "should count items under sub-headings too");
    }

    #[test]
    fn count_list_items_asterisk_prefix() {
        let content = "## Items\n* one\n* two\n* three";
        let count = count_list_items_after_heading(content, "## Items");
        assert_eq!(count, 3, "should count * list items");
    }

    #[test]
    fn count_list_items_no_heading_match() {
        let content = "## Something\n- item";
        let count = count_list_items_after_heading(content, "## Missing");
        assert_eq!(count, 0);
    }

    #[test]
    fn validate_project_repo_file_found() {
        let tmp = TempDir::new().unwrap();
        let project = "projrepo_test";
        let project_dir = tmp.path().join(project);
        fs::create_dir_all(project_dir.join(".alcove")).unwrap();

        let policy_toml = r###"
            [policy]
            [[policy.required]]
            name = "README.md"
            location = "project-repo"
        "###;
        fs::write(project_dir.join(".alcove/policy.toml"), policy_toml).unwrap();

        // Create the file in repo, not alcove
        let repo = TempDir::new().unwrap();
        fs::write(repo.path().join("README.md"), "# README\n\nProject readme with enough content to pass the 100 byte threshold for validation. Adding more text to be absolutely sure we exceed the limit.").unwrap();

        let (_, results) = validate(tmp.path(), project, Some(repo.path()));
        assert_eq!(results[0].status, FileStatus::Pass);
    }

    #[test]
    fn validate_project_repo_file_missing() {
        let tmp = TempDir::new().unwrap();
        let project = "projrepo_missing";
        let project_dir = tmp.path().join(project);
        fs::create_dir_all(project_dir.join(".alcove")).unwrap();

        let policy_toml = r###"
            [policy]
            [[policy.required]]
            name = "README.md"
            location = "project-repo"
        "###;
        fs::write(project_dir.join(".alcove/policy.toml"), policy_toml).unwrap();

        // No repo path — file can't be found
        let (_, results) = validate(tmp.path(), project, None);
        assert_eq!(results[0].status, FileStatus::Fail);
        assert_eq!(results[0].reason.as_deref(), Some("file_not_found"));
    }

    #[test]
    fn file_status_as_str() {
        assert_eq!(FileStatus::Pass.as_str(), "pass");
        assert_eq!(FileStatus::Warn.as_str(), "warn");
        assert_eq!(FileStatus::Fail.as_str(), "fail");
    }

    #[test]
    fn default_policy_has_all_core_files() {
        let policy = default_policy();
        let core = load_config().core_files();
        assert_eq!(policy.policy.required.len(), core.len());
        for (req, name) in policy.policy.required.iter().zip(core.iter()) {
            assert_eq!(&req.name, name);
        }
    }
}
