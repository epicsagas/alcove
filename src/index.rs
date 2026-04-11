use std::path::{Path, PathBuf};
use std::time::{SystemTime, Duration};
use std::os::unix::io::AsRawFd;

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{self, *};
use tantivy::{DocAddress, Score};
use tantivy::tokenizer::{LowerCaser, NgramTokenizer, TextAnalyzer};
use tantivy::{Index, ReloadPolicy, TantivyDocument};
use walkdir::WalkDir;

use crate::config::{effective_config, load_config};

const NGRAM_TOKENIZER: &str = "cjk_ngram";

// ---------------------------------------------------------------------------
// Index lock — prevents concurrent build/search races per docs_root
// ---------------------------------------------------------------------------

/// Maximum age (in seconds) for a lock file before it is considered stale.
/// If the lock holder crashes, the lock will be auto-cleared after this duration.
const LOCK_STALE_SECS: u64 = 600; // 10 minutes

fn lock_file(docs_root: &Path) -> PathBuf {
    docs_root.join(".alcove").join(".index_lock")
}

fn try_acquire_lock(docs_root: &Path) -> bool {
    let lock_path = lock_file(docs_root);
    if let Some(parent) = lock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // If a stale lock exists, remove it first
    if lock_path.exists() && is_lock_stale(&lock_path) {
        let _ = std::fs::remove_file(&lock_path);
    }
    if std::fs::File::create_new(&lock_path).is_ok() {
        // Write PID so we can detect stale locks from dead processes
        let _ = std::fs::write(&lock_path, std::process::id().to_string());
        return true;
    }
    false
}

fn release_lock(docs_root: &Path) {
    let _ = std::fs::remove_file(lock_file(docs_root));
}

fn is_locked(docs_root: &Path) -> bool {
    let path = lock_file(docs_root);
    if !path.exists() {
        return false;
    }
    // Treat stale locks as not locked
    if is_lock_stale(&path) {
        let _ = std::fs::remove_file(&path);
        return false;
    }
    true
}

/// A lock is stale if it is older than `LOCK_STALE_SECS` or its PID is no longer running.
fn is_lock_stale(lock_path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(lock_path) else {
        return false;
    };

    // Check age
    if let Ok(modified) = meta.modified()
        && let Ok(elapsed) = modified.elapsed()
        && elapsed.as_secs() > LOCK_STALE_SECS
    {
        return true;
    }

    // Check if PID is still alive (Unix: kill -0)
    #[cfg(unix)]
    {
        if let Ok(content) = std::fs::read_to_string(lock_path)
            && let Ok(pid) = content.trim().parse::<u32>()
        {
            let status = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            if let Ok(s) = status
                && !s.success()
            {
                return true; // Process doesn't exist
            }
        }
    }

    false
}

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
    let body_indexing = TextFieldIndexing::default()
        .set_tokenizer(NGRAM_TOKENIZER)
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let body_options = TextOptions::default()
        .set_indexing_options(body_indexing)
        .set_stored();
    let body = builder.add_text_field("body", body_options);
    let line_start = builder.add_u64_field("line_start", STORED);
    (builder.build(), project, file, chunk_id, body, line_start)
}

fn register_ngram_tokenizer(index: &Index) -> Result<()> {
    let ngram = TextAnalyzer::builder(NgramTokenizer::new(2, 3, false).map_err(|e| {
        anyhow::anyhow!("Failed to create NgramTokenizer: {}", e)
    })?)
    .filter(LowerCaser)
    .build();
    index.tokenizers().register(NGRAM_TOKENIZER, ngram);
    Ok(())
}

// ---------------------------------------------------------------------------
// Chunking
// ---------------------------------------------------------------------------

const CHUNK_SIZE: usize = 1500; // chars per chunk (prose / markdown)
const CHUNK_OVERLAP: usize = 300; // overlap between chunks (prose)

/// Smaller limits for source-code files — keeps chunks inside function boundaries.
const CODE_CHUNK_SIZE: usize = 800;
const CODE_CHUNK_OVERLAP: usize = 150;

/// File extensions that receive code-aware (smaller) chunking.
fn is_code_ext(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "go"
            | "py"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "java"
            | "cpp"
            | "cc"
            | "c"
            | "h"
            | "hpp"
            | "cs"
            | "rb"
            | "swift"
            | "kt"
            | "kts"
            | "scala"
            | "ex"
            | "exs"
            | "zig"
            | "lua"
            | "sh"
            | "bash"
            | "zsh"
    )
}

struct Chunk {
    text: String,
    line_start: usize,
}

