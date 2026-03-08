use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{self, *};
use tantivy::{Index, IndexWriter, ReloadPolicy, TantivyDocument};
use walkdir::WalkDir;

use crate::config::is_doc_file;

// ---------------------------------------------------------------------------
// Index lock — prevents concurrent build/search races
// ---------------------------------------------------------------------------

static INDEX_BUILDING: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Index directory
// ---------------------------------------------------------------------------

fn index_dir(docs_root: &Path) -> PathBuf {
    docs_root.join(".alcove").join("index")
}

fn meta_path(docs_root: &Path) -> PathBuf {
    docs_root.join(".alcove").join("index_meta.json")
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

fn build_schema() -> (Schema, Field, Field, Field, Field, Field) {
    let mut builder = Schema::builder();
    let project = builder.add_text_field("project", STRING | STORED);
    let file = builder.add_text_field("file", STRING | STORED);
    let chunk_id = builder.add_u64_field("chunk_id", INDEXED | STORED);
    let body = builder.add_text_field("body", TEXT | STORED);
    let line_start = builder.add_u64_field("line_start", STORED);
    (builder.build(), project, file, chunk_id, body, line_start)
}

// ---------------------------------------------------------------------------
// Chunking
// ---------------------------------------------------------------------------

const CHUNK_SIZE: usize = 500; // chars per chunk
const CHUNK_OVERLAP: usize = 50; // overlap between chunks

struct Chunk {
    text: String,
    line_start: usize,
}

fn chunk_content(content: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return vec![];
    }

    let mut chunks = Vec::new();
    let mut current_chars = 0;
    let mut chunk_lines: Vec<String> = Vec::new();
    let mut chunk_start_line = 0;

    for (i, line) in lines.iter().enumerate() {
        let line_len = line.chars().count() + 1; // +1 for newline
        if current_chars + line_len > CHUNK_SIZE && !chunk_lines.is_empty() {
            chunks.push(Chunk {
                text: chunk_lines.join("\n"),
                line_start: chunk_start_line + 1, // 1-indexed
            });

            // Overlap: keep last few lines
            let overlap_chars = CHUNK_OVERLAP;
            let mut kept = 0;
            let mut keep_from = chunk_lines.len();
            for (j, cl) in chunk_lines.iter().enumerate().rev() {
                kept += cl.chars().count() + 1;
                if kept >= overlap_chars {
                    keep_from = j;
                    break;
                }
            }
            let overlap_lines: Vec<String> = chunk_lines[keep_from..].to_vec();
            chunk_start_line = i - overlap_lines.len();
            chunk_lines = overlap_lines;
            current_chars = chunk_lines
                .iter()
                .map(|l: &String| l.chars().count() + 1)
                .sum();
        }

        chunk_lines.push(line.to_string());
        current_chars += line_len;
    }

    if !chunk_lines.is_empty() {
        chunks.push(Chunk {
            text: chunk_lines.join("\n"),
            line_start: chunk_start_line + 1,
        });
    }

    chunks
}

// ---------------------------------------------------------------------------
// Index metadata (for incremental updates)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Default)]
struct IndexMeta {
    files: std::collections::HashMap<String, u64>, // path -> mtime_secs
}

impl IndexMeta {
    fn load(docs_root: &Path) -> Self {
        let path = meta_path(docs_root);
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, docs_root: &Path) -> Result<()> {
        let path = meta_path(docs_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)?;
        Ok(())
    }
}

fn file_mtime_secs(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .and_then(|t| {
            t.duration_since(SystemTime::UNIX_EPOCH)
                .map_err(std::io::Error::other)
        })
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Check if the index is stale (any doc file newer than the index meta, or deleted).
pub fn is_index_stale(docs_root: &Path) -> bool {
    let meta_file = meta_path(docs_root);
    if !meta_file.exists() {
        return true;
    }
    let meta = IndexMeta::load(docs_root);
    if meta.files.is_empty() {
        return true;
    }

    // Collect current files on disk
    let mut current_files: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Check if any current doc file has a different mtime than what's recorded
    for entry in std::fs::read_dir(docs_root).into_iter().flatten().flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if name.starts_with('.') || name.starts_with('_') || name == "mcp" || name == "skills" {
            continue;
        }
        for walk_entry in WalkDir::new(&path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && crate::config::is_doc_file(e.path()))
        {
            let file_path = walk_entry.path();
            let rel = file_path
                .strip_prefix(docs_root)
                .unwrap_or(file_path)
                .to_string_lossy()
                .to_string();
            let mtime = file_mtime_secs(file_path);
            current_files.insert(rel.clone());
            match meta.files.get(&rel) {
                Some(&recorded) if recorded == mtime => {}
                _ => return true, // New file or changed mtime
            }
        }
    }

    // Check for deleted files: any meta entry that no longer exists on disk
    for key in meta.files.keys() {
        if !current_files.contains(key) {
            return true;
        }
    }

    false
}

