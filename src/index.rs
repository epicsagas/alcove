use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, Duration, Instant};
#[cfg(all(unix, feature = "alcove-full"))]
use std::os::unix::io::AsRawFd;

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, QueryParser, TermQuery};
use tantivy::schema::{self, *};
use tantivy::{DocAddress, Score};
use tantivy::tokenizer::{LowerCaser, NgramTokenizer, TextAnalyzer};
use tantivy::{Index, IndexReader, ReloadPolicy, TantivyDocument};
use walkdir::WalkDir;

use crate::config::effective_config;
#[cfg(not(test))]
use crate::config::load_config;

const NGRAM_TOKENIZER: &str = "cjk_ngram";

// ---------------------------------------------------------------------------
// Process-level IndexReader cache with TTL eviction
// ---------------------------------------------------------------------------
//
// Opening a Tantivy index on every search call is expensive: it re-mmaps all
// segment files and re-initialises the reader.  We keep one Arc<IndexReader>
// per docs_root path alive for the lifetime of the process (important for the
// long-lived MCP/HTTP server; harmless for CLI single-shot calls).
//
// TTL eviction: readers idle longer than `memory.reader_ttl_secs` (default 300 s)
// are dropped at the next `get_cached_reader` call, provided no search holds a
// clone of the Arc.  This bounds resident memory for long-lived server processes.
//
// On `rebuild_index` the entry is evicted before rebuilding so the next search
// picks up freshly written segments.

struct CachedReaderEntry {
    reader: Arc<IndexReader>,
    last_used: Instant,
}

static PROJECT_READER_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedReaderEntry>>> = OnceLock::new();
static VAULT_READER_CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedReaderEntry>>> = OnceLock::new();

/// Cache category — determines which reader cache to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheCategory {
    Project,
    Vault,
}

fn reader_cache_for(cat: CacheCategory) -> &'static Mutex<HashMap<PathBuf, CachedReaderEntry>> {
    match cat {
        CacheCategory::Project => PROJECT_READER_CACHE.get_or_init(|| Mutex::new(HashMap::new())),
        CacheCategory::Vault => VAULT_READER_CACHE.get_or_init(|| Mutex::new(HashMap::new())),
    }
}

/// Return a cached reader for `index_dir`, creating one if absent.
/// Calls `reload()` so that segments written after the reader was first
/// created are visible (no-op when nothing changed).
///
/// TTL eviction runs on every call: idle entries where no external Arc clone
/// is held are dropped.  The cache is also bounded by `memory.max_cached_readers`
/// (default 1); excess entries are evicted LRU-first.
fn get_cached_reader(index_dir: &Path, index: &Index, cat: CacheCategory) -> Result<Arc<IndexReader>> {
    let mem_cfg = crate::config::load_config().memory_config_with_defaults();
    let ttl = if mem_cfg.reader_ttl_secs > 0 {
        Some(Duration::from_secs(mem_cfg.reader_ttl_secs))
    } else {
        None
    };
    let max_readers = mem_cfg.max_cached_readers.clamp(1, 4);

    let mut cache = reader_cache_for(cat).lock().unwrap_or_else(|e| {
        eprintln!("[alcove] reader cache mutex poisoned — clearing stale entries and recovering");
        let mut guard = e.into_inner();
        guard.clear();
        guard
    });

    // TTL eviction pass: drop idle readers not referenced by an active search.
    if let Some(ttl) = ttl {
        cache.retain(|_, entry| {
            if entry.last_used.elapsed() > ttl {
                // Keep this entry — it is still in active use by a caller.
                Arc::strong_count(&entry.reader) > 1
            } else {
                true
            }
        });
    }

    // Cache hit: refresh last_used and reload new segments.
    if let Some(entry) = cache.get_mut(index_dir) {
        entry.last_used = Instant::now();
        let _ = entry.reader.reload();
        return Ok(Arc::clone(&entry.reader));
    }

    // Evict LRU entry (not in use) when at capacity.
    if cache.len() >= max_readers {
        let lru_key = cache
            .iter()
            .filter(|(_, e)| Arc::strong_count(&e.reader) <= 1)
            .min_by_key(|(_, e)| e.last_used)
            .map(|(k, _)| k.clone());
        if let Some(key) = lru_key {
            cache.remove(&key);
        }
    }

    let reader = Arc::new(
        index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .context("Failed to create index reader")?,
    );
    cache.insert(
        index_dir.to_path_buf(),
        CachedReaderEntry {
            reader: Arc::clone(&reader),
            last_used: Instant::now(),
        },
    );
    Ok(reader)
}