/// Chunk `content` using sensible size limits for the given file extension.
///
/// Code files use smaller chunks (800 chars / 150 overlap) so function
/// bodies are less likely to be split across chunk boundaries.
/// All other files use the default prose limits (1 500 / 300).
fn chunk_content(content: &str, ext: &str) -> Vec<Chunk> {
    let (chunk_size, overlap_size) = if is_code_ext(ext) {
        (CODE_CHUNK_SIZE, CODE_CHUNK_OVERLAP)
    } else {
        (CHUNK_SIZE, CHUNK_OVERLAP)
    };

    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return vec![];
    }

    let mut chunks = Vec::new();
    let mut current_chars = 0;
    let mut chunk_lines: Vec<String> = Vec::new();
    let mut chunk_start_line = 0;

    for (i, line) in lines.iter().enumerate() {
        let line_len = line.chars().count().saturating_add(1);
        if current_chars + line_len > chunk_size && !chunk_lines.is_empty() {
            chunks.push(Chunk {
                text: chunk_lines.join("\n"),
                line_start: chunk_start_line + 1,
            });

            let mut kept: usize = 0;
            let mut keep_from = chunk_lines.len();
            for (j, cl) in chunk_lines.iter().enumerate().rev() {
                kept = kept.saturating_add(cl.chars().count().saturating_add(1));
                if kept >= overlap_size {
                    keep_from = j;
                    break;
                }
            }
            let overlap_lines: Vec<String> = chunk_lines[keep_from..].to_vec();
            chunk_start_line = i - overlap_lines.len();
            chunk_lines = overlap_lines;
            current_chars = chunk_lines
                .iter()
                .map(|l: &String| l.chars().count().saturating_add(1))
                .sum();
        }

        chunk_lines.push(line.to_string());
        current_chars = current_chars.saturating_add(line_len);
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
    files: std::collections::HashMap<String, [u64; 2]>, // path -> [mtime_secs, size]
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
        // Atomic write: write to a temp file then rename so a mid-write crash
        // never corrupts the existing meta file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

fn file_fingerprint(path: &Path) -> [u64; 2] {
    match std::fs::metadata(path) {
        Ok(m) => {
            let mtime_secs = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let size = m.len();
            [mtime_secs, size]
        }
        Err(_) => [0, 0],
    }
}

/// Check if a search index exists for the given docs_root.
pub fn index_exists(docs_root: &Path) -> bool {
    meta_path(docs_root).exists()
}

/// Return detailed change report: added, modified, deleted files since last index build.
pub fn check_doc_changes(docs_root: &Path) -> JsonValue {
    let meta = IndexMeta::load(docs_root);
    let has_index = meta_path(docs_root).exists();

    let mut added: Vec<String> = Vec::new();
    let mut modified: Vec<String> = Vec::new();
    let mut unchanged: u64 = 0;
    let mut current_files: std::collections::HashSet<String> = std::collections::HashSet::new();

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
        let proj_cfg = effective_config(&path);
        for walk_entry in WalkDir::new(&path)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_file() && proj_cfg.is_indexable(e.path()))
        {
            let file_path = walk_entry.path();
            let rel = file_path
                .strip_prefix(docs_root)
                .unwrap_or(file_path)
                .to_string_lossy()
                .to_string();
            let fp = file_fingerprint(file_path);
            current_files.insert(rel.clone());

            match meta.files.get(&rel) {
                None => added.push(rel),
                Some(&recorded) if recorded != fp => modified.push(rel),
                _ => unchanged += 1,
            }
        }
    }

    let deleted: Vec<String> = meta
        .files
        .keys()
        .filter(|k| !current_files.contains(*k))
        .cloned()
        .collect();

    let is_stale = !added.is_empty() || !modified.is_empty() || !deleted.is_empty();

    json!({
        "index_exists": has_index,
        "is_stale": is_stale,
        "added": added,
        "modified": modified,
        "deleted": deleted,
        "unchanged_count": unchanged,
        "total_indexed": meta.files.len(),
    })
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
        let proj_cfg = effective_config(&path);
        for walk_entry in WalkDir::new(&path)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_file() && proj_cfg.is_indexable(e.path()))
        {
            let file_path = walk_entry.path();
            let rel = file_path
                .strip_prefix(docs_root)
                .unwrap_or(file_path)
                .to_string_lossy()
                .to_string();
            let fp = file_fingerprint(file_path);
            current_files.insert(rel.clone());
            match meta.files.get(&rel) {
                Some(&recorded) if recorded == fp => {}
                _ => return true,
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

/// Helper to extract text from XML tags (e.g., w:t for Word, a:t for PPT)
#[cfg(feature = "alcove-full")]
fn extract_xml_text(content: &str, tag_name: &[u8]) -> Result<String> {
    use quick_xml::reader::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(content);
    let mut text = String::new();
    let mut in_tag = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().as_ref() == tag_name => {
                in_tag = true;
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == tag_name => {
                in_tag = false;
            }
            Ok(Event::Text(e)) if in_tag => {
                if let Ok(s) = std::str::from_utf8(&e.into_inner()) {
                    text.push_str(
                        &quick_xml::escape::unescape(s).unwrap_or(std::borrow::Cow::Borrowed(s))
                    );
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow::anyhow!("XML parse error: {}", e)),
            _ => {}
        }
    }
    Ok(text)
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

/// Read file content, extracting text from PDF/DOCX if needed.
fn read_file_content(path: &Path) -> Result<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    
    match ext.as_str() {
        #[cfg(feature = "alcove-full")]
        "pdf" => {
            // pdf_extract prints unicode fallback noise to both stdout and stderr — suppress both
            let devnull = std::fs::File::open("/dev/null")
                .map_err(|e| anyhow::anyhow!("Failed to open /dev/null: {}", e))?;
            let devnull_fd = devnull.as_raw_fd();
            let saved_stdout = unsafe { libc::dup(libc::STDOUT_FILENO) };
            let saved_stderr = unsafe { libc::dup(libc::STDERR_FILENO) };
            unsafe {
                libc::dup2(devnull_fd, libc::STDOUT_FILENO);
                libc::dup2(devnull_fd, libc::STDERR_FILENO);
            }
            let result = pdf_extract::extract_text(path)
                .map_err(|e| anyhow::anyhow!("Failed to extract PDF: {}", e));
            unsafe {
                libc::dup2(saved_stdout, libc::STDOUT_FILENO);
                libc::dup2(saved_stderr, libc::STDERR_FILENO);
                libc::close(saved_stdout);
                libc::close(saved_stderr);
            }
            result
        }
        #[cfg(feature = "alcove-full")]
        "docx" | "pptx" => {
            use std::io::Read;
            let file = std::fs::File::open(path)?;
            let mut archive = zip::ZipArchive::new(file)
                .map_err(|e| anyhow::anyhow!("Failed to open {} (ZIP): {}", ext, e))?;
            
            let mut text = String::new();
            
            if ext == "docx" {
                let mut doc_xml = archive.by_name("word/document.xml")
                    .map_err(|e| anyhow::anyhow!("Failed to find word/document.xml in DOCX: {}", e))?;
                let mut content = String::new();
                doc_xml.read_to_string(&mut content)?;
                text = extract_xml_text(&content, b"w:t")?;
            } else {
                // PPTX: iterate through slides
                let mut slide_names: Vec<String> = archive.file_names()
                    .filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))
                    .map(|n| n.to_string())
                    .collect();
                slide_names.sort_by_key(|n| {
                    n.trim_start_matches("ppt/slides/slide")
                     .trim_end_matches(".xml")
                     .parse::<u32>().unwrap_or(0)
                });

                for name in slide_names {
                    let mut slide_xml = archive.by_name(&name)?;
                    let mut content = String::new();
                    slide_xml.read_to_string(&mut content)?;
                    let slide_text = extract_xml_text(&content, b"a:t")?;
                    if !slide_text.is_empty() {
                        text.push_str(&format!("\n--- Slide {} ---\n", name));
                        text.push_str(&slide_text);
                    }
                }
            }
            Ok(text)
        }
        #[cfg(feature = "alcove-full")]
        "xlsx" | "csv" => {
            use calamine::{Reader, open_workbook_auto};
            let mut workbook = open_workbook_auto(path)
                .map_err(|e| anyhow::anyhow!("Failed to open Excel/CSV: {}", e))?;
            
            let mut text = String::new();
            // Process all sheets
            for sheet_name in workbook.sheet_names().to_owned() {
                if let Ok(range) = workbook.worksheet_range(&sheet_name) {
                    text.push_str(&format!("\n--- Sheet: {} ---\n", sheet_name));
                    for row in range.rows() {
                        let row_text: Vec<String> = row.iter().map(|c| match c {
                            calamine::Data::Empty => "".to_string(),
                            calamine::Data::String(s) => s.clone(),
                            calamine::Data::Float(f) => f.to_string(),
                            calamine::Data::Int(i) => i.to_string(),
                            calamine::Data::Bool(b) => b.to_string(),
                            _ => "".to_string(),
                        }).collect();
                        text.push_str(&row_text.join("\t"));
                        text.push('\n');
                    }
                }
            }
            Ok(text)
        }
        _ => std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read file {}: {}", path.display(), e)),
    }
}