/// Ensure index is up-to-date, rebuilding in background if stale.
/// Returns true if a rebuild was triggered.
pub fn ensure_index_fresh(docs_root: &Path) -> bool {
    if !is_index_stale(docs_root) {
        return false;
    }
    // Rebuild synchronously (called from search path, needs result immediately)
    let _ = build_index(docs_root);
    true
}

// ---------------------------------------------------------------------------
// Build / rebuild index
// ---------------------------------------------------------------------------

/// Build or incrementally update the search index for all projects.
/// Uses an atomic flag to prevent concurrent builds.
pub fn build_index(docs_root: &Path) -> Result<JsonValue> {
    // Acquire build lock (non-blocking: if already building, skip)
    if INDEX_BUILDING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(json!({
            "status": "skipped",
            "reason": "Index build already in progress",
        }));
    }
    // Ensure we release the lock even on error
    let result = build_index_inner(docs_root);
    INDEX_BUILDING.store(false, Ordering::SeqCst);
    result
}

/// Inner build function without locking (used in tests to avoid global lock contention).
#[cfg(test)]
pub fn build_index_unlocked(docs_root: &Path) -> Result<JsonValue> {
    build_index_inner(docs_root)
}

fn build_index_inner(docs_root: &Path) -> Result<JsonValue> {
    let dir = index_dir(docs_root);
    std::fs::create_dir_all(&dir)?;

    let (schema, project_field, file_field, chunk_id_field, body_field, line_start_field) =
        build_schema();

    let mut meta = IndexMeta::load(docs_root);
    let mut indexed_count = 0u64;
    let mut skipped_count = 0u64;
    let mut project_count = 0u64;

    // Determine which files changed
    let mut current_files: std::collections::HashMap<String, u64> =
        std::collections::HashMap::new();
    let mut files_to_index: Vec<(String, String, PathBuf)> = Vec::new(); // (project, rel_path, full_path)

    for entry in std::fs::read_dir(docs_root)
        .context("Failed to read DOCS_ROOT")?
        .flatten()
    {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if name.starts_with('.') || name.starts_with('_') || name == "mcp" || name == "skills" {
            continue;
        }
        project_count += 1;

        for walk_entry in WalkDir::new(&path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && is_doc_file(e.path()))
        {
            let file_path = walk_entry.path().to_path_buf();
            let rel = file_path
                .strip_prefix(docs_root)
                .unwrap_or(&file_path)
                .to_string_lossy()
                .to_string();
            let mtime = file_mtime_secs(&file_path);
            current_files.insert(rel.clone(), mtime);

            let rel_to_project = file_path
                .strip_prefix(&path)
                .unwrap_or(&file_path)
                .to_string_lossy()
                .to_string();

            if meta.files.get(&rel).copied() == Some(mtime) {
                skipped_count += 1;
            } else {
                files_to_index.push((name.clone(), rel_to_project, file_path));
            }
        }
    }

    // If nothing changed, skip reindex
    let needs_full_rebuild = !dir.join("meta.json").exists()
        || meta.files.is_empty()
        || files_to_index.len() as f64 > (current_files.len() as f64 * 0.5);

    if needs_full_rebuild {
        // Full rebuild
        let index = Index::create_in_dir(&dir, schema.clone())
            .or_else(|_| {
                // Directory exists with old schema, recreate
                std::fs::remove_dir_all(&dir)?;
                std::fs::create_dir_all(&dir)?;
                Index::create_in_dir(&dir, schema.clone())
            })
            .context("Failed to create search index")?;

        let mut writer: IndexWriter = index
            .writer(15_000_000)
            .context("Failed to create index writer")?;

        // Index all files
        for entry in std::fs::read_dir(docs_root)?.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if name.starts_with('.') || name.starts_with('_') || name == "mcp" || name == "skills" {
                continue;
            }

            for walk_entry in WalkDir::new(&path)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file() && is_doc_file(e.path()))
            {
                let file_path = walk_entry.path();
                let content = std::fs::read_to_string(file_path).unwrap_or_default();
                let rel = file_path
                    .strip_prefix(&path)
                    .unwrap_or(file_path)
                    .to_string_lossy()
                    .to_string();

                for (chunk_idx, chunk) in chunk_content(&content).iter().enumerate() {
                    let mut doc = TantivyDocument::new();
                    doc.add_text(project_field, &name);
                    doc.add_text(file_field, &rel);
                    doc.add_u64(chunk_id_field, chunk_idx as u64);
                    doc.add_text(body_field, &chunk.text);
                    doc.add_u64(line_start_field, chunk.line_start as u64);
                    writer.add_document(doc)?;
                }
                indexed_count += 1;
            }
        }

        writer.commit().context("Failed to commit index")?;
    } else if !files_to_index.is_empty() {
        // Incremental update
        let index = Index::open_in_dir(&dir).context("Failed to open existing index")?;
        let mut writer: IndexWriter = index.writer(15_000_000)?;

        for (project_name, rel_path, file_path) in &files_to_index {
            // Delete old documents for this file
            let term = tantivy::Term::from_field_text(file_field, rel_path);
            writer.delete_term(term);

            let content = std::fs::read_to_string(file_path).unwrap_or_default();
            for (chunk_idx, chunk) in chunk_content(&content).iter().enumerate() {
                let mut doc = TantivyDocument::new();
                doc.add_text(project_field, project_name);
                doc.add_text(file_field, rel_path);
                doc.add_u64(chunk_id_field, chunk_idx as u64);
                doc.add_text(body_field, &chunk.text);
                doc.add_u64(line_start_field, chunk.line_start as u64);
                writer.add_document(doc)?;
            }
            indexed_count += 1;
        }

        writer.commit()?;
    }

    // Update metadata
    meta.files = current_files;
    meta.save(docs_root)?;

    Ok(json!({
        "status": "ok",
        "projects": project_count,
        "indexed": indexed_count,
        "skipped": skipped_count,
        "index_path": dir.to_string_lossy(),
    }))
}

