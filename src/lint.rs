use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Value, json};
use walkdir::WalkDir;

// ---------------------------------------------------------------------------
// Compiled regexes (allocated once, reused across all lint calls)
// ---------------------------------------------------------------------------

fn fence_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)(?:```|~~~)[^\n]*\n.*?(?:```|~~~)").unwrap())
}

fn inline_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`[^`\n]+`").unwrap())
}

fn wiki_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[\[([^\]|]+)(?:\|[^\]]*)?\]\]").unwrap())
}

fn md_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[(?:[^\]]*)\]\(([^)]+)\)").unwrap())
}

fn wikilink_strip_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[\[[^\]]+\]\]").unwrap())
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum LintSeverity {
    Warning,
    Info,
}

impl LintSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            LintSeverity::Warning => "warning",
            LintSeverity::Info => "info",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LintIssue {
    pub severity: LintSeverity,
    pub kind: &'static str,
    pub file: String,
    pub message: String,
}

#[derive(Debug)]
pub struct LintReport {
    pub issues: Vec<LintIssue>,
    pub files_scanned: usize,
}

// ---------------------------------------------------------------------------
// Stale markers
// ---------------------------------------------------------------------------

const STALE_MARKERS: &[&str] = &[
    "WIP",
    "TODO",
    "FIXME",
    "DRAFT",
    "DEPRECATED",
    "DO NOT USE",
    "OUTDATED",
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collect all markdown/text doc files under docs_root (optionally filtered by project).
fn collect_doc_files(docs_root: &Path, project_filter: Option<&str>) -> Vec<PathBuf> {
    let scan_root = if let Some(p) = project_filter {
        let d = docs_root.join(p);
        if d.is_dir() { d } else { return vec![] }
    } else {
        docs_root.to_path_buf()
    };

    WalkDir::new(&scan_root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            let ext = e.path().extension().and_then(|s| s.to_str()).unwrap_or("");
            matches!(ext, "md" | "txt" | "markdown")
        })
        .map(|e| e.path().to_path_buf())
        .collect()
}

/// Build a lookup map: filename (no ext, lowercased) → full path.
/// Used for Obsidian-style wikilink resolution.
fn build_filename_map(files: &[PathBuf]) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    for path in files {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            map.insert(stem.to_lowercase(), path.clone());
        }
        // Also store full filename (with extension)
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            map.insert(name.to_lowercase(), path.clone());
        }
    }
    map
}

/// Return true if the file is an index/readme/moc (excluded from orphan check).
fn is_index_file(path: &Path) -> bool {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        stem.as_str(),
        "index" | "readme" | "moc" | "map-of-content" | "home" | "start" | "_index"
    )
}

/// Replace fenced code blocks (``` … ``` and ~~~ … ~~~) and inline `code`
/// spans with spaces, preserving byte offsets so downstream regex matches
/// remain valid. This prevents TOML `[[table]]` syntax inside code fences
/// from being parsed as Obsidian-style wikilinks.
fn strip_code_blocks(content: &str) -> String {
    let after_fences = fence_re().replace_all(content, |caps: &regex::Captures| {
        " ".repeat(caps[0].len())
    });
    inline_re()
        .replace_all(&after_fences, |caps: &regex::Captures| {
            " ".repeat(caps[0].len())
        })
        .into_owned()
}

/// Extract all wikilinks `[[target]]` and markdown links `[text](path)` from content.
fn extract_links(content: &str) -> Vec<String> {
    let mut links = Vec::new();
    // Strip code blocks first so TOML [[table]] syntax isn't treated as wikilinks.
    let prose = strip_code_blocks(content);

    // Wikilinks: [[target]] or [[target|alias]]
    for cap in wiki_link_re().captures_iter(&prose) {
        links.push(cap[1].trim().to_string());
    }

    // Markdown links: [text](path) — skip http/https/mailto
    for cap in md_link_re().captures_iter(&prose) {
        let target = cap[1].trim();
        if !target.starts_with("http://")
            && !target.starts_with("https://")
            && !target.starts_with("mailto:")
            && !target.starts_with('#')
        {
            // Strip anchor from path
            let path_part = target.split('#').next().unwrap_or(target);
            if !path_part.is_empty() {
                links.push(path_part.to_string());
            }
        }
    }

    links
}

/// Resolve a link target relative to the file that contains it and docs_root.
/// Returns true if the link resolves to an existing file.
fn resolve_link(
    link: &str,
    containing_file: &Path,
    docs_root: &Path,
    filename_map: &HashMap<String, PathBuf>,
) -> bool {
    // Try relative path from containing file's directory
    if let Some(parent) = containing_file.parent() {
        let candidate = parent.join(link);
        if candidate.exists() {
            return true;
        }
        // Try with .md extension
        let with_md = parent.join(format!("{}.md", link));
        if with_md.exists() {
            return true;
        }
    }

    // Try relative path from docs_root
    let from_root = docs_root.join(link);
    if from_root.exists() {
        return true;
    }
    let from_root_md = docs_root.join(format!("{}.md", link));
    if from_root_md.exists() {
        return true;
    }

    // Obsidian-style: match by filename anywhere under docs_root
    let link_lower = link.to_lowercase();
    // Strip any path prefix — use just the filename part
    let filename_part = Path::new(&link_lower)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&link_lower);

    if filename_map.contains_key(filename_part) {
        return true;
    }
    // Also try stripping .md suffix for stem lookup
    let stem = filename_part.strip_suffix(".md").unwrap_or(filename_part);
    filename_map.contains_key(stem)
}

// ---------------------------------------------------------------------------
// Current year helper
// ---------------------------------------------------------------------------

fn current_year() -> i32 {
    // Fall back to system time
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Approximate year from seconds (good enough for "2+ years ago" check)
    1970 + (secs / 31_557_600) as i32
}

// ---------------------------------------------------------------------------
// Core lint function
// ---------------------------------------------------------------------------

pub fn lint(docs_root: &Path, project_filter: Option<&str>) -> LintReport {
    lint_with_year(docs_root, project_filter, current_year())
}

fn lint_with_year(docs_root: &Path, project_filter: Option<&str>, now_year: i32) -> LintReport {
    let files = collect_doc_files(docs_root, project_filter);
    let filename_map = build_filename_map(&files);
    let files_scanned = files.len();
    let mut issues = Vec::new();

    // For orphan detection: track which files are linked to
    let mut linked_files: HashSet<PathBuf> = HashSet::new();

    // Per-file link extraction for orphan analysis
    let mut file_links: Vec<(PathBuf, Vec<String>)> = Vec::new();

    // Stale marker regex
    let stale_marker_re = {
        let pattern = STALE_MARKERS
            .iter()
            .map(|m| regex::escape(m))
            .collect::<Vec<_>>()
            .join("|");
        Regex::new(&format!(r"(?i)\b({})\b", pattern)).unwrap()
    };

    // Stale date regex: matches years like "in 2019", "(2020)", "as of 2021".
    // `\b` already excludes `v2023` (no word boundary between `v` and `2`).
    // False positives in URLs and version strings (e.g. `/2023/`, `2023.1.0`)
    // are filtered out in the match loop below by inspecting the surrounding chars.
    let stale_date_re = Regex::new(r"\b(20\d{2}|19\d{2})\b").unwrap();

    for file_path in &files {
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let rel = file_path
            .strip_prefix(docs_root)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();

        // --- broken-link ---
        let links = extract_links(&content);
        for link in &links {
            if !resolve_link(link, file_path, docs_root, &filename_map) {
                issues.push(LintIssue {
                    severity: LintSeverity::Warning,
                    kind: "broken-link",
                    file: rel.clone(),
                    message: format!("Broken link: [[{}]]", link),
                });
            } else {
                // Resolve to canonical path for orphan tracking
                // Best-effort: try to find the actual path
                let resolved = resolve_to_path(link, file_path, docs_root, &filename_map);
                if let Some(p) = resolved {
                    linked_files.insert(p);
                }
            }
        }

        file_links.push((file_path.clone(), links));

        // --- stale-marker ---
        // Strip code blocks and wikilink targets so that e.g. [[deprecated/note]]
        // doesn't trigger a false-positive DEPRECATED marker hit.
        let prose_for_markers = {
            let no_code = strip_code_blocks(&content);
            wikilink_strip_re()
                .replace_all(&no_code, |caps: &regex::Captures| {
                    " ".repeat(caps[0].len())
                })
                .into_owned()
        };
        for cap in stale_marker_re.captures_iter(&prose_for_markers) {
            let marker = cap[1].to_uppercase();
            issues.push(LintIssue {
                severity: LintSeverity::Warning,
                kind: "stale-marker",
                file: rel.clone(),
                message: format!("Contains stale marker: {}", marker),
            });
        }

        // --- stale-date ---
        let content_bytes = content.as_bytes();
        for cap in stale_date_re.captures_iter(&content) {
            let m = cap.get(1).unwrap();
            // Skip false positives: year inside URL path, version string, or date.
            // Check character immediately before/after the match.
            let before = if m.start() > 0 { content_bytes[m.start() - 1] } else { b' ' };
            let after = if m.end() < content_bytes.len() { content_bytes[m.end()] } else { b' ' };
            if before == b'/' || before == b'-' {
                continue; // URL path segment or date continuation
            }
            if after == b'.' || after == b'-' || after == b'/' || after.is_ascii_digit() {
                continue; // version string or date continuation
            }

            if let Ok(year) = cap[1].parse::<i32>()
                && now_year - year >= 2 {
                    issues.push(LintIssue {
                        severity: LintSeverity::Info,
                        kind: "stale-date",
                        file: rel.clone(),
                        message: format!(
                            "Mentions year {} which is {} year(s) old",
                            year,
                            now_year - year
                        ),
                    });
                    break; // one issue per file
                }
        }
    }

    // --- orphan ---
    for file_path in &files {
        if is_index_file(file_path) {
            continue;
        }
        if !linked_files.contains(file_path) {
            let rel = file_path
                .strip_prefix(docs_root)
                .unwrap_or(file_path)
                .to_string_lossy()
                .to_string();
            issues.push(LintIssue {
                severity: LintSeverity::Info,
                kind: "orphan",
                file: rel.clone(),
                message: "No other document links to this file".to_string(),
            });
        }
    }

    LintReport {
        issues,
        files_scanned,
    }
}

/// Like resolve_link but returns the actual PathBuf.
fn resolve_to_path(
    link: &str,
    containing_file: &Path,
    docs_root: &Path,
    filename_map: &HashMap<String, PathBuf>,
) -> Option<PathBuf> {
    if let Some(parent) = containing_file.parent() {
        let candidate = parent.join(link);
        if candidate.exists() {
            return Some(candidate.canonicalize().ok().unwrap_or(candidate));
        }
        let with_md = parent.join(format!("{}.md", link));
        if with_md.exists() {
            return Some(with_md.canonicalize().ok().unwrap_or(with_md));
        }
    }
    let from_root = docs_root.join(link);
    if from_root.exists() {
        return Some(from_root.canonicalize().ok().unwrap_or(from_root));
    }
    let from_root_md = docs_root.join(format!("{}.md", link));
    if from_root_md.exists() {
        return Some(from_root_md.canonicalize().ok().unwrap_or(from_root_md));
    }

    let link_lower = link.to_lowercase();
    let filename_part = Path::new(&link_lower)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&link_lower)
        .to_string();

    if let Some(p) = filename_map.get(&filename_part) {
        return Some(p.clone());
    }
    let stem = if filename_part.ends_with(".md") {
        filename_part[..filename_part.len() - 3].to_string()
    } else {
        filename_part
    };
    filename_map.get(&stem).cloned()
}

// ---------------------------------------------------------------------------
// Output helpers
// ---------------------------------------------------------------------------

pub fn lint_to_json(report: &LintReport) -> Value {
    let issues: Vec<Value> = report
        .issues
        .iter()
        .map(|i| {
            json!({
                "severity": i.severity.as_str(),
                "kind": i.kind,
                "file": i.file,
                "message": i.message,
            })
        })
        .collect();

    json!({
        "files_scanned": report.files_scanned,
        "issue_count": report.issues.len(),
        "issues": issues,
    })
}

pub fn print_lint_human(report: &LintReport, project: &str) {
    use console::style;

    println!();
    println!("{}", style(format!("Lint: {}", project)).bold());
    println!(
        "{}",
        style(format!("Files scanned: {}", report.files_scanned)).dim()
    );
    println!();

    if report.issues.is_empty() {
        println!("{}", style("  No issues found.").green());
    } else {
        for issue in &report.issues {
            let label = match issue.severity {
                LintSeverity::Warning => style(format!("  WARN  [{}]", issue.kind)).yellow(),
                LintSeverity::Info => style(format!("  INFO  [{}]", issue.kind)).cyan(),
            };
            println!("{} {} — {}", label, issue.file, issue.message);
        }
    }

    let warnings = report
        .issues
        .iter()
        .filter(|i| i.severity == LintSeverity::Warning)
        .count();
    let infos = report
        .issues
        .iter()
        .filter(|i| i.severity == LintSeverity::Info)
        .count();

    println!();
    println!(
        "Summary: {} warning(s), {} info",
        style(warnings).yellow(),
        style(infos).cyan(),
    );
    println!();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn test_no_issues_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let report = lint(tmp.path(), None);
        assert_eq!(report.files_scanned, 0);
        assert!(report.issues.is_empty());
    }

    #[test]
    fn test_stale_marker_todo() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "proj/note.md", "# Note\nTODO: fix this\n");
        let report = lint(tmp.path(), None);
        let stale: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.kind == "stale-marker")
            .collect();
        assert!(!stale.is_empty(), "expected stale-marker issue");
    }

    #[test]
    fn test_stale_marker_wip() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "proj/note.md", "# WIP document\nContent here.\n");
        let report = lint(tmp.path(), None);
        let stale: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.kind == "stale-marker")
            .collect();
        assert!(!stale.is_empty());
    }

    #[test]
    fn test_stale_date() {
        let tmp = TempDir::new().unwrap();
        // Use year that is definitely 2+ years ago from any reasonable current year
        write(tmp.path(), "proj/note.md", "# Note\nAs of 2018, this was true.\n");
        // Inject year directly — no env mutation, safe under parallel test execution
        let report = lint_with_year(tmp.path(), None, 2026);
        let dated: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.kind == "stale-date")
            .collect();
        assert!(!dated.is_empty(), "expected stale-date issue");
    }

    #[test]
    fn test_no_stale_date_recent() {
        let tmp = TempDir::new().unwrap();
        // 2025 is within 2 years of 2026
        write(tmp.path(), "proj/note.md", "# Note\nUpdated in 2025.\n");
        // Inject year directly — no env mutation, safe under parallel test execution
        let report = lint_with_year(tmp.path(), None, 2026);
        let dated: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.kind == "stale-date")
            .collect();
        assert!(dated.is_empty(), "2025 should not be stale in 2026");
    }

    #[test]
    fn test_broken_wikilink() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "proj/note.md",
            "# Note\nSee [[nonexistent-file]] for details.\n",
        );
        let report = lint(tmp.path(), None);
        let broken: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.kind == "broken-link")
            .collect();
        assert!(!broken.is_empty(), "expected broken-link issue");
    }

    #[test]
    fn test_valid_wikilink() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "proj/target.md", "# Target\nContent.\n");
        write(
            tmp.path(),
            "proj/note.md",
            "# Note\nSee [[target]] for details.\n",
        );
        let report = lint(tmp.path(), None);
        let broken: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.kind == "broken-link")
            .collect();
        assert!(broken.is_empty(), "valid wikilink should not be broken");
    }

    #[test]
    fn test_orphan_detection() {
        let tmp = TempDir::new().unwrap();
        // Only one file, not linked from anywhere
        write(tmp.path(), "proj/lonely.md", "# Lonely\nContent.\n");
        let report = lint(tmp.path(), None);
        let orphans: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.kind == "orphan")
            .collect();
        assert!(!orphans.is_empty(), "expected orphan issue");
    }

    #[test]
    fn test_index_not_orphan() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "proj/index.md", "# Index\nContent.\n");
        let report = lint(tmp.path(), None);
        let orphans: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.kind == "orphan")
            .collect();
        assert!(orphans.is_empty(), "index.md should not be flagged as orphan");
    }

    #[test]
    fn test_project_filter() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "project-a/note.md", "# Note A\nTODO: fix\n");
        write(tmp.path(), "project-b/note.md", "# Note B\nClean content.\n");
        let report = lint(tmp.path(), Some("project-a"));
        assert_eq!(report.files_scanned, 1);
        let stale: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.kind == "stale-marker")
            .collect();
        assert!(!stale.is_empty());
    }

    #[test]
    fn test_stale_marker_multiple_in_one_file() {
        let tmp = TempDir::new().unwrap();
        // File contains both TODO and FIXME — both must be reported
        write(
            tmp.path(),
            "proj/note.md",
            "# Note\nTODO: do something\nFIXME: broken logic\n",
        );
        let report = lint(tmp.path(), None);
        let stale: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.kind == "stale-marker")
            .collect();
        assert_eq!(stale.len(), 2, "expected two stale-marker issues, one per match");
        let messages: Vec<&str> = stale.iter().map(|i| i.message.as_str()).collect();
        assert!(messages.iter().any(|m| m.contains("TODO")));
        assert!(messages.iter().any(|m| m.contains("FIXME")));
    }

    #[test]
    fn test_lint_to_json() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "proj/note.md", "# Note\nTODO: fix this\n");
        let report = lint(tmp.path(), None);
        let json = lint_to_json(&report);
        assert!(json["files_scanned"].as_u64().unwrap() >= 1);
        assert!(json["issues"].as_array().is_some());
    }

    #[test]
    fn test_wikilink_in_code_fence_not_broken_link() {
        // [[policy.required]] inside a TOML code block must NOT be treated
        // as a wikilink — this was the SPEC_DOC_POLICY.md false-positive bug.
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "proj/spec.md",
            "# Spec\n\n```toml\n[[policy.required]]\nname = \"PRD.md\"\n```\n",
        );
        let report = lint(tmp.path(), None);
        let broken: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.kind == "broken-link")
            .collect();
        assert!(
            broken.is_empty(),
            "[[...]] inside code fence should not be a broken-link: {broken:?}"
        );
    }

    #[test]
    fn test_deprecated_wikilink_path_not_stale_marker() {
        // [[deprecated/some-note]] uses "deprecated" as a path component.
        // The word should NOT trigger a stale-marker warning — only prose
        // occurrences of DEPRECATED should count.
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "proj/some-note.md",
            "# Note\nSee also [[deprecated/some-note]].\n",
        );
        write(tmp.path(), "deprecated/some-note.md", "# Old note\n");
        let report = lint(tmp.path(), None);
        let markers: Vec<_> = report
            .issues
            .iter()
            .filter(|i| i.kind == "stale-marker")
            .collect();
        assert!(
            markers.is_empty(),
            "wikilink path 'deprecated/...' should not trigger stale-marker: {markers:?}"
        );
    }
}