// ---------------------------------------------------------------------------
// Build / rebuild index
// ---------------------------------------------------------------------------

pub fn build_index(docs_root: &Path) -> Result<JsonValue> {
    build_index_with_mode(docs_root, false)
}

pub fn rebuild_index(docs_root: &Path) -> Result<JsonValue> {
    build_index_with_mode(docs_root, true)
}

fn build_index_with_mode(docs_root: &Path, force_rebuild: bool) -> Result<JsonValue> {
    if !try_acquire_lock(docs_root) {
        return Ok(json!({
            "status": "skipped",
            "reason": "Index build already in progress",
        }));
    }
    if force_rebuild {
        let dir = index_dir(docs_root);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        let meta_path = docs_root.join(".alcove").join("index_meta.json");
        if meta_path.exists() {
            std::fs::remove_file(&meta_path)?;
        }
        // Also clear vector store so embeddings are re-indexed
        let vectors_path = docs_root.join(".alcove").join("vectors.db");
        if vectors_path.exists() {
            std::fs::remove_file(&vectors_path)?;
        }
    }
    let result = build_index_inner(docs_root);
    release_lock(docs_root);
    result
}

#[cfg(test)]
pub fn build_index_unlocked(docs_root: &Path) -> Result<JsonValue> {
    build_index_inner(docs_root)
}