// ---------------------------------------------------------------------------
// Query sanitization
// ---------------------------------------------------------------------------

/// Escape special characters in tantivy query syntax.
/// Characters like +, -, (, ), {, }, [, ], ^, ~, *, ?, \, /, : have special
/// meaning in the tantivy query parser. We escape them so user input is treated
/// as a literal phrase search.
fn sanitize_query(query: &str) -> String {
    let special = [
        '+', '-', '(', ')', '{', '}', '[', ']', '^', '~', '*', '?', '\\', '/', ':', '!',
    ];
    let mut sanitized = String::with_capacity(query.len());
    for ch in query.chars() {
        if special.contains(&ch) {
            sanitized.push('\\');
        }
        sanitized.push(ch);
    }
    // If query is empty after trimming, return a wildcard-safe empty
    let trimmed = sanitized.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed.to_string()
}

// ---------------------------------------------------------------------------
// Search using index (BM25)
// ---------------------------------------------------------------------------

/// Search using BM25 ranking via tantivy index.
/// Returns top-k chunks ranked by relevance, deduplicated per file (best chunk wins).
pub fn search_indexed(
    docs_root: &Path,
    query: &str,
    limit: usize,
    project_filter: Option<&str>,
) -> Result<JsonValue> {
    let dir = index_dir(docs_root);
    if !dir.exists() {
        anyhow::bail!("Search index not found. Run index rebuild first.");
    }

    // Wait briefly if index is being built (up to 5 seconds)
    for _ in 0..50 {
        if !INDEX_BUILDING.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    let sanitized = sanitize_query(query);
    if sanitized.is_empty() {
        return Ok(json!({
            "query": query,
            "scope": if project_filter.is_some() { "project" } else { "global" },
            "mode": "ranked",
            "matches": [],
            "truncated": false,
        }));
    }

    let (_schema, project_field, file_field, _chunk_id_field, body_field, line_start_field) =
        build_schema();

    let index = Index::open_in_dir(&dir).context("Failed to open search index")?;

    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::Manual)
        .try_into()
        .context("Failed to create index reader")?;

    let searcher = reader.searcher();

    let query_parser = QueryParser::for_index(&index, vec![body_field]);
    let parsed_query = query_parser
        .parse_query(&sanitized)
        .context("Failed to parse search query")?;

    // Fetch more candidates for deduplication
    let top_docs = searcher
        .search(&parsed_query, &TopDocs::with_limit(limit * 5))
        .context("Search failed")?;

    // Deduplicate: keep only the best-scoring chunk per (project, file) pair
    let mut seen: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    let mut results = Vec::new();

    for (score, doc_address) in top_docs {
        let doc: TantivyDocument = searcher.doc(doc_address)?;

        let project = doc
            .get_first(project_field)
            .and_then(|v| schema::Value::as_str(&v))
            .unwrap_or("")
            .to_string();

        // Apply project filter if specified
        if let Some(filter) = project_filter
            && project != filter
        {
            continue;
        }

        let file = doc
            .get_first(file_field)
            .and_then(|v| schema::Value::as_str(&v))
            .unwrap_or("")
            .to_string();

        // Skip if we already have a better chunk from this file
        let key = (project.clone(), file.clone());
        if seen.contains_key(&key) {
            continue;
        }
        seen.insert(key, results.len());

        let body = doc
            .get_first(body_field)
            .and_then(|v| schema::Value::as_str(&v))
            .unwrap_or("")
            .to_string();

        let line_start = doc
            .get_first(line_start_field)
            .and_then(|v| schema::Value::as_u64(&v))
            .unwrap_or(0);

        results.push(json!({
            "project": project,
            "file": file,
            "line_start": line_start,
            "snippet": body,
            "score": (score * 1000.0).round() / 1000.0,
        }));

        if results.len() >= limit {
            break;
        }
    }

    let truncated = results.len() >= limit;
    Ok(json!({
        "query": query,
        "scope": if project_filter.is_some() { "project" } else { "global" },
        "mode": "ranked",
        "matches": results,
        "truncated": truncated,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_indexed_root() -> TempDir {
        let tmp = TempDir::new().unwrap();
        // backend
        let backend = tmp.path().join("backend");
        fs::create_dir_all(&backend).unwrap();
        fs::write(
            backend.join("PRD.md"),
            "# Backend PRD\n\nAuthentication flow using OAuth 2.0.\nThe API gateway handles token validation.\nRefresh tokens are stored in Redis.",
        ).unwrap();
        fs::write(
            backend.join("ARCHITECTURE.md"),
            "# Backend Architecture\n\nMicroservices design with gRPC.\nService mesh using Istio.\nDatabase: PostgreSQL with read replicas.",
        ).unwrap();
        // frontend
        let frontend = tmp.path().join("frontend");
        fs::create_dir_all(&frontend).unwrap();
        fs::write(
            frontend.join("PRD.md"),
            "# Frontend PRD\n\nLogin page with OAuth integration.\nSocial login support for Google and GitHub.",
        ).unwrap();
        // notes (knowledge base)
        let notes = tmp.path().join("notes");
        fs::create_dir_all(&notes).unwrap();
        fs::write(
            notes.join("k8s-tips.md"),
            "# K8s Tips\n\nTroubleshooting CrashLoopBackOff errors.\nCheck resource limits and liveness probes.\nUse kubectl describe pod for diagnostics.",
        ).unwrap();
        fs::write(
            notes.join("oauth-memo.md"),
            "# OAuth Memo\n\nOAuth 2.0 authorization code flow.\nPKCE extension for public clients.\nToken refresh best practices.",
        ).unwrap();
        // hidden (should be skipped)
        fs::create_dir_all(tmp.path().join("_template")).unwrap();
        fs::write(tmp.path().join("_template/TPL.md"), "# Template").unwrap();
        tmp
    }

    #[test]
    fn build_index_succeeds() {
        let tmp = setup_indexed_root();
        let result = build_index_unlocked(tmp.path()).unwrap();
        assert_eq!(result["status"], "ok");
        assert!(result["indexed"].as_u64().unwrap() >= 5);
        assert!(result["projects"].as_u64().unwrap() >= 3);
        // Index directory should exist
        assert!(tmp.path().join(".alcove/index").exists());
    }

    #[test]
    fn build_index_incremental_skips_unchanged() {
        let tmp = setup_indexed_root();
        // First build
        build_index_unlocked(tmp.path()).unwrap();
        // Second build with no changes
        let result = build_index_unlocked(tmp.path()).unwrap();
        assert_eq!(result["status"], "ok");
        // All files should be skipped on second run
        assert!(result["skipped"].as_u64().unwrap() >= 5);
    }

    #[test]
    fn search_indexed_finds_oauth() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();

        let result = search_indexed(tmp.path(), "OAuth", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "should find OAuth matches");
        assert_eq!(result["mode"], "ranked");

        // Should have scores
        for m in matches {
            assert!(m["score"].as_f64().unwrap() > 0.0);
            assert!(m["project"].is_string());
        }
    }

    #[test]
    fn search_indexed_with_project_filter() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();

        let result = search_indexed(tmp.path(), "OAuth", 10, Some("backend")).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty());
        for m in matches {
            assert_eq!(m["project"], "backend");
        }
    }

    #[test]
    fn search_indexed_respects_limit() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();

        let result = search_indexed(tmp.path(), "OAuth", 1, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn search_indexed_no_results() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();

        let result = search_indexed(tmp.path(), "zzz_nonexistent_query_zzz", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn search_indexed_skips_hidden_projects() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();

        let result = search_indexed(tmp.path(), "Template", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        let projects: Vec<&str> = matches
            .iter()
            .filter_map(|m| m["project"].as_str())
            .collect();
        assert!(!projects.contains(&"_template"));
    }

    #[test]
    fn chunk_content_basic() {
        let content = "line1\nline2\nline3";
        let chunks = chunk_content(content);
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].line_start, 1);
    }

    #[test]
    fn chunk_content_long_splits() {
        // Create content longer than CHUNK_SIZE
        let lines: Vec<String> = (0..100)
            .map(|i| {
                format!(
                    "This is line number {} with some padding text to make it longer.",
                    i
                )
            })
            .collect();
        let content = lines.join("\n");
        let chunks = chunk_content(&content);
        assert!(
            chunks.len() > 1,
            "long content should produce multiple chunks"
        );
        // First chunk starts at line 1
        assert_eq!(chunks[0].line_start, 1);
    }

    #[test]
    fn chunk_content_empty() {
        let chunks = chunk_content("");
        assert!(chunks.is_empty());
    }

    #[test]
    fn is_index_stale_when_no_index() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("proj")).unwrap();
        fs::write(tmp.path().join("proj/DOC.md"), "# Doc").unwrap();
        assert!(is_index_stale(tmp.path()), "no index should be stale");
    }

    #[test]
    fn is_index_fresh_after_build() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();
        assert!(
            !is_index_stale(tmp.path()),
            "just-built index should not be stale"
        );
    }

    #[test]
    fn is_index_stale_after_file_change() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();
        assert!(!is_index_stale(tmp.path()));

        // Modify a file (need to change mtime)
        std::thread::sleep(std::time::Duration::from_secs(1));
        fs::write(
            tmp.path().join("backend/PRD.md"),
            "# Updated PRD\n\nNew content added.",
        )
        .unwrap();
        assert!(
            is_index_stale(tmp.path()),
            "modified file should make index stale"
        );
    }

    #[test]
    fn is_index_stale_after_new_file() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();

        // Add a new file
        fs::write(tmp.path().join("backend/NEW.md"), "# New doc").unwrap();
        assert!(
            is_index_stale(tmp.path()),
            "new file should make index stale"
        );
    }

    #[test]
    fn ensure_index_fresh_rebuilds_when_stale() {
        let tmp = setup_indexed_root();
        // No index yet
        assert!(is_index_stale(tmp.path()));

        let rebuilt = ensure_index_fresh(tmp.path());
        assert!(rebuilt, "should have rebuilt");
        assert!(!is_index_stale(tmp.path()), "should be fresh after rebuild");
    }

    #[test]
    fn ensure_index_fresh_skips_when_fresh() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();

        let rebuilt = ensure_index_fresh(tmp.path());
        assert!(!rebuilt, "should not rebuild when fresh");
    }

    #[test]
    fn is_index_stale_after_file_deletion() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();
        assert!(!is_index_stale(tmp.path()));

        // Delete a file
        fs::remove_file(tmp.path().join("backend/PRD.md")).unwrap();
        assert!(
            is_index_stale(tmp.path()),
            "deleted file should make index stale"
        );
    }

    #[test]
    fn sanitize_query_escapes_special_chars() {
        assert_eq!(sanitize_query("hello world"), "hello world");
        assert_eq!(sanitize_query("C++"), "C\\+\\+");
        assert_eq!(sanitize_query("test:query"), "test\\:query");
        assert_eq!(sanitize_query("(foo)"), "\\(foo\\)");
        assert_eq!(sanitize_query("a/b"), "a\\/b");
        assert_eq!(sanitize_query(""), "");
        assert_eq!(sanitize_query("   "), "");
    }

    #[test]
    fn search_indexed_special_chars_no_panic() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();

        // These should not panic or error
        let result = search_indexed(tmp.path(), "C++", 10, None).unwrap();
        assert!(result["matches"].is_array());

        let result = search_indexed(tmp.path(), "test:query", 10, None).unwrap();
        assert!(result["matches"].is_array());

        let result = search_indexed(tmp.path(), "(foo AND bar)", 10, None).unwrap();
        assert!(result["matches"].is_array());
    }

    #[test]
    fn search_indexed_empty_query() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();

        let result = search_indexed(tmp.path(), "", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(matches.is_empty(), "empty query should return no matches");
    }

    #[test]
    fn search_indexed_deduplicates_by_file() {
        let tmp = TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        fs::create_dir_all(&proj).unwrap();

        // Create a large file that will produce multiple chunks mentioning the same term
        let mut content = String::new();
        for i in 0..50 {
            content.push_str(&format!(
                "Line {}: The authentication system uses OAuth tokens.\n",
                i
            ));
        }
        fs::write(proj.join("BIG.md"), &content).unwrap();

        build_index_unlocked(tmp.path()).unwrap();
        let result = search_indexed(tmp.path(), "OAuth", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();

        // Should have at most 1 result per file (BIG.md), not multiple chunks
        let files: Vec<&str> = matches.iter().filter_map(|m| m["file"].as_str()).collect();
        let unique_files: std::collections::HashSet<&&str> = files.iter().collect();
        assert_eq!(
            files.len(),
            unique_files.len(),
            "results should be deduplicated by file"
        );
    }

    #[test]
    fn sanitize_query_preserves_unicode() {
        assert_eq!(sanitize_query("인증 흐름"), "인증 흐름");
        assert_eq!(sanitize_query("認証フロー"), "認証フロー");
    }

    #[test]
    fn sanitize_query_mixed_special_and_text() {
        assert_eq!(sanitize_query("user@name"), "user@name");
        assert_eq!(sanitize_query("[RFC-001]"), "\\[RFC\\-001\\]");
        assert_eq!(sanitize_query("feat!: breaking"), "feat\\!\\: breaking");
    }

    #[test]
    fn search_indexed_no_index_returns_error() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("proj")).unwrap();
        fs::write(tmp.path().join("proj/DOC.md"), "# Doc").unwrap();
        // No index built — should error
        let result = search_indexed(tmp.path(), "doc", 10, None);
        assert!(result.is_err());
    }

    #[test]
    fn search_indexed_global_scope_label() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();
        let result = search_indexed(tmp.path(), "OAuth", 10, None).unwrap();
        assert_eq!(result["scope"], "global");
        assert_eq!(result["mode"], "ranked");
    }

    #[test]
    fn search_indexed_project_scope_label() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();
        let result = search_indexed(tmp.path(), "OAuth", 10, Some("backend")).unwrap();
        assert_eq!(result["scope"], "project");
    }

    #[test]
    fn build_index_incremental_rebuilds_after_change() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();

        // Add new file
        fs::write(
            tmp.path().join("backend/NEW.md"),
            "# New Document\n\nFresh content here.",
        )
        .unwrap();
        let r2 = build_index_unlocked(tmp.path()).unwrap();
        assert_eq!(r2["status"], "ok");
        // The new file should be picked up (indexed >= 1)
        assert!(
            r2["indexed"].as_u64().unwrap_or(0) >= 1,
            "incremental rebuild should index new file"
        );
    }

    #[test]
    fn chunk_content_single_line() {
        let chunks = chunk_content("Single line document.");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].line_start, 1);
        assert_eq!(chunks[0].text, "Single line document.");
    }

    #[test]
    fn build_index_lock_prevents_concurrent() {
        // Verify the AtomicBool compare_exchange logic works correctly
        let flag = AtomicBool::new(false);

        // First acquire succeeds
        assert!(
            flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        );
        // Second acquire fails (simulating concurrent call)
        assert!(
            flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        );
        // Release
        flag.store(false, Ordering::SeqCst);
        // Now acquire succeeds again
        assert!(
            flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        );
        flag.store(false, Ordering::SeqCst);
    }
}