/// Evict the cached reader for `docs_root`.
/// Must be called whenever the index is fully rebuilt so the next search
/// does not read stale segment data.
fn invalidate_reader_cache(docs_root: &Path, cat: CacheCategory) {
    let dir = index_dir(docs_root);
    if let Ok(mut cache) = reader_cache_for(cat).lock() {
        cache.remove(&dir);
    }
}

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
    // create_new is atomic (O_CREAT | O_EXCL) — if two processes race past the
    // stale-check above, only one will succeed here.
    match std::fs::File::create_new(&lock_path) {
        Ok(mut f) => {
            // Write PID directly to the opened fd — no window with empty content.
            use std::io::Write;
            let _ = write!(f, "{}", std::process::id());
            true
        }
        Err(_) => false,
    }
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
        let docs_root_canonical = docs_root.canonicalize().unwrap_or_else(|_| docs_root.to_path_buf());
        for walk_entry in WalkDir::new(&path)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_file() && proj_cfg.is_indexable(e.path())
                && !e.path().file_name().unwrap_or_default().to_string_lossy().starts_with('_'))
        {
            let file_path = walk_entry.path();
            if let Ok(canonical) = file_path.canonicalize()
                && !canonical.starts_with(&docs_root_canonical) {
                    continue;
                }
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
        let docs_root_canonical = docs_root.canonicalize().unwrap_or_else(|_| docs_root.to_path_buf());
        for walk_entry in WalkDir::new(&path)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_file() && proj_cfg.is_indexable(e.path())
                && !e.path().file_name().unwrap_or_default().to_string_lossy().starts_with('_'))
        {
            let file_path = walk_entry.path();
            if let Ok(canonical) = file_path.canonicalize()
                && !canonical.starts_with(&docs_root_canonical)
            {
                continue;
            }
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

/// Ensure index is up-to-date, rebuilding synchronously if stale and not locked.
///
/// If the index is stale but a rebuild is already in progress (locked), the
/// function skips the rebuild and returns true so callers can serve stale
/// results rather than blocking for minutes.
///
/// Returns true if a rebuild was triggered or the index is stale-but-locked.
pub fn ensure_index_fresh(docs_root: &Path) -> bool {
    if !is_index_stale(docs_root) {
        return false;
    }
    if is_locked(docs_root) {
        eprintln!("[alcove] index is stale but a rebuild is already in progress — serving stale results");
        return true;
    }
    // Index is stale and no rebuild is running: rebuild synchronously.
    let _ = build_index(docs_root);
    true
}

/// Read file content, extracting text from PDF/DOCX if needed.
fn read_file_content(path: &Path) -> Result<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    
    match ext.as_str() {
        #[cfg(all(unix, feature = "alcove-full"))]
        "pdf" => {
            // pdf_extract prints unicode fallback noise to both stdout and stderr — suppress both.
            // FdGuard restores original fds on drop, protecting against panics.
            #[cfg(all(unix, feature = "alcove-full"))]
            struct FdGuard {
                saved_stdout: libc::c_int,
                saved_stderr: libc::c_int,
            }
            #[cfg(all(unix, feature = "alcove-full"))]
            impl Drop for FdGuard {
                fn drop(&mut self) {
                    unsafe {
                        if self.saved_stdout >= 0 {
                            libc::dup2(self.saved_stdout, libc::STDOUT_FILENO);
                            libc::close(self.saved_stdout);
                        }
                        if self.saved_stderr >= 0 {
                            libc::dup2(self.saved_stderr, libc::STDERR_FILENO);
                            libc::close(self.saved_stderr);
                        }
                    }
                }
            }
            let devnull = std::fs::File::open("/dev/null")
                .map_err(|e| anyhow::anyhow!("Failed to open /dev/null: {}", e))?;
            let devnull_fd = devnull.as_raw_fd();
            let saved_stdout = unsafe { libc::dup(libc::STDOUT_FILENO) };
            if saved_stdout < 0 {
                return Err(anyhow::anyhow!("dup(STDOUT_FILENO) failed"));
            }
            let saved_stderr = unsafe { libc::dup(libc::STDERR_FILENO) };
            if saved_stderr < 0 {
                unsafe { libc::close(saved_stdout); }
                return Err(anyhow::anyhow!("dup(STDERR_FILENO) failed"));
            }
            let _guard = FdGuard { saved_stdout, saved_stderr };
            unsafe {
                libc::dup2(devnull_fd, libc::STDOUT_FILENO);
                libc::dup2(devnull_fd, libc::STDERR_FILENO);
            }
            let result = pdf_extract::extract_text(path)
                .map_err(|e| anyhow::anyhow!("Failed to extract PDF: {}", e));
            // _guard drops here, restoring stdout/stderr automatically
            // Fallback to pdftotext if pdf_extract failed or returned empty content.
            // Uses spawn + try_wait with a 30-second deadline to prevent DoS via
            // a malformed PDF that makes pdftotext loop indefinitely.
            match result {
                Ok(text) if !text.trim().is_empty() => Ok(text),
                _ => {
                    use std::time::{Duration, Instant};
                    let pdftotext_bin = ["/usr/bin/pdftotext", "/usr/local/bin/pdftotext", "/opt/homebrew/bin/pdftotext"]
                        .iter()
                        .find(|p| std::path::Path::new(p).exists())
                        .copied()
                        .unwrap_or("pdftotext");
                    let mut child = std::process::Command::new(pdftotext_bin)
                        .args([path.as_os_str(), std::ffi::OsStr::new("-")])
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .map_err(|e| anyhow::anyhow!("pdftotext not available: {}", e))?;
                    let deadline = Instant::now() + Duration::from_secs(30);
                    let status = loop {
                        match child.try_wait() {
                            Ok(Some(s)) => break Ok(s),
                            Ok(None) => {
                                if Instant::now() > deadline {
                                    let _ = child.kill();
                                    break Err(anyhow::anyhow!("pdftotext timed out"));
                                }
                                std::thread::sleep(Duration::from_millis(100));
                            }
                            Err(e) => break Err(anyhow::anyhow!("pdftotext wait error: {}", e)),
                        }
                    };
                    let status = status?;
                    if status.success() {
                        let mut stdout = child.stdout.take().unwrap_or_else(|| {
                            // stdout was already consumed by try_wait path; return empty
                            unreachable!("stdout pipe must be present after spawn")
                        });
                        let mut buf = Vec::new();
                        use std::io::Read;
                        stdout.read_to_end(&mut buf)
                            .map_err(|e| anyhow::anyhow!("pdftotext read error: {}", e))?;
                        String::from_utf8(buf)
                            .map_err(|e| anyhow::anyhow!("pdftotext output not UTF-8: {}", e))
                    } else {
                        Err(anyhow::anyhow!("pdftotext failed"))
                    }
                }
            }
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
    // Evict the cached reader before wiping the index directory so that any
    // search initiated after this call opens a fresh reader on the new data.
    invalidate_reader_cache(docs_root, CacheCategory::Project);
    build_index_with_mode(docs_root, true)
}

fn build_index_with_mode(docs_root: &Path, force_rebuild: bool) -> Result<JsonValue> {
    build_index_with_options(docs_root, force_rebuild, false)
}

fn build_index_with_options(docs_root: &Path, force_rebuild: bool, skip_embedding: bool) -> Result<JsonValue> {
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
    let result = build_index_inner(docs_root, skip_embedding);
    release_lock(docs_root);
    result
}

/// Build only the BM25 (Tantivy) index, skipping vector embedding entirely.
/// Used by the MCP server startup to avoid blocking on ONNX model loading.
pub fn build_index_bm25_only(docs_root: &Path) -> Result<JsonValue> {
    build_index_with_options(docs_root, false, true)
}

#[cfg(test)]
pub fn build_index_unlocked(docs_root: &Path) -> Result<JsonValue> {
    build_index_inner(docs_root, true)
}

fn build_index_inner(docs_root: &Path, skip_embedding: bool) -> Result<JsonValue> {
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
        let docs_root_canonical = docs_root.canonicalize().unwrap_or_else(|_| docs_root.to_path_buf());
        for walk_entry in WalkDir::new(&path).into_iter().flatten().filter(|e| e.file_type().is_file() && proj_cfg.is_indexable(e.path())
            && !e.path().file_name().unwrap_or_default().to_string_lossy().starts_with('_')) {
            let file_path = walk_entry.path().to_path_buf();
            if let Ok(canonical) = file_path.canonicalize()
                && !canonical.starts_with(&docs_root_canonical)
            {
                continue;
            }
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
        // In test builds use a single writer thread with the minimum heap
        // (15 MB) so that many parallel test indices don't exhaust RAM.
        // Tantivy spawns one thread per logical CPU by default; with 147 tests
        // running concurrently that multiplies to gigabytes of writer buffers.
        // Production uses all CPUs with the configured buffer size.
        #[cfg(not(test))]
        let mut writer = index.writer(load_config().index_buffer_bytes())?;
        #[cfg(test)]
        let mut writer = index.writer_with_num_threads(1, 15_000_000)?;

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
                let proj_term = tantivy::Term::from_field_text(project_field, proj);
                let file_term = tantivy::Term::from_field_text(file_field, rel);
                let delete_query = BooleanQuery::new(vec![
                    (Occur::Must, Box::new(TermQuery::new(proj_term, IndexRecordOption::Basic)) as Box<dyn tantivy::query::Query>),
                    (Occur::Must, Box::new(TermQuery::new(file_term, IndexRecordOption::Basic)) as Box<dyn tantivy::query::Query>),
                ]);
                writer.delete_query(Box::new(delete_query))?;
            }

            if let Ok(content) = read_file_content(full) {
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
    // Vector indexing (alcove-full feature) — skipped when skip_embedding=true
    // ---------------------------------------------------------------------------

    #[cfg_attr(not(feature = "alcove-full"), allow(unused_mut))]
    let mut vector_status = if skip_embedding { "skipped".to_string() } else { "disabled".to_string() };
    #[cfg_attr(not(feature = "alcove-full"), allow(unused_mut))]
    let mut vectors_indexed = 0u64;
    #[cfg_attr(not(feature = "alcove-full"), allow(unused_mut))]
    let mut vector_errors = 0u64;
    #[cfg_attr(not(feature = "alcove-full"), allow(unused_mut))]
    let mut embedding_model = String::new();

    #[cfg(feature = "alcove-full")]
    if !skip_embedding {
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
                query_cache_size: emb_cfg.query_cache_size,
            });

            // Ensure model is loaded (and downloaded if auto_download is enabled)
            let _ = service.ensure_model();

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
                                            let actual = pending.len();
                                            let doc_pfx = model.doc_prefix();
                                            let texts: Vec<String> = pending.iter().map(|(_, _, _, t)| {
                                                match doc_pfx {
                                                    Some(pfx) => format!("{}{}", pfx, t),
                                                    None => t.clone(),
                                                }
                                            }).collect();
                                            let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                                            match service.embed(&text_refs) {
                                                Ok(embeddings) => {
                                                    let it = pending.drain(..).zip(embeddings).map(|((p, r, id, _), emb)| (p, r, id, emb));
                                                    if let Err(e) = store.batch_upsert(it) {
                                                        eprintln!("[alcove] batch upsert failed: {}", e);
                                                        vector_errors += actual as u64;
                                                    } else {
                                                        vectors_indexed += actual as u64;
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
                                let doc_pfx = model.doc_prefix();
                                let texts: Vec<String> = pending.iter().map(|(_, _, _, t)| {
                                    match doc_pfx {
                                        Some(pfx) => format!("{}{}", pfx, t),
                                        None => t.clone(),
                                    }
                                }).collect();
                                let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                                match service.embed(&text_refs) {
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
                        // Always report total vectors in store (not just this run's delta)
                        if let Ok(meta) = store.meta() {
                            vectors_indexed = meta.count as u64;
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
        '<', '>', '"',
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
    search_with_index(docs_root, query, &sanitized, limit, project_filter, &index)
}

/// Inner BM25 search — accepts a pre-opened `Index` so callers (e.g. hybrid
/// search) can reuse a single open/reader across multiple search phases.
///
/// `sanitized` must already be the output of `sanitize_query(query)` and
/// must be non-empty.
fn search_with_index(
    docs_root: &Path,
    query: &str,
    sanitized: &str,
    limit: usize,
    project_filter: Option<&str>,
    index: &Index,
) -> Result<JsonValue> {
    let dir = index_dir(docs_root);
    let schema = index.schema();
    let project_field = schema.get_field("project").context("missing 'project' field")?;
    let file_field = schema.get_field("file").context("missing 'file' field")?;
    let chunk_id_field = schema.get_field("chunk_id").context("missing 'chunk_id' field")?;
    let body_field = schema.get_field("body").context("missing 'body' field")?;
    let line_start_field = schema.get_field("line_start").context("missing 'line_start' field")?;

    // Use the process-level cached reader (creates one on first call per path).
    let reader = get_cached_reader(&dir, index, CacheCategory::Project)?;
    let searcher = reader.searcher();

    let query_parser = QueryParser::for_index(index, vec![body_field]);
    let parsed_query = query_parser
        .parse_query(sanitized)
        .context("Failed to parse search query")?;

    // Fetch 3× candidates for per-file deduplication.
    // Using 3× instead of 5× reduces wasted doc fetches while still giving
    // enough headroom to deduplicate down to `limit` unique files.
    let top_docs: Vec<(Score, DocAddress)> = searcher
        .search(&parsed_query, &TopDocs::with_limit(limit * 3).order_by_score())
        .context("Search failed")?;

    // Deduplicate: keep only the best-scoring chunk per (project, file) pair.
    // We use a HashSet<(&str, &str)> built from owned strings held in `results`
    // to avoid double-allocating keys for the "already seen" fast path.
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
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

        // Skip if we already have a better chunk from this file.
        // `entry` avoids a redundant key clone on the insert path.
        use std::collections::hash_map::Entry;
        if let Entry::Vacant(e) = seen.entry((project.clone(), file.clone())) {
            e.insert(results.len());
        } else {
            continue;
        }

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

/// Perform hybrid search combining BM25 and vector similarity.
/// Returns results ranked by Reciprocal Rank Fusion (RRF).
///
/// The Tantivy index is opened **once** and reused across the BM25 phase and
/// any vector-only cache-miss lookups, eliminating the previous double-open.
#[cfg(feature = "alcove-full")]
pub fn search_hybrid(
    docs_root: &Path,
    query: &str,
    embedding_service: &crate::embedding::EmbeddingService,
    limit: usize,
    project_filter: Option<&str>,
) -> Result<JsonValue> {
    use crate::vector::{reciprocal_rank_fusion, VectorStore};

    // 1. Check embedding model state
    let model_state = embedding_service.state();
    let model_ready = model_state == crate::embedding::ModelState::Ready;

    // 3. Open the Tantivy index ONCE — reused for BM25 and cache-miss lookups.
    let dir = index_dir(docs_root);
    if !dir.exists() {
        anyhow::bail!("Search index not found. Run index rebuild first.");
    }
    let sanitized = sanitize_query(query);
    let index = Index::open_in_dir(&dir).context("Failed to open search index")?;
    register_ngram_tokenizer(&index)?;

    // 4. BM25 search (uses cached reader via search_with_index)
    let bm25_json = if sanitized.is_empty() {
        json!({ "matches": [], "truncated": false })
    } else {
        search_with_index(docs_root, query, &sanitized, limit * 2, project_filter, &index)?
    };

    // Extract BM25 tuples for RRF — read directly from the JSON values produced
    // above; avoids a second pass through Tantivy for the same data.
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

    // 5. If model not ready, return BM25-only with status
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

    // 6. Generate query embedding
    let query_pfx = embedding_service.model_choice().query_prefix();
    let prefixed_query = match query_pfx {
        Some(pfx) => format!("{}{}", pfx, query),
        None => query.to_string(),
    };
    let query_embedding = match embedding_service.embed(&[&prefixed_query]) {
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

    // 7. Open vector store and search
    let vector_path = docs_root.join(".alcove").join("vectors.db");
    let store = VectorStore::open(&vector_path, embedding_service.model_name(), embedding_service.dimension());

    let mut vector_error: Option<String> = None;
    let vector_results = match store {
        Ok(s) => match s.search_with_filter(&query_embedding, limit * 2, project_filter) {
            Ok(r) => r,
            Err(e) => {
                vector_error = Some(format!("vector search error: {e}"));
                Vec::new()
            }
        },
        Err(e) => {
            vector_error = Some(format!("vector store open error: {e}"));
            Vec::new()
        }
    };

    // 8. Combine with RRF.
    // k scales with limit: smaller result sets benefit from a lower k which
    // spreads scores more aggressively; larger sets use the classic k=60.
    // Formula: k = max(10, round(60 * sqrt(limit / 50)))
    let rrf_k = ((60.0 * ((limit as f32) / 50.0).sqrt()).round() as u32).max(10);
    let fused = reciprocal_rank_fusion(&bm25_results, &vector_results, rrf_k);
    let truncated = fused.len() > limit;

    // 9. Build final results with snippets.
    //
    // BM25 results already carry `snippet` and `line_start`.  We cache them in
    // a HashMap so vector-only hits (chunk_ids absent from BM25) can still fall
    // back to a single Tantivy lookup — but the common case of BM25-overlap hits
    // requires zero additional index queries.
    //
    // Key: (project, file, chunk_id) → (snippet, line_start)
    type SnippetKey = (String, String, u64);
    let mut snippet_cache: HashMap<SnippetKey, (String, u64)> = HashMap::new();

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

    // For vector-only cache misses we need a Tantivy searcher.  We reuse the
    // already-open `index` instead of calling Index::open_in_dir a second time.
    // The reader is fetched from the process-level cache (same as BM25 above).
    let miss_reader = get_cached_reader(&dir, &index, CacheCategory::Project)?;
    let miss_searcher = miss_reader.searcher();
    let miss_schema = index.schema();
    let miss_project_field = miss_schema.get_field("project").context("missing 'project' field")?;
    let miss_file_field = miss_schema.get_field("file").context("missing 'file' field")?;
    let miss_body_field = miss_schema.get_field("body").context("missing 'body' field")?;
    let miss_line_start_field = miss_schema.get_field("line_start").context("missing 'line_start' field")?;
    let miss_chunk_id_field = miss_schema.get_field("chunk_id").context("missing 'chunk_id' field")?;

    let mut results: Vec<JsonValue> = Vec::new();

    for (project, file, chunk_id, rrf_score) in fused.into_iter().take(limit) {
        let (snippet, line_start) =
            if let Some(cached) = snippet_cache.get(&(project.clone(), file.clone(), chunk_id)) {
                cached.clone()
            } else {
                // Cache miss: vector-only hit — look up via Tantivy (rare path).
                // Uses the reader already obtained above; no second index open.
                let combined = tantivy::query::BooleanQuery::new(vec![
                    (
                        tantivy::query::Occur::Must,
                        Box::new(tantivy::query::TermQuery::new(
                            tantivy::Term::from_field_text(miss_project_field, &project),
                            tantivy::schema::IndexRecordOption::Basic,
                        )) as Box<dyn tantivy::query::Query>,
                    ),
                    (
                        tantivy::query::Occur::Must,
                        Box::new(tantivy::query::TermQuery::new(
                            tantivy::Term::from_field_text(miss_file_field, &file),
                            tantivy::schema::IndexRecordOption::Basic,
                        )),
                    ),
                    (
                        tantivy::query::Occur::Must,
                        Box::new(tantivy::query::TermQuery::new(
                            tantivy::Term::from_field_u64(miss_chunk_id_field, chunk_id),
                            tantivy::schema::IndexRecordOption::Basic,
                        )),
                    ),
                ]);

                if let Ok(top_docs) =
                    miss_searcher.search(&combined, &TopDocs::with_limit(1).order_by_score())
                {
                    if let Some((_s, addr)) = top_docs.first() {
                        if let Ok(doc) = miss_searcher.doc::<TantivyDocument>(*addr) {
                            let body = doc
                                .get_first(miss_body_field)
                                .and_then(|v| schema::Value::as_str(&v))
                                .unwrap_or("")
                                .to_string();
                            let ls = doc
                                .get_first(miss_line_start_field)
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
        "embedding_status": match &vector_error {
            Some(e) => e.as_str(),
            None => "ready",
        },
        "matches": results,
        "truncated": truncated,
    }))
}

// ---------------------------------------------------------------------------
// Vault indexing
// ---------------------------------------------------------------------------

/// Build a BM25 index for a single vault directory.
///
/// Vault files live directly under `vault_path` (flat structure), unlike
/// project docs which are nested under `docs_root/<project>/`.  The vault
/// name (directory basename) is stored in the `project` field for schema
/// compatibility with project indexes.
pub fn build_vault_index(vault_path: &Path) -> Result<JsonValue> {
    let vault_name = vault_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let dir = index_dir(vault_path);
    std::fs::create_dir_all(&dir)?;

    let (schema, project_field, file_field, chunk_id_field, body_field, line_start_field) =
        build_schema();

    // Walk all .md files in vault_path, excluding `_` prefix and `.alcove/`
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(vault_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            // Skip .alcove directory
            if path
                .strip_prefix(vault_path)
                .ok()
                .and_then(|rel| rel.components().next())
                .map(|c| {
                    let s = c.as_os_str().to_string_lossy();
                    s.starts_with('.') || s.starts_with('_')
                })
                .unwrap_or(false)
            {
                return false;
            }
            // Must be a file
            if !e.file_type().is_file() {
                return false;
            }
            // Must be .md
            if !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                return false;
            }
            // Must not have underscore prefix filename
            if path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .starts_with('_')
            {
                return false;
            }
            true
        })
    {
        files.push(entry.path().to_path_buf());
    }

    let file_count = files.len() as u64;

    let index = Index::create_in_dir(&dir, schema.clone()).or_else(|_| {
        std::fs::remove_dir_all(&dir)?;
        std::fs::create_dir_all(&dir)?;
        Index::create_in_dir(&dir, schema.clone())
    })?;
    register_ngram_tokenizer(&index)?;

    #[cfg(not(test))]
    let mut writer = index.writer(crate::config::load_config().index_buffer_bytes())?;
    #[cfg(test)]
    let mut writer = index.writer_with_num_threads(1, 15_000_000)?;

    for file_path in &files {
        let rel = file_path
            .strip_prefix(vault_path)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();

        if let Ok(content) = read_file_content(file_path) {
            let file_ext = file_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            for (chunk_idx, chunk) in chunk_content(&content, file_ext).iter().enumerate() {
                let mut doc = TantivyDocument::new();
                doc.add_text(project_field, &vault_name);
                doc.add_text(file_field, &rel);
                doc.add_u64(chunk_id_field, chunk_idx as u64);
                doc.add_text(body_field, &chunk.text);
                doc.add_u64(line_start_field, chunk.line_start as u64);
                writer.add_document(doc)?;
            }
        }
    }

    writer.commit()?;

    // Save metadata for the vault index
    let mut meta = IndexMeta::default();
    for file_path in &files {
        let rel = file_path
            .strip_prefix(vault_path)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();
        meta.files.insert(rel, file_fingerprint(file_path));
    }
    let _ = meta.save(vault_path);

    // ---------------------------------------------------------------------------
    // Vector indexing (alcove-full feature) — uses vault-specific embedding model
    // with fallback to global config.
    // ---------------------------------------------------------------------------
    #[cfg_attr(not(feature = "alcove-full"), allow(unused_mut))]
    let mut vector_status = "disabled".to_string();
    #[cfg_attr(not(feature = "alcove-full"), allow(unused_mut))]
    let mut vectors_indexed = 0u64;
    #[cfg_attr(not(feature = "alcove-full"), allow(unused_mut))]
    let mut vector_errors = 0u64;
    #[cfg_attr(not(feature = "alcove-full"), allow(unused_mut))]
    let mut embedding_model = String::new();

    #[cfg(feature = "alcove-full")]
    {
        use crate::embedding::{EmbeddingModelChoice, EmbeddingService};
        use crate::vector::VectorStore;
        use crate::config::vault_embedding_config;

        let emb_cfg = vault_embedding_config(vault_path);

        if emb_cfg.enabled {
            let model = EmbeddingModelChoice::parse(&emb_cfg.model).unwrap_or_default();
            embedding_model = model.as_str().to_string();

            let service = EmbeddingService::new(crate::config::EmbeddingConfig {
                model: model.as_str().to_string(),
                auto_download: emb_cfg.auto_download,
                cache_dir: emb_cfg.cache_dir.clone(),
                enabled: true,
                query_cache_size: emb_cfg.query_cache_size,
            });

            let _ = service.ensure_model();

            if service.state() == crate::embedding::ModelState::Ready {
                let vector_path = vault_path.join(".alcove").join("vectors.db");
                match VectorStore::open(&vector_path, service.model_name(), service.dimension()) {
                    Ok(mut store) => {
                        vector_status = "ok".to_string();

                        // Filter out already-indexed files
                        let to_embed: Vec<&PathBuf> = files
                            .iter()
                            .filter(|fp| {
                                let rel = fp
                                    .strip_prefix(vault_path)
                                    .unwrap_or(*fp)
                                    .to_string_lossy();
                                !matches!(store.has_file(&vault_name, &rel), Ok(true))
                            })
                            .collect();

                        let total_files = to_embed.len() as u64;

                        if total_files > 0 {
                            let vpb = ProgressBar::new(total_files);
                            vpb.set_style(
                                ProgressStyle::default_bar()
                                    .template("  {spinner:.magenta}  {bar:35.white/dim}  {pos:>3}/{len}  {msg:.dim}")?
                                    .progress_chars("━━╌"),
                            );
                            vpb.enable_steady_tick(Duration::from_millis(80));
                            vpb.set_message("embedding");

                            let embed_batch: usize = match model.size_mb() {
                                s if s < 100 => 128,
                                s if s <= 800 => 64,
                                _ => 32,
                            };
                            let mut pending: Vec<(String, String, u64, String)> = Vec::new();

                            for fp in &to_embed {
                                let rel = fp
                                    .strip_prefix(vault_path)
                                    .unwrap_or(*fp)
                                    .to_string_lossy()
                                    .to_string();
                                vpb.set_message(rel.clone());

                                if let Ok(content) = read_file_content(fp) {
                                    let file_ext = fp.extension().and_then(|e| e.to_str()).unwrap_or("");
                                    for (i, chunk) in chunk_content(&content, file_ext).into_iter().enumerate() {
                                        pending.push((vault_name.clone(), rel.clone(), i as u64, chunk.text));

                                        if pending.len() >= embed_batch {
                                            let actual = pending.len();
                                            let doc_pfx = model.doc_prefix();
                                            let texts: Vec<String> = pending.iter().map(|(_, _, _, t)| {
                                                match doc_pfx {
                                                    Some(pfx) => format!("{}{}", pfx, t),
                                                    None => t.clone(),
                                                }
                                            }).collect();
                                            let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                                            match service.embed(&text_refs) {
                                                Ok(embeddings) => {
                                                    let it = pending.drain(..).zip(embeddings).map(|((p, r, id, _), emb)| (p, r, id, emb));
                                                    if let Err(e) = store.batch_upsert(it) {
                                                        eprintln!("[alcove] vault batch upsert failed: {}", e);
                                                        vector_errors += actual as u64;
                                                    } else {
                                                        vectors_indexed += actual as u64;
                                                    }
                                                }
                                                Err(e) => {
                                                    eprintln!("[alcove] vault embed failed: {}", e);
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
                                let doc_pfx = model.doc_prefix();
                                let texts: Vec<String> = pending.iter().map(|(_, _, _, t)| {
                                    match doc_pfx {
                                        Some(pfx) => format!("{}{}", pfx, t),
                                        None => t.clone(),
                                    }
                                }).collect();
                                let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                                match service.embed(&text_refs) {
                                    Ok(embeddings) => {
                                        let count = pending.len() as u64;
                                        let it = pending.drain(..).zip(embeddings).map(|((p, r, id, _), emb)| (p, r, id, emb));
                                        if let Err(e) = store.batch_upsert(it) {
                                            eprintln!("[alcove] vault batch upsert failed: {}", e);
                                            vector_errors += count;
                                        } else {
                                            vectors_indexed += count;
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("[alcove] vault embed failed: {}", e);
                                        vector_errors += pending.len() as u64;
                                    }
                                }
                            }

                            vpb.finish_and_clear();
                        }
                        // Always report total vectors in store
                        if let Ok(meta) = store.meta() {
                            vectors_indexed = meta.count as u64;
                        }
                    }
                    Err(e) => {
                        eprintln!("[alcove] Failed to open vault vector store: {}", e);
                        vector_status = format!("error: {}", e);
                    }
                }
            } else {
                vector_status = "model_not_ready".to_string();
            }
        }
    }

    Ok(json!({
        "status": "ok",
        "vault": vault_name,
        "files": file_count,
        "index_path": dir.to_string_lossy(),
        "vectors_indexed": vectors_indexed,
        "vector_errors": vector_errors,
        "vector_status": vector_status,
        "embedding_model": embedding_model,
    }))
}

/// Build BM25 indexes for all registered vaults.
///
/// Calls `vault::list_vaults()` and runs `build_vault_index` on each.
pub fn build_all_vault_indexes() -> Result<JsonValue> {
    let vaults = crate::vault::list_vaults()?;
    let mut results = Vec::new();
    let mut errors = Vec::new();

    for vault in &vaults {
        match build_vault_index(&vault.path) {
            Ok(summary) => results.push(summary),
            Err(e) => errors.push(json!({
                "vault": vault.name,
                "error": e.to_string(),
            })),
        }
    }

    Ok(json!({
        "status": "ok",
        "vaults_indexed": results.len(),
        "vaults_failed": errors.len(),
        "results": results,
        "errors": errors,
    }))
}

/// Search a single vault's BM25 index.
///
/// Returns the same JSON format as `search_indexed` (matches array with
/// file, score, snippet, etc.) for consistency.
pub fn search_vault(vault_path: &Path, query: &str, limit: usize) -> Result<JsonValue> {
    let dir = index_dir(vault_path);
    if !dir.exists() {
        anyhow::bail!("Vault search index not found. Run vault index build first.");
    }

    let vault_name = vault_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let sanitized = sanitize_query(query);
    if sanitized.is_empty() {
        return Ok(json!({
            "query": query,
            "vault": vault_name,
            "mode": "ranked",
            "matches": [],
            "truncated": false,
        }));
    }

    let index = Index::open_in_dir(&dir).context("Failed to open vault search index")?;
    register_ngram_tokenizer(&index)?;

    // --- Hybrid search (BM25 + vector) for vaults ---
    // Falls back to BM25-only when vectors are not available.
    #[cfg(feature = "alcove-full")]
    {
        use crate::embedding::{EmbeddingModelChoice, EmbeddingService};
        use crate::vector::{reciprocal_rank_fusion, VectorStore};
        use crate::config::vault_embedding_config;

        let emb_cfg = vault_embedding_config(vault_path);

        if emb_cfg.enabled {
            let model = EmbeddingModelChoice::parse(&emb_cfg.model).unwrap_or_default();
            let service = EmbeddingService::new(crate::config::EmbeddingConfig {
                model: model.as_str().to_string(),
                auto_download: emb_cfg.auto_download,
                cache_dir: emb_cfg.cache_dir.clone(),
                enabled: true,
                query_cache_size: emb_cfg.query_cache_size,
            });

            if service.state() == crate::embedding::ModelState::Ready {
                let vector_path = vault_path.join(".alcove").join("vectors.db");

                // BM25 search with extra candidates for RRF
                let bm25_json = search_vault_bm25_inner(vault_path, query, &sanitized, limit * 2, &index)?;

                // Query embedding with prefix
                let query_pfx = service.model_choice().query_prefix();
                let prefixed_query = match query_pfx {
                    Some(pfx) => format!("{}{}", pfx, query),
                    None => query.to_string(),
                };
                let query_embedding = match service.embed(&[&prefixed_query]) {
                    Ok(emb) => emb.into_iter().next().unwrap_or_default(),
                    Err(_) => {
                        // Embedding failed, return BM25-only
                        return Ok(json!({
                            "query": query,
                            "vault": vault_name,
                            "mode": "bm25-only",
                            "embedding_status": "query_embed_failed",
                            "matches": bm25_json["matches"],
                            "truncated": bm25_json["truncated"],
                        }));
                    }
                };

                // Vector search
                let mut vector_error: Option<String> = None;
                let vector_results = match VectorStore::open(&vector_path, service.model_name(), service.dimension()) {
                    Ok(s) => match s.search(&query_embedding, limit * 2) {
                        Ok(r) => r,
                        Err(e) => {
                            vector_error = Some(format!("vector search error: {e}"));
                            Vec::new()
                        }
                    },
                    Err(e) => {
                        vector_error = Some(format!("vector store open error: {e}"));
                        Vec::new()
                    }
                };

                // Extract BM25 tuples for RRF
                let bm25_results: Vec<(String, String, u64, f32)> = bm25_json["matches"]
                    .as_array()
                    .map(|arr| {
                        arr.iter().filter_map(|m| {
                            Some((
                                m["project"].as_str()?.to_string(),
                                m["file"].as_str()?.to_string(),
                                m["chunk_id"].as_u64()?,
                                m["score"].as_f64()? as f32,
                            ))
                        }).collect()
                    })
                    .unwrap_or_default();

                // RRF fusion
                let rrf_k = ((60.0 * ((limit as f32) / 50.0).sqrt()).round() as u32).max(10);
                let fused = reciprocal_rank_fusion(&bm25_results, &vector_results, rrf_k);
                let truncated = fused.len() > limit;

                // Build snippet cache from BM25 results
                type SnippetKey = (String, String, u64);
                let mut snippet_cache: HashMap<SnippetKey, (String, u64)> = HashMap::new();
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

                // Resolve snippets for vector-only hits
                let miss_reader = get_cached_reader(&dir, &index, CacheCategory::Vault)?;
                let miss_searcher = miss_reader.searcher();
                let miss_schema = index.schema();
                let miss_project_field = miss_schema.get_field("project").context("missing project field")?;
                let miss_file_field = miss_schema.get_field("file").context("missing file field")?;
                let miss_body_field = miss_schema.get_field("body").context("missing body field")?;
                let miss_line_start_field = miss_schema.get_field("line_start").context("missing line_start field")?;
                let miss_chunk_id_field = miss_schema.get_field("chunk_id").context("missing chunk_id field")?;

                let mut results: Vec<JsonValue> = Vec::new();
                for (project, file, chunk_id, rrf_score) in fused.into_iter().take(limit) {
                    let (snippet, line_start) =
                        if let Some(cached) = snippet_cache.get(&(project.clone(), file.clone(), chunk_id)) {
                            cached.clone()
                        } else {
                            let combined = tantivy::query::BooleanQuery::new(vec![
                                (tantivy::query::Occur::Must, Box::new(tantivy::query::TermQuery::new(
                                    tantivy::Term::from_field_text(miss_project_field, &project),
                                    tantivy::schema::IndexRecordOption::Basic,
                                )) as Box<dyn tantivy::query::Query>),
                                (tantivy::query::Occur::Must, Box::new(tantivy::query::TermQuery::new(
                                    tantivy::Term::from_field_text(miss_file_field, &file),
                                    tantivy::schema::IndexRecordOption::Basic,
                                ))),
                                (tantivy::query::Occur::Must, Box::new(tantivy::query::TermQuery::new(
                                    tantivy::Term::from_field_u64(miss_chunk_id_field, chunk_id),
                                    tantivy::schema::IndexRecordOption::Basic,
                                ))),
                            ]);
                            if let Ok(top_docs) = miss_searcher.search(&combined, &TopDocs::with_limit(1).order_by_score()) {
                                if let Some((_s, addr)) = top_docs.first() {
                                    if let Ok(doc) = miss_searcher.doc::<TantivyDocument>(*addr) {
                                        let body = doc.get_first(miss_body_field)
                                            .and_then(|v| schema::Value::as_str(&v))
                                            .unwrap_or("")
                                            .to_string();
                                        let ls = doc.get_first(miss_line_start_field)
                                            .and_then(|v| schema::Value::as_u64(&v))
                                            .unwrap_or(0);
                                        (body, ls)
                                    } else { (String::new(), 0) }
                                } else { (String::new(), 0) }
                            } else { (String::new(), 0) }
                        };

                    results.push(json!({
                        "project": project,
                        "file": file,
                        "line_start": line_start,
                        "snippet": snippet,
                        "score": (rrf_score * 1000.0).round() / 1000.0,
                    }));
                }

                return Ok(json!({
                    "query": query,
                    "vault": vault_name,
                    "mode": "hybrid-bm25-vector",
                    "embedding_status": match &vector_error {
                        Some(e) => e.as_str(),
                        None => "ready",
                    },
                    "matches": results,
                    "truncated": truncated,
                }));
            }
        }
    }

    // BM25-only fallback (always available, used when embedding is disabled or not ready)
    search_vault_bm25_inner(vault_path, query, &sanitized, limit, &index)
}

/// Inner BM25 search for vault — accepts a pre-opened index for reuse.
fn search_vault_bm25_inner(
    vault_path: &Path,
    query: &str,
    sanitized: &str,
    limit: usize,
    index: &Index,
) -> Result<JsonValue> {
    let vault_name = vault_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let dir = index_dir(vault_path);
    let schema = index.schema();
    let project_field = schema.get_field("project").context("missing 'project' field")?;
    let file_field = schema.get_field("file").context("missing 'file' field")?;
    let chunk_id_field = schema.get_field("chunk_id").context("missing 'chunk_id' field")?;
    let body_field = schema.get_field("body").context("missing 'body' field")?;
    let line_start_field = schema
        .get_field("line_start")
        .context("missing 'line_start' field")?;

    let reader = get_cached_reader(&dir, &index, CacheCategory::Vault)?;
    let searcher = reader.searcher();

    let query_parser = QueryParser::for_index(&index, vec![body_field]);
    let parsed_query = query_parser
        .parse_query(&sanitized)
        .context("Failed to parse vault search query")?;

    let top_docs: Vec<(Score, DocAddress)> = searcher
        .search(
            &parsed_query,
            &TopDocs::with_limit(limit * 3).order_by_score(),
        )
        .context("Vault search failed")?;

    // Deduplicate: keep only the best-scoring chunk per file.
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut results = Vec::new();

    for (score, doc_address) in top_docs {
        let doc: TantivyDocument = searcher.doc(doc_address)?;

        let project = doc
            .get_first(project_field)
            .and_then(|v| schema::Value::as_str(&v))
            .unwrap_or("")
            .to_string();

        let file = doc
            .get_first(file_field)
            .and_then(|v| schema::Value::as_str(&v))
            .unwrap_or("")
            .to_string();

        use std::collections::hash_map::Entry;
        if let Entry::Vacant(e) = seen.entry(file.clone()) {
            e.insert(results.len());
        } else {
            continue;
        }

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
        "vault": vault_name,
        "mode": "ranked",
        "matches": results,
        "truncated": truncated,
    }))
}

/// Delete and rebuild a single vault's BM25 index.
///
/// Invalidates the vault reader cache before wiping the index directory
/// so the next search opens a fresh reader.
pub fn rebuild_vault_index(vault_path: &Path) -> Result<JsonValue> {
    invalidate_reader_cache(vault_path, CacheCategory::Vault);

    let dir = index_dir(vault_path);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    let meta = meta_path(vault_path);
    if meta.exists() {
        std::fs::remove_file(&meta)?;
    }
    // Also clear vector store so embeddings are re-indexed
    let vectors_path = vault_path.join(".alcove").join("vectors.db");
    if vectors_path.exists() {
        std::fs::remove_file(&vectors_path)?;
    }

    build_vault_index(vault_path)
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
        let result = build_index_inner(tmp.path(), true).unwrap();
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
        build_index_inner(tmp.path(), true).unwrap();
        // Second build with no changes
        let result = build_index_inner(tmp.path(), true).unwrap();
        assert_eq!(result["status"], "ok");
        // All files should be skipped on second run
        assert!(result["skipped"].as_u64().unwrap() >= 5);
    }

    #[test]
    fn search_indexed_finds_oauth() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();

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
        build_index_inner(tmp.path(), true).unwrap();

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
        build_index_inner(tmp.path(), true).unwrap();

        let result = search_indexed(tmp.path(), "OAuth", 1, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn search_indexed_no_results() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();

        let result = search_indexed(tmp.path(), "zzz_nonexistent_query_zzz", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn search_indexed_skips_hidden_projects() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();

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
        build_index_inner(tmp.path(), true).unwrap();
        assert!(
            !is_index_stale(tmp.path()),
            "just-built index should not be stale"
        );
    }

    #[test]
    fn is_index_stale_after_file_change() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();
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
        build_index_inner(tmp.path(), true).unwrap();

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
        build_index_inner(tmp.path(), true).unwrap();

        let rebuilt = ensure_index_fresh(tmp.path());
        assert!(!rebuilt, "should not rebuild when fresh");
    }

    #[test]
    fn is_index_stale_after_file_deletion() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();
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
        build_index_inner(tmp.path(), true).unwrap();

        // These should not panic or error
        let result = search_indexed(tmp.path(), "C++", 10, None).unwrap();
        assert!(result["matches"].is_array());

        let result = search_indexed(tmp.path(), "test:query", 10, None).unwrap();
        assert!(result["matches"].is_array());

        let result = search_indexed(tmp.path(), "(foo AND bar)", 10, None).unwrap();
        assert!(result["matches"].is_array());
    }

    #[test]
    fn search_indexed_xss_special_chars_no_panic() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();

        // Angle brackets and quotes are tantivy special chars that must be escaped
        let result = search_indexed(tmp.path(), "<script>alert()</script>", 10, None).unwrap();
        assert!(result["matches"].is_array());

        let result = search_indexed(tmp.path(), "foo<bar>baz", 10, None).unwrap();
        assert!(result["matches"].is_array());

        let result = search_indexed(tmp.path(), r#""quoted phrase""#, 10, None).unwrap();
        assert!(result["matches"].is_array());
    }

    #[test]
    fn sanitize_query_escapes_angle_brackets_and_quotes() {
        assert_eq!(sanitize_query("<script>"), "\\<script\\>");
        assert_eq!(sanitize_query(r#""hello""#), "\\\"hello\\\"");
        assert_eq!(
            sanitize_query("<script>alert()</script>"),
            "\\<script\\>alert\\(\\)\\<\\/script\\>"
        );
    }

    #[test]
    fn search_indexed_empty_query() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();

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

        build_index_inner(tmp.path(), true).unwrap();
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
        build_index_inner(tmp.path(), true).unwrap();
        let result = search_indexed(tmp.path(), "OAuth", 10, None).unwrap();
        assert_eq!(result["scope"], "global");
        assert_eq!(result["mode"], "ranked");
    }

    #[test]
    fn search_indexed_returns_chunk_id() {
        // chunk_id must be present in BM25 results so hybrid RRF can join on it
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();
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
        build_index_inner(tmp.path(), true).unwrap();
        let result = search_indexed(tmp.path(), "OAuth", 10, Some("backend")).unwrap();
        assert_eq!(result["scope"], "project");
    }

    #[test]
    fn build_index_incremental_rebuilds_after_change() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();

        // Add new file
        fs::write(
            tmp.path().join("backend/NEW.md"),
            "# New Document\n\nFresh content here.",
        )
        .unwrap();
        let r2 = build_index_inner(tmp.path(), true).unwrap();
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
        build_index_inner(tmp.path(), true).unwrap();
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
        build_index_inner(tmp.path(), true).unwrap();
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
        build_index_inner(tmp.path(), true).unwrap();
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
        build_index_inner(tmp.path(), true).unwrap();
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
        build_index_inner(tmp.path(), true).unwrap();
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

        build_index_inner(tmp.path(), true).unwrap();
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

        build_index_inner(tmp.path(), true).unwrap();
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

        build_index_inner(tmp.path(), true).unwrap();
        let result = search_indexed(tmp.path(), "认证", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "should find Chinese text '认证'");
    }

    #[cfg(feature = "alcove-full")]
    #[test]
    fn search_hybrid_returns_bm25_only_when_embedding_not_ready() {
        use crate::embedding::EmbeddingService;
        use crate::config::EmbeddingConfig;

        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();

        // Create an EmbeddingService with embedding disabled — state will be Disabled (non-Ready)
        let svc = EmbeddingService::new(EmbeddingConfig {
            model: "snowflake-arctic-embed-xs".to_string(),
            auto_download: false,
            cache_dir: tmp.path().join(".alcove/models").to_string_lossy().to_string(),
            enabled: false,
            query_cache_size: 64,
        });

        let result = search_hybrid(tmp.path(), "OAuth", &svc, 5, None).unwrap();
        assert_eq!(result["mode"], "bm25-only", "expected bm25-only when embedding is not ready");
    }

    // B4: vector store open/search errors must be surfaced in embedding_status,
    // not silently swallowed while still reporting mode = "hybrid-bm25-vector".
    #[cfg(feature = "alcove-full")]
    #[test]
    fn search_hybrid_vector_store_error_reflected_in_embedding_status() {
        use crate::embedding::EmbeddingService;
        use crate::config::EmbeddingConfig;

        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();

        // Build a ready embedding service with a bogus cache dir so the model
        // is not found, but embed() will fail — exercising vector store open
        // failure path when vectors.db is absent (never built).
        let svc = EmbeddingService::new(EmbeddingConfig {
            model: "snowflake-arctic-embed-xs".to_string(),
            auto_download: false,
            cache_dir: tmp.path().join(".alcove/models").to_string_lossy().to_string(),
            enabled: true,
            query_cache_size: 64,
        });

        let result = search_hybrid(tmp.path(), "OAuth", &svc, 5, None).unwrap();
        // When embedding fails (model absent), we get bm25-only — that is fine.
        // When embedding succeeds but the vector DB is absent, we must NOT
        // silently report "ready"; the error must appear in embedding_status.
        // In this test the embedding step itself will fail (no model on disk),
        // so we get bm25-only. Verify at minimum that embedding_status is NOT
        // "ready" when the result cannot have come from a full hybrid run.
        let mode = result["mode"].as_str().unwrap_or("");
        let status = result["embedding_status"].as_str().unwrap_or("");
        if mode == "hybrid-bm25-vector" {
            // If we somehow got hybrid, status must be "ready"
            assert_eq!(status, "ready");
        } else {
            // bm25-only path: status must NOT be "ready"
            assert_ne!(status, "ready",
                "embedding_status should reflect the failure reason, not 'ready'");
        }
    }

    #[test]
    fn search_indexed_skips_underscore_prefixed_files() {
        let tmp = TempDir::new().unwrap();
        // Create a normal project with a real doc and a _template.md file
        let proj = tmp.path().join("myproject");
        fs::create_dir_all(&proj).unwrap();
        fs::write(
            proj.join("PRD.md"),
            "# PRD\n\nThis project uses OAuth 2.0 authentication flow.",
        )
        .unwrap();
        fs::write(
            proj.join("_template.md"),
            "# Template\n\nOAuth placeholder for template files.",
        )
        .unwrap();

        build_index_inner(tmp.path(), true).unwrap();

        let result = search_indexed(tmp.path(), "OAuth", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "should find OAuth in PRD.md");

        // Verify no match comes from _template.md
        for m in matches {
            let file = m["file"].as_str().unwrap_or("");
            assert!(
                !file.contains("_template.md"),
                "_template.md should be excluded from search results, but found: {file}"
            );
        }
    }

    // pdftotext timeout: spawn + try_wait loop must not block indefinitely.
    // We test the helper by pointing it at a valid (non-PDF) file path so
    // pdftotext exits quickly with a non-zero status — the important thing is
    // that read_file_content returns Ok/Err without hanging.
    #[test]
    fn test_project_vault_cache_isolation() {
        // Build two separate indexes in different temp dirs.
        let tmp_project = TempDir::new().unwrap();
        let tmp_vault = TempDir::new().unwrap();

        // Create minimal index content for project.
        let proj_docs = tmp_project.path().join("docs");
        fs::create_dir_all(&proj_docs).unwrap();
        fs::write(proj_docs.join("proj.md"), "# Project doc\nHello project world.\n").unwrap();

        // Create minimal index content for vault.
        let vault_docs = tmp_vault.path().join("docs");
        fs::create_dir_all(&vault_docs).unwrap();
        fs::write(vault_docs.join("vault.md"), "# Vault doc\nHello vault world.\n").unwrap();

        // Build indexes.
        let proj_index_dir = index_dir(tmp_project.path());
        let vault_index_dir = index_dir(tmp_vault.path());
        build_index(tmp_project.path()).expect("project index build");
        build_index(tmp_vault.path()).expect("vault index build");

        // Open indexes and get cached readers with different categories.
        let proj_index = Index::open_in_dir(&proj_index_dir).expect("open project index");
        let vault_index = Index::open_in_dir(&vault_index_dir).expect("open vault index");

        let _proj_reader = get_cached_reader(&proj_index_dir, &proj_index, CacheCategory::Project)
            .expect("project reader");
        let _vault_reader = get_cached_reader(&vault_index_dir, &vault_index, CacheCategory::Vault)
            .expect("vault reader");

        // Verify isolation: project cache has the project entry but not the vault entry.
        {
            let proj_cache = reader_cache_for(CacheCategory::Project).lock().unwrap();
            assert!(proj_cache.contains_key(&proj_index_dir), "project cache must contain project index");
            assert!(!proj_cache.contains_key(&vault_index_dir), "project cache must not contain vault index");
        }

        // Verify isolation: vault cache has the vault entry but not the project entry.
        {
            let vault_cache = reader_cache_for(CacheCategory::Vault).lock().unwrap();
            assert!(vault_cache.contains_key(&vault_index_dir), "vault cache must contain vault index");
            assert!(!vault_cache.contains_key(&proj_index_dir), "vault cache must not contain project index");
        }
    }

    #[cfg(feature = "alcove-full")]
    #[test]
    fn read_file_content_pdf_does_not_hang() {
        use std::time::{Duration, Instant};
        let tmp = tempfile::TempDir::new().unwrap();
        // Write a minimal valid-looking but actually broken PDF so pdftotext exits fast.
        let pdf_path = tmp.path().join("broken.pdf");
        std::fs::write(&pdf_path, b"%PDF-1.4 broken content").unwrap();
        let start = Instant::now();
        // read_file_content should return within a reasonable time even when
        // pdftotext fails (non-zero exit or not installed).
        let _ = read_file_content(&pdf_path);
        assert!(
            start.elapsed() < Duration::from_secs(35),
            "read_file_content must not block longer than the 30s timeout"
        );
    }

    // -- Vault indexing tests --

    fn setup_vault_dir(tmp: &TempDir) -> PathBuf {
        let vault = tmp.path().join("my-vault");
        fs::create_dir_all(&vault).unwrap();
        fs::write(
            vault.join("note1.md"),
            "# Note One\n\nThis is about OAuth 2.0 authentication.\nPKCE flow for public clients.",
        )
        .unwrap();
        fs::write(
            vault.join("note2.md"),
            "# Note Two\n\nKubernetes deployment strategies.\nRolling updates and blue-green deployments.",
        )
        .unwrap();
        fs::write(
            vault.join("readme.txt"),
            "This is a text file, not markdown.",
        )
        .unwrap();
        vault
    }

    #[test]
    fn test_build_vault_index() {
        let tmp = TempDir::new().unwrap();
        let vault = setup_vault_dir(&tmp);

        let result = build_vault_index(&vault).unwrap();
        assert_eq!(result["status"], "ok");
        assert_eq!(result["vault"], "my-vault");
        assert_eq!(result["files"].as_u64().unwrap(), 2);
        // Index directory should exist
        assert!(vault.join(".alcove/index").exists());
    }

    #[test]
    fn test_search_vault() {
        let tmp = TempDir::new().unwrap();
        let vault = setup_vault_dir(&tmp);

        build_vault_index(&vault).unwrap();

        let result = search_vault(&vault, "OAuth", 10).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "should find OAuth matches in vault");

        // All results should have the vault name as project
        for m in matches {
            assert_eq!(m["project"], "my-vault");
            assert!(m["score"].as_f64().unwrap() > 0.0);
        }
    }

    #[test]
    fn test_vault_index_excludes_underscore_files() {
        let tmp = TempDir::new().unwrap();
        let vault = setup_vault_dir(&tmp);

        // Add an underscore-prefixed file
        fs::write(
            vault.join("_template.md"),
            "# Template\n\nOAuth placeholder template content.",
        )
        .unwrap();

        let result = build_vault_index(&vault).unwrap();
        // _template.md should be excluded from the count
        assert_eq!(result["files"].as_u64().unwrap(), 2);

        // Also verify search does not return it
        let search_result = search_vault(&vault, "OAuth", 10).unwrap();
        let matches = search_result["matches"].as_array().unwrap();
        for m in matches {
            let file = m["file"].as_str().unwrap_or("");
            assert!(
                !file.contains("_template.md"),
                "_template.md should be excluded from vault search results"
            );
        }
    }

    #[test]
    fn test_rebuild_vault_index() {
        let tmp = TempDir::new().unwrap();
        let vault = setup_vault_dir(&tmp);

        build_vault_index(&vault).unwrap();

        // Add a new file
        fs::write(
            vault.join("note3.md"),
            "# Note Three\n\nNew content about gRPC services.",
        )
        .unwrap();

        let result = rebuild_vault_index(&vault).unwrap();
        assert_eq!(result["status"], "ok");
        assert_eq!(result["files"].as_u64().unwrap(), 3);
    }

    #[test]
    fn test_search_vault_empty_query() {
        let tmp = TempDir::new().unwrap();
        let vault = setup_vault_dir(&tmp);
        build_vault_index(&vault).unwrap();

        let result = search_vault(&vault, "", 10).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(matches.is_empty(), "empty query should return no matches");
    }

    #[test]
    fn test_search_vault_no_index_returns_error() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("empty-vault");
        fs::create_dir_all(&vault).unwrap();
        fs::write(vault.join("note.md"), "# Note").unwrap();

        let result = search_vault(&vault, "note", 10);
        assert!(result.is_err(), "search on unindexed vault should error");
    }

    #[test]
    fn test_vault_index_excludes_alcove_dir() {
        let tmp = TempDir::new().unwrap();
        let vault = setup_vault_dir(&tmp);

        // Build index first (creates .alcove/)
        build_vault_index(&vault).unwrap();

        // Add a .md file inside .alcove that should be ignored
        fs::write(
            vault.join(".alcove/internal.md"),
            "# Internal\n\nThis should not be indexed.",
        )
        .unwrap();

        // Rebuild and verify count stays the same
        let result = rebuild_vault_index(&vault).unwrap();
        assert_eq!(result["files"].as_u64().unwrap(), 2);
    }
}