fn build_index_inner(docs_root: &Path) -> Result<JsonValue> {
    let dir = index_dir(docs_root);
    std::fs::create_dir_all(&dir)?;

    // fun_messages removed — progress now shows project/file type label only

    let (schema, project_field, file_field, chunk_id_field, body_field, line_start_field) =
        build_schema();

    let mut meta = IndexMeta::load(docs_root);
    let mut indexed_count = 0u64;
    let mut skipped_count = 0u64;
    let mut project_count = 0u64;

    // 1. Scan for all potential files
    let mut all_files: Vec<(String, String, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(docs_root)
        .context("Failed to read DOCS_ROOT")?
        .flatten()
    {
        let path = entry.path();
        if !path.is_dir() { continue; }
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        if name.starts_with('.') || name.starts_with('_') || name == "mcp" || name == "skills" { continue; }
        project_count += 1;

        let proj_cfg = effective_config(&path);
        for walk_entry in WalkDir::new(&path).into_iter().flatten().filter(|e| e.file_type().is_file() && proj_cfg.is_indexable(e.path())) {
            let file_path = walk_entry.path().to_path_buf();
            let rel_to_project = file_path.strip_prefix(&path).unwrap_or(&file_path).to_string_lossy().to_string();
            all_files.push((name.clone(), rel_to_project, file_path));
        }
    }

    // 2. Filter changed files
    let mut current_files: std::collections::HashMap<String, [u64; 2]> = std::collections::HashMap::new();
    let mut files_to_index: Vec<(String, String, PathBuf)> = Vec::new();

    for (proj, rel_to_proj, full_path) in all_files {
        let rel_to_root = full_path.strip_prefix(docs_root).unwrap_or(&full_path).to_string_lossy().to_string();
        let fp = file_fingerprint(&full_path);
        current_files.insert(rel_to_root.clone(), fp);

        if meta.files.get(&rel_to_root).copied() == Some(fp) {
            skipped_count += 1;
        } else {
            files_to_index.push((proj, rel_to_proj, full_path));
        }
    }

    let needs_full_rebuild = !dir.join("meta.json").exists() || meta.files.is_empty();

    if !files_to_index.is_empty() {
        let pb = ProgressBar::new(files_to_index.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("  {spinner:.cyan}  {bar:35.white/dim}  {pos:>3}/{len}  {msg:.dim}")?
                .progress_chars("━━╌"),
        );
        pb.enable_steady_tick(Duration::from_millis(80));

        let index = if needs_full_rebuild {
            pb.set_message("initializing...");
            Index::create_in_dir(&dir, schema.clone())
                .or_else(|_| {
                    std::fs::remove_dir_all(&dir)?;
                    std::fs::create_dir_all(&dir)?;
                    Index::create_in_dir(&dir, schema.clone())
                })?
        } else {
            Index::open_in_dir(&dir)?
        };
        register_ngram_tokenizer(&index)?;
        let mut writer = index.writer(load_config().index_buffer_bytes())?;

        for (proj, rel, full) in &files_to_index {
            let ext = full.extension().and_then(|e| e.to_str()).unwrap_or("md").to_lowercase();
            let label = match ext.as_str() {
                "pdf"  => format!("{proj}  pdf"),
                "docx" => format!("{proj}  docx"),
                "xlsx" => format!("{proj}  xlsx"),
                _      => proj.clone(),
            };
            pb.set_message(label);

            if !needs_full_rebuild {
                let term = tantivy::Term::from_field_text(file_field, rel);
                writer.delete_term(term);
            }

            if let Ok(content) = read_file_content(full) {
                // Generate .ir.json sidecar for markdown files (only if stale or missing)
                if full.extension().and_then(|e| e.to_str()) == Some("md") {
                    let sidecar_path = full.with_extension("ir.json");
                    let needs_regen = match (full.metadata(), sidecar_path.metadata()) {
                        (Ok(md_meta), Ok(ir_meta)) => {
                            md_meta.modified().ok() > ir_meta.modified().ok()
                        }
                        _ => true,
                    };
                    if needs_regen {
                        let ir_doc = ai_ir::Transpiler::from_markdown(
                            full.to_str().unwrap_or(""),
                            &content,
                        );
                        if let Ok(json) = ir_doc.to_json() {
                            let _ = std::fs::write(&sidecar_path, json);
                        }
                    }
                }

                let file_ext = full.extension().and_then(|e| e.to_str()).unwrap_or("");
                for (chunk_idx, chunk) in chunk_content(&content, file_ext).iter().enumerate() {
                    let mut doc = TantivyDocument::new();
                    doc.add_text(project_field, proj);
                    doc.add_text(file_field, rel);
                    doc.add_u64(chunk_id_field, chunk_idx as u64);
                    doc.add_text(body_field, &chunk.text);
                    doc.add_u64(line_start_field, chunk.line_start as u64);
                    writer.add_document(doc)?;
                }
            }
            indexed_count += 1;
            pb.inc(1);
        }
        
        pb.set_message("saving...");
        writer.commit()?;
        pb.finish_and_clear();
    }

    // ---------------------------------------------------------------------------
    // Vector indexing (alcove-full feature)
    // ---------------------------------------------------------------------------

    let mut vector_status = "disabled".to_string();
    let mut vectors_indexed = 0u64;
    let mut vector_errors = 0u64;
    let mut embedding_model = String::new();

    #[cfg(feature = "alcove-full")]
    {
        use crate::embedding::{EmbeddingModelChoice, EmbeddingService};
        use crate::vector::VectorStore;
        use crate::config::load_config;

        let cfg = load_config();
        let emb_cfg = cfg.embedding_config_with_defaults();

        if emb_cfg.enabled {
            let model = EmbeddingModelChoice::parse(&emb_cfg.model).unwrap_or_default();
            embedding_model = model.as_str().to_string();
            
            let service = EmbeddingService::new(crate::config::EmbeddingConfig {
                model: model.as_str().to_string(),
                auto_download: emb_cfg.auto_download,
                cache_dir: emb_cfg.cache_dir.clone(),
                enabled: true,
            });

            // Ensure model is loaded (and downloaded if auto_download is enabled)
            if emb_cfg.auto_download {
                let _ = service.ensure_model();
            }

            if service.state() == crate::embedding::ModelState::Ready {
                let vector_path = docs_root.join(".alcove").join("vectors.db");
                match VectorStore::open(&vector_path, service.model_name(), service.dimension()) {
                    Ok(mut store) => {
                        vector_status = "ok".to_string();
                        if !files_to_index.is_empty() {
                            // Filter out already-indexed files (sequential DB read)
                            let to_embed: Vec<(String, String, PathBuf)> = files_to_index
                                .into_iter()
                                .filter(|(proj, rel, _)| {
                                    !matches!(store.has_file(proj, rel), Ok(true))
                                })
                                .collect();

                            let total_files = to_embed.len() as u64;

                            let vpb = ProgressBar::new(total_files);
                            vpb.set_style(
                                ProgressStyle::default_bar()
                                    .template("  {spinner:.magenta}  {bar:35.white/dim}  {pos:>3}/{len}  {msg:.dim}")?
                                    .progress_chars("━━╌"),
                            );
                            vpb.enable_steady_tick(Duration::from_millis(80));
                            vpb.set_message("embedding");

                            // Sequential read + chunk per file, embed in batches.
                            // Parallel file reads caused RSS spikes (large PDFs × N threads).
                            // Bottleneck is ONNX inference, not I/O — sequential reads keep
                            // memory bounded to one file's chunks at a time.
                            //
                            // Batch size is tuned to model size:
                            //   small models (< 100 MB)  → 128  (VRAM/RAM headroom)
                            //   medium models (≤ 800 MB) → 64   (default)
                            //   large models  (> 800 MB) → 32   (OOM guard)
                            let embed_batch: usize = match model.size_mb() {
                                s if s < 100 => 128,
                                s if s <= 800 => 64,
                                _ => 32,
                            };
                            let mut pending: Vec<(String, String, u64, String)> = Vec::new();

                            for (proj, rel, full) in &to_embed {
                                vpb.set_message(proj.clone());

                                if let Ok(content) = read_file_content(full) {
                                    let file_ext = full.extension().and_then(|e| e.to_str()).unwrap_or("");
                                    for (i, chunk) in chunk_content(&content, file_ext).into_iter().enumerate() {
                                        pending.push((proj.clone(), rel.clone(), i as u64, chunk.text));

                                        if pending.len() >= embed_batch {
                                            let texts: Vec<&str> = pending.iter().map(|(_, _, _, t)| t.as_str()).collect();
                                            match service.embed(&texts) {
                                                Ok(embeddings) => {
                                                    let it = pending.drain(..).zip(embeddings).map(|((p, r, id, _), emb)| (p, r, id, emb));
                                                    if let Err(e) = store.batch_upsert(it) {
                                                        eprintln!("[alcove] batch upsert failed: {}", e);
                                                        vector_errors += embed_batch as u64;
                                                    } else {
                                                        vectors_indexed += embed_batch as u64;
                                                    }
                                                }
                                                Err(e) => {
                                                    eprintln!("[alcove] embed failed: {}", e);
                                                    vector_errors += pending.len() as u64;
                                                    pending.clear();
                                                }
                                            }
                                        }
                                    }
                                }
                                vpb.inc(1);
                            }

                            // Flush remaining
                            if !pending.is_empty() {
                                let texts: Vec<&str> = pending.iter().map(|(_, _, _, t)| t.as_str()).collect();
                                match service.embed(&texts) {
                                    Ok(embeddings) => {
                                        let count = pending.len() as u64;
                                        let it = pending.drain(..).zip(embeddings).map(|((p, r, id, _), emb)| (p, r, id, emb));
                                        if let Err(e) = store.batch_upsert(it) {
                                            eprintln!("[alcove] batch upsert failed: {}", e);
                                            vector_errors += count;
                                        } else {
                                            vectors_indexed += count;
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("[alcove] embed failed: {}", e);
                                        vector_errors += pending.len() as u64;
                                    }
                                }
                            }

                            vpb.finish_and_clear();
                        }
                    }
                    Err(e) => {
                        eprintln!("[alcove] Failed to open vector store: {}", e);
                        vector_status = format!("error: {}", e);
                    }
                }
            } else {
                vector_status = "model_not_ready".to_string();
            }
        }
    }

    // Final metadata save after all indexing steps (Tantivy + Vector) are complete
    meta.files = current_files;
    let _ = meta.save(docs_root);

    Ok(json!({
        "status": "ok",
        "projects": project_count,
        "indexed": indexed_count,
        "skipped": skipped_count,
        "index_path": dir.to_string_lossy(),
        "vector_status": vector_status,
        "vectors_indexed": vectors_indexed,
        "vector_errors": vector_errors,
        "embedding_model": embedding_model,
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

    for _ in 0..50 {
        if !is_locked(docs_root) {
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

    let index = Index::open_in_dir(&dir).context("Failed to open search index")?;
    register_ngram_tokenizer(&index)?;

    let schema = index.schema();
    let project_field = schema.get_field("project").context("missing 'project' field")?;
    let file_field = schema.get_field("file").context("missing 'file' field")?;
    let chunk_id_field = schema.get_field("chunk_id").context("missing 'chunk_id' field")?;
    let body_field = schema.get_field("body").context("missing 'body' field")?;
    let line_start_field = schema.get_field("line_start").context("missing 'line_start' field")?;

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
    let top_docs: Vec<(Score, DocAddress)> = searcher
        .search(&parsed_query, &TopDocs::with_limit(limit * 5).order_by_score())
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

        let chunk_id = doc
            .get_first(chunk_id_field)
            .and_then(|v| schema::Value::as_u64(&v))
            .unwrap_or(0);

        results.push(json!({
            "project": project,
            "file": file,
            "chunk_id": chunk_id,
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
// Hybrid Search (BM25 + Vector)
// ---------------------------------------------------------------------------

/// Perform hybrid search combining BM25 and vector similarity
/// Returns results ranked by Reciprocal Rank Fusion (RRF)
#[cfg(feature = "alcove-full")]
pub fn search_hybrid(
    docs_root: &Path,
    query: &str,
    embedding_service: &crate::embedding::EmbeddingService,
    limit: usize,
    project_filter: Option<&str>,
) -> Result<JsonValue> {
    use crate::vector::{reciprocal_rank_fusion, VectorStore};

    // 1. Ensure index is fresh
    ensure_index_fresh(docs_root);

    // 2. Check embedding model state
    let model_state = embedding_service.state();
    let model_ready = model_state == crate::embedding::ModelState::Ready;

    // 3. Get BM25 results first (always available)
    let bm25_json = search_indexed(docs_root, query, limit * 2, project_filter)?;

    let bm25_results: Vec<(String, String, u64, f32)> = bm25_json["matches"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    Some((
                        m["project"].as_str()?.to_string(),
                        m["file"].as_str()?.to_string(),
                        m["chunk_id"].as_u64()?,
                        m["score"].as_f64()? as f32,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    // 4. If model not ready, return BM25-only with status
    if !model_ready {
        return Ok(json!({
            "query": query,
            "scope": if project_filter.is_some() { "project" } else { "global" },
            "mode": "bm25-only",
            "embedding_status": model_state.to_string(),
            "matches": bm25_json["matches"],
            "truncated": bm25_json["truncated"],
        }));
    }

    // 5. Generate query embedding
    let query_embedding = match embedding_service.embed(&[query]) {
        Ok(emb) => emb.into_iter().next().unwrap_or_default(),
        Err(e) => {
            return Ok(json!({
                "query": query,
                "scope": if project_filter.is_some() { "project" } else { "global" },
                "mode": "bm25-only",
                "embedding_status": format!("failed: {}", e),
                "matches": bm25_json["matches"],
                "truncated": bm25_json["truncated"],
            }));
        }
    };

    // 6. Open vector store and search
    let vector_path = docs_root.join(".alcove").join("vectors.db");
    let store = VectorStore::open(&vector_path, embedding_service.model_name(), embedding_service.dimension());

    let vector_results = match store {
        Ok(s) => match s.search(&query_embedding, limit * 2) {
            Ok(r) => r,
            Err(_) => Vec::new(),
        },
        Err(_) => Vec::new(),
    };

    // 7. Combine with RRF.
    // k scales with limit: smaller result sets benefit from a lower k which
    // spreads scores more aggressively; larger sets use the classic k=60.
    // Formula: k = max(10, round(60 * sqrt(limit / 50)))
    let rrf_k = ((60.0 * ((limit as f32) / 50.0).sqrt()).round() as u32).max(10);
    let fused = reciprocal_rank_fusion(&bm25_results, &vector_results, rrf_k);

    // 8. Build final results with snippets.
    //
    // BM25 results already carry `snippet` and `line_start`.  We cache them in
    // a HashMap so vector-only hits (chunk_ids absent from BM25) can still fall
    // back to a single Tantivy lookup — but the common case of BM25-overlap hits
    // requires zero additional index queries.
    //
    // Key: (project, file, chunk_id)  →  (snippet, line_start)
    type SnippetKey = (String, String, u64);
    let mut snippet_cache: std::collections::HashMap<SnippetKey, (String, u64)> =
        std::collections::HashMap::new();

    if let Some(arr) = bm25_json["matches"].as_array() {
        for m in arr {
            if let (Some(proj), Some(file), Some(cid), Some(snip), Some(ls)) = (
                m["project"].as_str(),
                m["file"].as_str(),
                m["chunk_id"].as_u64(),
                m["snippet"].as_str(),
                m["line_start"].as_u64(),
            ) {
                snippet_cache.insert(
                    (proj.to_string(), file.to_string(), cid),
                    (snip.to_string(), ls),
                );
            }
        }
    }

    // Open Tantivy only if there are vector-only hits that need a fallback lookup.
    // Lazily initialised below on first cache miss.
    let mut tantivy_searcher: Option<(
        tantivy::Searcher,
        Field,
        Field,
        Field,
        Field,
        Field,
    )> = None;

    let mut results: Vec<JsonValue> = Vec::new();

    for (project, file, chunk_id, rrf_score) in fused.into_iter().take(limit) {
        let (snippet, line_start) =
            if let Some(cached) = snippet_cache.get(&(project.clone(), file.clone(), chunk_id)) {
                cached.clone()
            } else {
                // Cache miss: vector-only hit — look up via Tantivy (rare path).
                let (ref searcher, project_field, file_field, body_field, line_start_field, chunk_id_field) =
                    *tantivy_searcher.get_or_insert_with(|| {
                        let index_path = index_dir(docs_root);
                        let index = Index::open_in_dir(&index_path).expect("open index");
                        let reader = index.reader().expect("index reader");
                        let searcher = reader.searcher();
                        let schema = index.schema();
                        let pf = schema.get_field("project").expect("project field");
                        let ff = schema.get_field("file").expect("file field");
                        let bf = schema.get_field("body").expect("body field");
                        let lf = schema.get_field("line_start").expect("line_start field");
                        let cf = schema.get_field("chunk_id").expect("chunk_id field");
                        (searcher, pf, ff, bf, lf, cf)
                    });

                let combined = tantivy::query::BooleanQuery::new(vec![
                    (
                        tantivy::query::Occur::Must,
                        Box::new(tantivy::query::TermQuery::new(
                            tantivy::Term::from_field_text(project_field, &project),
                            tantivy::schema::IndexRecordOption::Basic,
                        )) as Box<dyn tantivy::query::Query>,
                    ),
                    (
                        tantivy::query::Occur::Must,
                        Box::new(tantivy::query::TermQuery::new(
                            tantivy::Term::from_field_text(file_field, &file),
                            tantivy::schema::IndexRecordOption::Basic,
                        )),
                    ),
                    (
                        tantivy::query::Occur::Must,
                        Box::new(tantivy::query::TermQuery::new(
                            tantivy::Term::from_field_u64(chunk_id_field, chunk_id),
                            tantivy::schema::IndexRecordOption::Basic,
                        )),
                    ),
                ]);

                if let Ok(top_docs) =
                    searcher.search(&combined, &TopDocs::with_limit(1).order_by_score())
                {
                    if let Some((_s, addr)) = top_docs.first() {
                        if let Ok(doc) = searcher.doc::<TantivyDocument>(*addr) {
                            let body = doc
                                .get_first(body_field)
                                .and_then(|v| schema::Value::as_str(&v))
                                .unwrap_or("")
                                .to_string();
                            let ls = doc
                                .get_first(line_start_field)
                                .and_then(|v| schema::Value::as_u64(&v))
                                .unwrap_or(0);
                            (body, ls)
                        } else {
                            (String::new(), 0)
                        }
                    } else {
                        (String::new(), 0)
                    }
                } else {
                    (String::new(), 0)
                }
            };

        results.push(json!({
            "project": project,
            "file": file,
            "line_start": line_start,
            "snippet": snippet,
            "score": (rrf_score * 1000.0).round() / 1000.0,
        }));
    }

    Ok(json!({
        "query": query,
        "scope": if project_filter.is_some() { "project" } else { "global" },
        "mode": "hybrid-bm25-vector",
        "embedding_status": "ready",
        "matches": results,
        "truncated": results.len() >= limit,
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
        let chunks = chunk_content(content, "md");
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
        let chunks = chunk_content(&content, "md");
        assert!(
            chunks.len() > 1,
            "long content should produce multiple chunks"
        );
        // First chunk starts at line 1
        assert_eq!(chunks[0].line_start, 1);
    }

    #[test]
    fn chunk_content_empty() {
        let chunks = chunk_content("", "md");
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
    fn search_indexed_returns_chunk_id() {
        // chunk_id must be present in BM25 results so hybrid RRF can join on it
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();
        let result = search_indexed(tmp.path(), "OAuth", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty());
        for m in matches {
            assert!(m.get("chunk_id").is_some(), "each match must have a chunk_id field");
            assert!(m["chunk_id"].is_number(), "chunk_id must be numeric");
        }
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
        let chunks = chunk_content("Single line document.", "md");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].line_start, 1);
        assert_eq!(chunks[0].text, "Single line document.");
    }

    #[test]
    fn build_index_lock_prevents_concurrent() {
        let tmp = TempDir::new().unwrap();
        let _lock_path = lock_file(tmp.path());
        
        assert!(!is_locked(tmp.path()));
        
        assert!(try_acquire_lock(tmp.path()));
        assert!(is_locked(tmp.path()));
        
        assert!(!try_acquire_lock(tmp.path()));
        
        release_lock(tmp.path());
        assert!(!is_locked(tmp.path()));
        
        assert!(try_acquire_lock(tmp.path()));
        release_lock(tmp.path());
    }

    #[test]
    fn index_exists_false_when_no_index() {
        let tmp = TempDir::new().unwrap();
        assert!(!index_exists(tmp.path()));
    }

    #[test]
    fn index_exists_true_after_build() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();
        assert!(index_exists(tmp.path()));
    }

    #[test]
    fn check_doc_changes_no_index() {
        let tmp = setup_indexed_root();
        let result = check_doc_changes(tmp.path());
        assert!(!result["index_exists"].as_bool().unwrap());
        assert!(result["is_stale"].as_bool().unwrap());
        // All files should be "added" since no index exists
        assert!(!result["added"].as_array().unwrap().is_empty());
        assert!(result["modified"].as_array().unwrap().is_empty());
        assert!(result["deleted"].as_array().unwrap().is_empty());
    }

    #[test]
    fn check_doc_changes_fresh_index() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();
        let result = check_doc_changes(tmp.path());
        assert!(result["index_exists"].as_bool().unwrap());
        assert!(!result["is_stale"].as_bool().unwrap());
        assert!(result["added"].as_array().unwrap().is_empty());
        assert!(result["modified"].as_array().unwrap().is_empty());
        assert!(result["deleted"].as_array().unwrap().is_empty());
        assert!(result["unchanged_count"].as_u64().unwrap() >= 5);
    }

    #[test]
    fn check_doc_changes_after_add() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();
        fs::write(tmp.path().join("backend/NEW.md"), "# New").unwrap();
        let result = check_doc_changes(tmp.path());
        assert!(result["is_stale"].as_bool().unwrap());
        let added: Vec<&str> = result["added"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(added.iter().any(|a| a.contains("NEW.md")));
    }

    #[test]
    fn check_doc_changes_after_delete() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();
        fs::remove_file(tmp.path().join("backend/PRD.md")).unwrap();
        let result = check_doc_changes(tmp.path());
        assert!(result["is_stale"].as_bool().unwrap());
        let deleted: Vec<&str> = result["deleted"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(deleted.iter().any(|d| d.contains("PRD.md")));
    }

    #[test]
    fn check_doc_changes_after_modify() {
        let tmp = setup_indexed_root();
        build_index_unlocked(tmp.path()).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        fs::write(tmp.path().join("backend/PRD.md"), "# Updated PRD").unwrap();
        let result = check_doc_changes(tmp.path());
        assert!(result["is_stale"].as_bool().unwrap());
        let modified: Vec<&str> = result["modified"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(modified.iter().any(|m| m.contains("PRD.md")));
    }

    #[test]
    fn search_indexed_korean() {
        let tmp = TempDir::new().unwrap();
        let proj = tmp.path().join("korean");
        fs::create_dir_all(&proj).unwrap();
        fs::write(
            proj.join("PRD.md"),
            "# 제품 요구사항\n\n사용자 인증 기능이 필요합니다.\nOAuth 2.0을 사용하여 로그인을 구현합니다.",
        )
        .unwrap();

        build_index_unlocked(tmp.path()).unwrap();
        let result = search_indexed(tmp.path(), "인증", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "should find Korean text '인증'");
    }

    #[test]
    fn search_indexed_japanese() {
        let tmp = TempDir::new().unwrap();
        let proj = tmp.path().join("japanese");
        fs::create_dir_all(&proj).unwrap();
        fs::write(
            proj.join("PRD.md"),
            "# 製品要件\n\nユーザー認証機能が必要です。\nOAuth 2.0を使用してログインを実装します。",
        )
        .unwrap();

        build_index_unlocked(tmp.path()).unwrap();
        let result = search_indexed(tmp.path(), "認証", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "should find Japanese text '認証'");
    }

    #[test]
    fn search_indexed_chinese() {
        let tmp = TempDir::new().unwrap();
        let proj = tmp.path().join("chinese");
        fs::create_dir_all(&proj).unwrap();
        fs::write(
            proj.join("PRD.md"),
            "# 产品需求\n\n用户认证功能是必需的。\n使用OAuth 2.0实现登录。",
        )
        .unwrap();

        build_index_unlocked(tmp.path()).unwrap();
        let result = search_indexed(tmp.path(), "认证", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "should find Chinese text '认证'");
    }
}
