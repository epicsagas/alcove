use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

type ProjectFile = (String, String, PathBuf);
type ChunkedFile = (String, String, Vec<(usize, super::chunker::Chunk)>);
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use tantivy::query::{BooleanQuery, Occur, TermQuery};
use tantivy::schema::{Field, IndexRecordOption};
use tantivy::{Index, TantivyDocument};
use walkdir::WalkDir;

struct IndexFields {
    project: Field,
    file: Field,
    filename: Field,
    title: Field,
    chunk_id: Field,
    body: Field,
    line_start: Field,
}

#[cfg(feature = "embed")]
struct VectorCounters {
    vectors_indexed: u64,
    vector_errors: u64,
}

#[cfg(not(test))]
use crate::config::load_config;
use crate::config::{effective_config, is_reserved_dir_name};

use super::cache::{CacheCategory, invalidate_reader_cache};
#[cfg(feature = "embed")]
use super::chunker::{ChunkConfig, chunk_content_with_config};
use super::chunker::{chunk_content, extract_title};
use super::frontmatter::parse_frontmatter_flags;
use super::lock::{index_dir, is_locked, meta_path, release_lock, try_acquire_lock};
use super::reader::read_file_content;
use super::schema::{
    CHUNK_STRATEGY_VERSION, IndexSchema, SCHEMA_VERSION, register_ngram_tokenizer,
};

// ---------------------------------------------------------------------------
// Index metadata (for incremental updates)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct IndexMeta {
    pub(crate) files: HashMap<String, [u64; 2]>, // path -> [mtime_secs, size]
    #[serde(default)]
    pub(crate) schema_version: u32,
    #[serde(default)]
    pub(crate) chunk_strategy_version: u32,
    /// Fingerprints of files whose vectors were last brought fully in sync.
    /// Tracked apart from `files` so BM25-only runs never hide pending
    /// embeddings, and edited or deleted docs get their vectors replaced.
    #[serde(default)]
    pub(crate) vector_files: HashMap<String, [u64; 2]>,
}

impl IndexMeta {
    pub(crate) fn load(docs_root: &Path) -> Self {
        let path = meta_path(docs_root);
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub(crate) fn save(&self, docs_root: &Path) -> Result<()> {
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

pub(crate) fn file_fingerprint(path: &Path) -> [u64; 2] {
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

/// Split a docs-root-relative meta key (`<project>/<path>`) into the
/// `(project, path-within-project)` pair stored in the index.
pub(crate) fn split_root_key(key: &str) -> Option<(String, String)> {
    let mut parts = Path::new(key).components();
    let project = parts.next()?.as_os_str().to_string_lossy().to_string();
    let rest = parts.as_path().to_string_lossy().to_string();
    (!rest.is_empty()).then_some((project, rest))
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
        if is_reserved_dir_name(&name) {
            continue;
        }
        let proj_cfg = effective_config(&path);
        let docs_root_canonical = docs_root
            .canonicalize()
            .unwrap_or_else(|_| docs_root.to_path_buf());
        for walk_entry in WalkDir::new(&path)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| {
                e.file_type().is_file()
                    && proj_cfg.is_indexable(e.path())
                    && !e
                        .path()
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .starts_with('_')
            })
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

/// True when vectors lag behind the BM25 file set and an index run could
/// close the gap now (embeddings enabled, model already on disk). Requiring a
/// local model keeps background refresh from retrying every check while a
/// download is impossible.
fn vectors_pending(meta: &IndexMeta, vault_path: Option<&Path>) -> bool {
    #[cfg(all(feature = "embed", not(test)))]
    {
        if meta.vector_files == meta.files {
            return false;
        }
        let cfg = match vault_path {
            Some(v) => crate::config::vault_embedding_config(v),
            None => crate::config::load_config().embedding_config_with_defaults(),
        };
        crate::embedding::is_model_available(&cfg)
    }
    // Tests never read the user's config or model cache.
    #[cfg(any(not(feature = "embed"), test))]
    {
        let _ = (meta, vault_path);
        false
    }
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
    if vectors_pending(&meta, None) {
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
        if is_reserved_dir_name(&name) {
            continue;
        }
        let proj_cfg = effective_config(&path);
        let docs_root_canonical = docs_root
            .canonicalize()
            .unwrap_or_else(|_| docs_root.to_path_buf());
        for walk_entry in WalkDir::new(&path)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| {
                e.file_type().is_file()
                    && proj_cfg.is_indexable(e.path())
                    && !e
                        .path()
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .starts_with('_')
            })
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
        eprintln!(
            "[alcove] index is stale but a rebuild is already in progress — serving stale results"
        );
        return true;
    }
    // Index is stale and no rebuild is running: rebuild synchronously.
    let _ = build_index(docs_root);
    true
}

/// Last staleness check per index root, shared by docs-root and vault refreshes.
pub(crate) type RefreshClock = std::sync::Mutex<HashMap<PathBuf, std::time::Instant>>;
static REFRESH_CLOCK: std::sync::LazyLock<RefreshClock> =
    std::sync::LazyLock::new(Default::default);

/// `[index] refresh_check_secs` as a duration, or `None` when
/// `[index] auto_refresh = false`.
fn refresh_interval() -> Option<Duration> {
    #[cfg(not(test))]
    let cfg = load_config().index_config_with_defaults();
    // Tests never read the user's real config.
    #[cfg(test)]
    let cfg = crate::config::IndexConfig::default();
    cfg.auto_refresh
        .then(|| Duration::from_secs(cfg.refresh_check_secs))
}

/// Incrementally refresh a docs-root index in the background when docs changed
/// or vectors lag behind.
///
/// Searches call this and keep serving the current index; the staleness walk
/// and any rebuild run on a separate thread so the request never blocks on
/// embedding. Controlled by `[index] auto_refresh` and `refresh_check_secs`;
/// `build_index`'s lock keeps a refresh from overlapping another run.
pub fn refresh_index_in_background(docs_root: &Path) {
    if let Some(interval) = refresh_interval() {
        let _ = spawn_refresh_if_due(docs_root, &REFRESH_CLOCK, interval, refresh_docs_root);
    }
}

/// Vault counterpart of [`refresh_index_in_background`].
pub fn refresh_vault_index_in_background(vault_path: &Path) {
    if let Some(interval) = refresh_interval() {
        let _ = spawn_refresh_if_due(vault_path, &REFRESH_CLOCK, interval, refresh_vault);
    }
}

pub(crate) fn refresh_docs_root(root: &Path) {
    if !is_locked(root) && is_index_stale(root) {
        let _ = build_index(root);
    }
}

pub(crate) fn refresh_vault(root: &Path) {
    // Skip (rather than wait) when another build holds the vault lock.
    if root.is_dir() && is_vault_index_stale(root) && try_acquire_lock(root) {
        let _ = build_vault_index_unlocked(root);
        release_lock(root);
    }
}

pub(crate) fn spawn_refresh_if_due(
    root: &Path,
    clock: &RefreshClock,
    interval: Duration,
    job: fn(&Path),
) -> Option<std::thread::JoinHandle<()>> {
    {
        let mut last = clock.lock().unwrap_or_else(|e| e.into_inner());
        if last.get(root).is_some_and(|t| t.elapsed() < interval) {
            return None;
        }
        last.insert(root.to_path_buf(), std::time::Instant::now());
    }
    let root = root.to_path_buf();
    Some(std::thread::spawn(move || job(&root)))
}

// ---------------------------------------------------------------------------
// Build / rebuild index
// ---------------------------------------------------------------------------

pub fn build_index(docs_root: &Path) -> Result<JsonValue> {
    build_index_with_mode(docs_root, false)
}

/// Move `embedding_cache.db` (plus its WAL/SHM sidecars) aside to `.bak`
/// during a force rebuild, so stale embeddings are not reused after a model
/// or prefix change (e.g. llm-kernel 0.25 query-prefix addition).
///
/// Returns the backup path if the cache existed and was moved, `None` if there
/// was nothing to back up. An existing `.bak` from a prior rebuild is replaced.
fn backup_embedding_cache(alcove_dir: &Path) -> Result<Option<PathBuf>> {
    let cache_path = alcove_dir.join("embedding_cache.db");
    if !cache_path.exists() {
        return Ok(None);
    }
    let backup_path = alcove_dir.join("embedding_cache.db.bak");
    if backup_path.exists() {
        std::fs::remove_file(&backup_path)?;
    }
    // Move the main DB plus SQLite WAL-mode sidecars so the backup is complete.
    for suffix in ["", "-wal", "-shm"] {
        let src = alcove_dir.join(format!("embedding_cache.db{suffix}"));
        if src.exists() {
            std::fs::rename(
                &src,
                alcove_dir.join(format!("embedding_cache.db{suffix}.bak")),
            )?;
        }
    }
    Ok(Some(backup_path))
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

fn build_index_with_options(
    docs_root: &Path,
    force_rebuild: bool,
    skip_embedding: bool,
) -> Result<JsonValue> {
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
        // Back up the embedding cache so stale embeddings (e.g. from a prefix
        // change) are not reused; it is regenerated during re-indexing.
        backup_embedding_cache(&docs_root.join(".alcove"))?;
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

pub(crate) fn build_index_inner(docs_root: &Path, skip_embedding: bool) -> Result<JsonValue> {
    let dir = index_dir(docs_root);
    std::fs::create_dir_all(&dir)?;

    let IndexSchema {
        schema,
        project: project_field,
        file: file_field,
        filename: filename_field,
        title: title_field,
        chunk_id: chunk_id_field,
        body: body_field,
        line_start: line_start_field,
    } = IndexSchema::build();

    let mut meta = IndexMeta::load(docs_root);
    apply_schema_migration(&dir, &mut meta)?;

    let (all_files, project_count) = scan_all_files(docs_root)?;

    let (files_to_index, current_files, skipped_count) =
        filter_changed_files(all_files.clone(), docs_root, &meta);

    let needs_full_rebuild = !dir.join("meta.json").exists() || meta.files.is_empty();

    // Docs that were indexed before but are gone now (deleted, moved, or
    // newly excluded) must leave the BM25 index too.
    let removed_files: Vec<(String, String)> = if needs_full_rebuild {
        Vec::new()
    } else {
        meta.files
            .keys()
            .filter(|k| !current_files.contains_key(*k))
            .filter_map(|k| split_root_key(k))
            .collect()
    };

    let indexed_count = write_tantivy_index(
        &dir,
        schema,
        &IndexFields {
            project: project_field,
            file: file_field,
            filename: filename_field,
            title: title_field,
            chunk_id: chunk_id_field,
            body: body_field,
            line_start: line_start_field,
        },
        &files_to_index,
        &removed_files,
        needs_full_rebuild,
    )?;

    // Persist BM25 progress before the (possibly long) vector stage so the
    // index is usable and a later run does not redo it. `vector_files` is
    // left as is until vectors are actually in sync.
    meta.files = current_files.clone();
    meta.schema_version = SCHEMA_VERSION;
    meta.chunk_strategy_version = CHUNK_STRATEGY_VERSION;
    let _ = meta.save(docs_root);

    // Count projects that opted out of vector indexing (vector_index = false).
    let embedding_enabled_for_count = {
        #[cfg(feature = "embed")]
        {
            crate::config::load_config()
                .embedding_config_with_defaults()
                .enabled
        }
        #[cfg(not(feature = "embed"))]
        {
            false
        }
    };
    let vector_skipped_projects =
        count_vector_skipped_projects(docs_root, embedding_enabled_for_count);

    let vectors = run_vector_indexing(docs_root, skip_embedding, &all_files, &meta)?;
    if vectors.synced {
        meta.vector_files = current_files;
    }

    // Document relationship graph (wikilinks/backlinks). Built from the full
    // file set so the filename map covers every indexed doc; incremental runs
    // reprocess only the changed files (full rebuild on any file-set change).
    #[cfg(feature = "doc-graph")]
    let doc_graph_status =
        match super::graph::rebuild_doc_graph(docs_root, &all_files, Some(&files_to_index)) {
            Ok(status) => status,
            Err(e) => {
                eprintln!("[alcove] doc graph build failed: {e}");
                format!("error: {e}")
            }
        };

    // Final metadata save after all indexing steps (Tantivy + Vector) are complete
    let _ = meta.save(docs_root);

    let result = json!({
        "status": "ok",
        "projects": project_count,
        "indexed": indexed_count,
        "skipped": skipped_count,
        "index_path": dir.to_string_lossy(),
        "vector_status": vectors.status,
        "vectors_indexed": vectors.indexed,
        "vector_errors": vectors.errors,
        "embedding_model": vectors.model,
        "vector_skipped_projects": vector_skipped_projects,
    });
    #[cfg(feature = "doc-graph")]
    let result = {
        let mut r = result;
        r["doc_graph"] = json!(doc_graph_status);
        r
    };
    Ok(result)
}

/// Step 1: Apply schema migration — wipe index dir if schema or chunking strategy is outdated.
fn apply_schema_migration(dir: &Path, meta: &mut IndexMeta) -> Result<()> {
    if meta.schema_version < SCHEMA_VERSION || meta.chunk_strategy_version < CHUNK_STRATEGY_VERSION
    {
        if dir.exists() {
            let _ = std::fs::remove_dir_all(dir);
            std::fs::create_dir_all(dir)?;
        }
        meta.files.clear();
    }
    Ok(())
}

/// Step 2: Walk docs_root and collect all indexable files across all projects.
/// Returns (all_files, project_count).
fn scan_all_files(docs_root: &Path) -> Result<(Vec<ProjectFile>, u64)> {
    let mut all_files: Vec<ProjectFile> = Vec::new();
    let mut project_count = 0u64;
    let docs_root_canonical = docs_root
        .canonicalize()
        .unwrap_or_else(|_| docs_root.to_path_buf());

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
        if is_reserved_dir_name(&name) {
            continue;
        }
        project_count += 1;

        let proj_cfg = effective_config(&path);
        for walk_entry in WalkDir::new(&path).into_iter().flatten().filter(|e| {
            e.file_type().is_file()
                && proj_cfg.is_indexable(e.path())
                && !e
                    .path()
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .starts_with('_')
                // `memory/` dirs (global or per-project) are agent-memory
                // storage — indexed only by their own vault index, never here.
                && !e
                    .path()
                    .components()
                    .any(|c| c.as_os_str() == "memory")
        }) {
            let file_path = walk_entry.path().to_path_buf();
            if let Ok(canonical) = file_path.canonicalize()
                && !canonical.starts_with(&docs_root_canonical)
            {
                continue;
            }
            // Skip markdown files whose front matter marks them as draft or deprecated.
            if file_path.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
            }) && std::fs::read_to_string(&file_path)
                .is_ok_and(|c| parse_frontmatter_flags(&c).should_skip)
            {
                continue;
            }
            let rel_to_project = file_path
                .strip_prefix(&path)
                .unwrap_or(&file_path)
                .to_string_lossy()
                .to_string();
            all_files.push((name.clone(), rel_to_project, file_path));
        }
    }

    Ok((all_files, project_count))
}

/// Step 3: Separate files into those that need re-indexing vs. unchanged.
/// Returns (files_to_index, current_files_fingerprints, skipped_count).
fn filter_changed_files(
    all_files: Vec<ProjectFile>,
    docs_root: &Path,
    meta: &IndexMeta,
) -> (Vec<ProjectFile>, HashMap<String, [u64; 2]>, u64) {
    let mut current_files: HashMap<String, [u64; 2]> = HashMap::new();
    let mut files_to_index: Vec<ProjectFile> = Vec::new();
    let mut skipped_count = 0u64;

    for (proj, rel_to_proj, full_path) in all_files {
        let rel_to_root = full_path
            .strip_prefix(docs_root)
            .unwrap_or(&full_path)
            .to_string_lossy()
            .to_string();
        let fp = file_fingerprint(&full_path);
        current_files.insert(rel_to_root.clone(), fp);

        if meta.files.get(&rel_to_root).copied() == Some(fp)
            && meta.schema_version >= SCHEMA_VERSION
            && meta.chunk_strategy_version >= CHUNK_STRATEGY_VERSION
        {
            skipped_count += 1;
        } else {
            files_to_index.push((proj, rel_to_proj, full_path));
        }
    }

    (files_to_index, current_files, skipped_count)
}

/// Step 4: Write changed files into the Tantivy (BM25) index.
/// Returns the number of files indexed.
#[allow(clippy::too_many_arguments)]
fn open_or_create_index(
    dir: &Path,
    schema: tantivy::schema::Schema,
    needs_full_rebuild: bool,
    pb: &ProgressBar,
) -> Result<Index> {
    if needs_full_rebuild {
        pb.set_message("initializing...");
        Ok(Index::create_in_dir(dir, schema.clone()).or_else(|_| {
            std::fs::remove_dir_all(dir)?;
            std::fs::create_dir_all(dir)?;
            Index::create_in_dir(dir, schema.clone())
        })?)
    } else {
        Ok(Index::open_in_dir(dir)?)
    }
}

fn chunk_files_parallel(files_to_index: &[ProjectFile]) -> Vec<ChunkedFile> {
    use rayon::prelude::*;
    files_to_index
        .par_iter()
        .filter_map(|(proj, rel, full)| {
            let ext = full
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            read_file_content(full).ok().map(|content| {
                let chunks = chunk_content(&content, &ext)
                    .into_iter()
                    .enumerate()
                    .collect();
                (proj.clone(), rel.clone(), chunks)
            })
        })
        .collect()
}

fn write_chunks_to_index(
    writer: &mut tantivy::IndexWriter,
    all_chunks: &[ChunkedFile],
    fields: &IndexFields,
    needs_full_rebuild: bool,
    pb: &ProgressBar,
) -> Result<u64> {
    let mut indexed_count = 0u64;
    for (proj, rel, chunks) in all_chunks {
        let ext = Path::new(rel)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("md")
            .to_lowercase();
        let label = match ext.as_str() {
            "pdf" => format!("{proj}  pdf"),
            "docx" => format!("{proj}  docx"),
            "xlsx" => format!("{proj}  xlsx"),
            _ => proj.clone(),
        };
        pb.set_message(label);
        if !needs_full_rebuild {
            delete_file_docs(writer, fields, proj, rel)?;
        }
        for (chunk_idx, chunk) in chunks {
            let title_text = extract_title(&chunk.text, rel, *chunk_idx);
            let filename_text = Path::new(rel)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(rel)
                .to_string();
            let mut doc = TantivyDocument::new();
            doc.add_text(fields.project, proj);
            doc.add_text(fields.file, rel);
            doc.add_text(fields.filename, &filename_text);
            doc.add_text(fields.title, &title_text);
            doc.add_u64(fields.chunk_id, *chunk_idx as u64);
            doc.add_text(fields.body, &chunk.text);
            doc.add_u64(fields.line_start, chunk.line_start as u64);
            writer.add_document(doc)?;
        }
        indexed_count += 1;
        pb.inc(1);
    }
    Ok(indexed_count)
}

/// Delete every chunk of one file from the BM25 index.
fn delete_file_docs(
    writer: &mut tantivy::IndexWriter,
    fields: &IndexFields,
    proj: &str,
    rel: &str,
) -> Result<()> {
    let proj_term = tantivy::Term::from_field_text(fields.project, proj);
    let file_term = tantivy::Term::from_field_text(fields.file, rel);
    let delete_query = BooleanQuery::new(vec![
        (
            Occur::Must,
            Box::new(TermQuery::new(proj_term, IndexRecordOption::Basic))
                as Box<dyn tantivy::query::Query>,
        ),
        (
            Occur::Must,
            Box::new(TermQuery::new(file_term, IndexRecordOption::Basic))
                as Box<dyn tantivy::query::Query>,
        ),
    ]);
    writer.delete_query(Box::new(delete_query))?;
    Ok(())
}

fn write_tantivy_index(
    dir: &Path,
    schema: tantivy::schema::Schema,
    fields: &IndexFields,
    files_to_index: &[ProjectFile],
    removed_files: &[(String, String)],
    needs_full_rebuild: bool,
) -> Result<u64> {
    if files_to_index.is_empty() && removed_files.is_empty() {
        return Ok(0);
    }
    let pb = ProgressBar::new(files_to_index.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("  {spinner:.cyan}  {bar:35.white/dim}  {pos:>3}/{len}  {msg:.dim}")?
            .progress_chars("━━╌"),
    );
    pb.enable_steady_tick(Duration::from_millis(80));
    let index = open_or_create_index(dir, schema, needs_full_rebuild, &pb)?;
    register_ngram_tokenizer(&index)?;
    // Tests: 1 thread / 15 MB to avoid RAM exhaustion from many parallel test indices.
    #[cfg(not(test))]
    let mut writer = index.writer(load_config().index_buffer_bytes())?;
    #[cfg(test)]
    let mut writer = index.writer_with_num_threads(1, 15_000_000)?;
    for (proj, rel) in removed_files {
        delete_file_docs(&mut writer, fields, proj, rel)?;
    }
    let all_chunks = chunk_files_parallel(files_to_index);
    let indexed_count =
        write_chunks_to_index(&mut writer, &all_chunks, fields, needs_full_rebuild, &pb)?;
    pb.set_message("saving...");
    writer.commit()?;
    pb.finish_and_clear();
    Ok(indexed_count)
}

/// Embeds `pending` chunks and upserts them into the vector store.
/// Drains `pending` on success; clears it on embed error.
/// Updates `vectors_indexed` and `vector_errors` in place.
#[cfg(feature = "embed")]
fn flush_embed_batch(
    pending: &mut Vec<([u8; 32], String, String, u64, String)>,
    service: &crate::embedding::EmbeddingService,
    store: &mut crate::vector::VectorStore,
    model: &crate::embedding::EmbeddingModel,
    counters: &mut VectorCounters,
    cache: &crate::embedding_cache::EmbeddingCache,
    cached: &mut Vec<(String, String, u64, Vec<f32>)>,
) {
    if pending.is_empty() {
        return;
    }

    let model_name = model.as_str();
    let doc_pfx = model.doc_prefix();

    // Separate cache hits from misses.
    let mut miss_hashes: Vec<[u8; 32]> = Vec::new();
    let mut miss_items: Vec<(String, String, u64, String)> = Vec::new();

    for (hash, proj, rel, chunk_id, text) in pending.drain(..) {
        match cache.get(&hash, model_name) {
            Ok(Some(emb)) => cached.push((proj, rel, chunk_id, emb)),
            _ => {
                miss_hashes.push(hash);
                miss_items.push((proj, rel, chunk_id, text));
            }
        }
    }

    // Upsert cached results immediately.
    if !cached.is_empty() {
        let count = cached.len() as u64;
        if let Err(e) = store.batch_upsert(cached.drain(..)) {
            eprintln!("[alcove] batch upsert (cached) failed: {}", e);
            counters.vector_errors += count;
        } else {
            counters.vectors_indexed += count;
        }
    }

    if miss_items.is_empty() {
        return;
    }

    // Load ONNX model on first cache miss.
    let _ = service.ensure_model();
    if service.state() != crate::embedding::ModelState::Ready {
        eprintln!("[alcove] embed failed: model not ready");
        counters.vector_errors += miss_items.len() as u64;
        return;
    }

    let count = miss_items.len() as u64;
    let texts: Vec<String> = miss_items
        .iter()
        .map(|(_, _, _, t)| match doc_pfx {
            Some(pfx) => format!("{}{}", pfx, t),
            None => t.clone(),
        })
        .collect();
    let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    match service.embed(&text_refs) {
        Ok(embeddings) => {
            // Store new embeddings in persistent cache.
            if let Err(e) = cache.put_batch(&miss_hashes, model_name, &embeddings) {
                eprintln!("[alcove] embedding cache write failed: {}", e);
            }
            let it = miss_items
                .into_iter()
                .zip(embeddings)
                .map(|((p, r, id, _), emb)| (p, r, id, emb));
            if let Err(e) = store.batch_upsert(it) {
                eprintln!("[alcove] batch upsert failed: {}", e);
                counters.vector_errors += count;
            } else {
                counters.vectors_indexed += count;
            }
        }
        Err(e) => {
            eprintln!("[alcove] embed failed: {}", e);
            counters.vector_errors += count;
        }
    }
}

/// Reads, chunks, and embeds `to_embed` files in batches of `embed_batch`.
#[cfg(feature = "embed")]
#[allow(clippy::too_many_arguments)]
fn embed_files_in_batches(
    to_embed: &[ProjectFile],
    service: &crate::embedding::EmbeddingService,
    store: &mut crate::vector::VectorStore,
    model: &crate::embedding::EmbeddingModel,
    embed_batch: usize,
    vpb: &ProgressBar,
    counters: &mut VectorCounters,
    cache: &crate::embedding_cache::EmbeddingCache,
) -> Vec<(String, String, u64)> {
    // Chunk counts per re-embedded file, so stale trailing chunks can be pruned.
    let mut tails: Vec<(String, String, u64)> = Vec::new();
    // Sequential read + chunk per file, embed in batches.
    // Parallel file reads caused RSS spikes (large PDFs × N threads).
    // Bottleneck is ONNX inference, not I/O — sequential reads keep
    // memory bounded to one file's chunks at a time.
    let chunk_cfg = ChunkConfig::for_max_tokens(model.max_seq_length());
    let doc_pfx = model.doc_prefix();
    // pending: (content_hash, project, rel_path, chunk_id, text)
    let mut pending: Vec<([u8; 32], String, String, u64, String)> = Vec::new();
    // Accumulate cached results for batched upsert.
    let mut cached: Vec<(String, String, u64, Vec<f32>)> = Vec::new();

    for (proj, rel, full) in to_embed {
        vpb.set_message(proj.clone());
        if let Ok(content) = read_file_content(full) {
            let file_ext = full.extension().and_then(|e| e.to_str()).unwrap_or("");
            let chunks = chunk_content_with_config(&content, file_ext, &chunk_cfg);
            tails.push((proj.clone(), rel.clone(), chunks.len() as u64));
            for (i, chunk) in chunks.into_iter().enumerate() {
                // Hash the prefixed text (same text sent to ONNX/embed).
                let prefixed = match doc_pfx {
                    Some(pfx) => format!("{}{}", pfx, chunk.text),
                    None => chunk.text.clone(),
                };
                let hash = blake3::hash(prefixed.as_bytes());
                pending.push((hash.into(), proj.clone(), rel.clone(), i as u64, chunk.text));
                if pending.len() >= embed_batch {
                    flush_embed_batch(
                        &mut pending,
                        service,
                        store,
                        model,
                        counters,
                        cache,
                        &mut cached,
                    );
                }
            }
        }
        vpb.inc(1);
    }
    flush_embed_batch(
        &mut pending,
        service,
        store,
        model,
        counters,
        cache,
        &mut cached,
    );

    // Flush any remaining cached results.
    if !cached.is_empty() {
        let count = cached.len() as u64;
        if let Err(e) = store.batch_upsert(cached.drain(..)) {
            eprintln!("[alcove] final cached upsert failed: {}", e);
            counters.vector_errors += count;
        } else {
            counters.vectors_indexed += count;
        }
    }
    tails
}

/// Result of a vector stage. `synced` means every eligible file is embedded
/// at its current fingerprint and no orphan vectors remain.
pub(crate) struct VectorSync {
    status: String,
    indexed: u64,
    errors: u64,
    model: String,
    synced: bool,
}

impl VectorSync {
    fn not_run(status: &str) -> Self {
        Self {
            status: status.to_string(),
            indexed: 0,
            errors: 0,
            model: String::new(),
            synced: false,
        }
    }
}

/// Bring the vector store under `root/.alcove/` in line with `eligible`
/// (`(meta key, file)` pairs): drop vectors of files no longer eligible,
/// re-embed files whose fingerprint differs from `previous`, and prune
/// trailing chunks of files that shrank. Unchanged chunks come from the
/// persistent embedding cache, so re-embedding an edited file only runs the
/// model on the chunks that actually changed.
#[cfg(feature = "embed")]
fn sync_vectors(
    root: &Path,
    emb_cfg: &crate::config::EmbeddingConfig,
    eligible: &[(String, ProjectFile)],
    current: &HashMap<String, [u64; 2]>,
    previous: &HashMap<String, [u64; 2]>,
) -> Result<VectorSync> {
    use crate::embedding::{EmbeddingService, resolve_model};
    use crate::embedding_cache::EmbeddingCache;
    use crate::vector::VectorStore;

    let model = resolve_model(&emb_cfg.model);
    let model_name = model.as_str().to_string();
    let service = EmbeddingService::new(crate::config::EmbeddingConfig {
        model: emb_cfg.model.clone(),
        auto_download: emb_cfg.auto_download,
        cache_dir: emb_cfg.cache_dir.clone(),
        enabled: true,
        query_cache_size: emb_cfg.query_cache_size,
    });

    let alcove_dir = root.join(".alcove");
    let mut store = VectorStore::open(
        &alcove_dir.join("vectors.db"),
        service.model_name(),
        service.dimension(),
    )
    .map_err(|e| {
        eprintln!("[alcove] Failed to open vector store: {}", e);
        e
    })?;

    let keep: std::collections::HashSet<(&str, &str)> = eligible
        .iter()
        .map(|(_, (p, r, _))| (p.as_str(), r.as_str()))
        .collect();
    let orphans: Vec<(String, String)> = store
        .files()?
        .into_iter()
        .filter(|(p, r)| !keep.contains(&(p.as_str(), r.as_str())))
        .collect();
    let to_embed: Vec<ProjectFile> = eligible
        .iter()
        .filter(|(key, _)| previous.get(key) != current.get(key))
        .map(|(_, f)| f.clone())
        .collect();

    if !orphans.is_empty() {
        store.prune(&orphans, &[])?;
    }

    let mut counters = VectorCounters {
        vectors_indexed: 0,
        vector_errors: 0,
    };
    // Nothing to embed: skip ONNX model loading entirely.
    if !to_embed.is_empty() {
        // Persistent embedding cache (survives full rebuilds). The model
        // itself loads lazily in flush_embed_batch on the first cache miss.
        let cache = EmbeddingCache::open(&alcove_dir.join("embedding_cache.db")).map_err(|e| {
            eprintln!("[alcove] Failed to open embedding cache: {}", e);
            e
        })?;
        let vpb = ProgressBar::new(to_embed.len() as u64);
        vpb.set_style(
            ProgressStyle::default_bar()
                .template("  {spinner:.magenta}  {bar:35.white/dim}  {pos:>3}/{len}  {msg:.dim}")?
                .progress_chars("━━╌"),
        );
        vpb.enable_steady_tick(Duration::from_millis(80));
        vpb.set_message("embedding");
        // Batch size tuned to model size: small < 100 MB → 128, medium ≤ 800 MB → 64, large → 32
        let embed_batch: usize = match model.size_mb() {
            s if s < 100 => 128,
            s if s <= 800 => 64,
            _ => 32,
        };
        let tails = embed_files_in_batches(
            &to_embed,
            &service,
            &mut store,
            &model,
            embed_batch,
            &vpb,
            &mut counters,
            &cache,
        );
        vpb.finish_and_clear();
        store.prune(&[], &tails)?;
    }

    Ok(VectorSync {
        status: "ok".to_string(),
        indexed: store.meta().map(|m| m.count as u64).unwrap_or(0),
        errors: counters.vector_errors,
        model: model_name,
        synced: counters.vector_errors == 0,
    })
}

/// Vector stage for a docs root: every indexable file of a project that has
/// vectors enabled is eligible.
#[cfg(feature = "embed")]
fn run_full_vector_indexing(
    docs_root: &Path,
    all_files: &[ProjectFile],
    meta: &IndexMeta,
) -> Result<VectorSync> {
    use crate::config::{effective_config, load_config};

    let emb_cfg = load_config().embedding_config_with_defaults();
    if !emb_cfg.enabled {
        return Ok(VectorSync::not_run("disabled"));
    }
    let mut project_enabled: HashMap<&str, bool> = HashMap::new();
    let eligible: Vec<(String, ProjectFile)> = all_files
        .iter()
        .filter(|(proj, _, _)| {
            *project_enabled.entry(proj.as_str()).or_insert_with(|| {
                effective_config(&docs_root.join(proj))
                    .vector_index
                    .unwrap_or(emb_cfg.enabled)
            })
        })
        .map(|f| {
            let key =
                f.2.strip_prefix(docs_root)
                    .unwrap_or(&f.2)
                    .to_string_lossy()
                    .to_string();
            (key, f.clone())
        })
        .collect();
    sync_vectors(
        docs_root,
        &emb_cfg,
        &eligible,
        &meta.files,
        &meta.vector_files,
    )
}

/// Count how many project directories under `docs_root` have `vector_index = false`.
/// When `vector_index` is unset (None), `embedding_enabled` is used as the default.
fn count_vector_skipped_projects(docs_root: &Path, embedding_enabled: bool) -> u64 {
    let Ok(entries) = std::fs::read_dir(docs_root) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            let path = e.path();
            if !path.is_dir() {
                return false;
            }
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if is_reserved_dir_name(&name) {
                return false;
            }
            !effective_config(&path)
                .vector_index
                .unwrap_or(embedding_enabled)
        })
        .count() as u64
}

fn run_vector_indexing(
    docs_root: &Path,
    skip_embedding: bool,
    all_files: &[ProjectFile],
    meta: &IndexMeta,
) -> Result<VectorSync> {
    if skip_embedding {
        return Ok(VectorSync::not_run("skipped"));
    }
    #[cfg(feature = "embed")]
    return run_full_vector_indexing(docs_root, all_files, meta);
    #[cfg(not(feature = "embed"))]
    {
        let _ = (docs_root, all_files, meta);
        Ok(VectorSync::not_run("disabled"))
    }
}

// ---------------------------------------------------------------------------
// Vault indexing
// ---------------------------------------------------------------------------

/// Markdown files of a vault that belong in its index: excludes reserved
/// dirs, `_`-prefixed names, and drafts or deprecated docs.
fn scan_vault_files(vault_path: &Path) -> Vec<PathBuf> {
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
                    is_reserved_dir_name(&s)
                })
                .unwrap_or(false)
            {
                return false;
            }
            // Must be a file
            if !e.file_type().is_file() {
                return false;
            }
            // Must be markdown (.md or .markdown)
            if !path.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
            }) {
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
        let file_path = entry.path().to_path_buf();
        // Skip markdown files whose front matter marks them as draft or deprecated.
        if std::fs::read_to_string(&file_path)
            .is_ok_and(|c| parse_frontmatter_flags(&c).should_skip)
        {
            continue;
        }
        files.push(file_path);
    }
    files
}

/// True when a vault's files differ from its index metadata (added, edited,
/// or removed docs) or its vectors lag behind.
pub fn is_vault_index_stale(vault_path: &Path) -> bool {
    if !meta_path(vault_path).exists() {
        return true;
    }
    let meta = IndexMeta::load(vault_path);
    let current: HashMap<String, [u64; 2]> = scan_vault_files(vault_path)
        .iter()
        .map(|fp| {
            let rel = fp
                .strip_prefix(vault_path)
                .unwrap_or(fp)
                .to_string_lossy()
                .to_string();
            (rel, file_fingerprint(fp))
        })
        .collect();
    current != meta.files || vectors_pending(&meta, Some(vault_path))
}

/// Build a BM25 index for a single vault directory.
///
/// Vault files live directly under `vault_path` (flat structure), unlike
/// project docs which are nested under `docs_root/<project>/`.  The vault
/// name (directory basename) is stored in the `project` field for schema
/// compatibility with project indexes.
pub fn build_vault_index(vault_path: &Path) -> Result<JsonValue> {
    // Serialize with background refreshes and other builds of this vault.
    // ponytail: fixed 60 s wait, a queued rebuild if vaults grow large enough
    // for one build to outlast it.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while !try_acquire_lock(vault_path) {
        if std::time::Instant::now() > deadline {
            anyhow::bail!("vault index is busy (another index build is running)");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let result = build_vault_index_unlocked(vault_path);
    release_lock(vault_path);
    result
}

fn build_vault_index_unlocked(vault_path: &Path) -> Result<JsonValue> {
    let vault_name = vault_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let dir = index_dir(vault_path);
    std::fs::create_dir_all(&dir)?;

    let IndexSchema {
        schema,
        project: project_field,
        file: file_field,
        filename: filename_field,
        title: title_field,
        chunk_id: chunk_id_field,
        body: body_field,
        line_start: line_start_field,
    } = IndexSchema::build();

    let files = scan_vault_files(vault_path);

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
            let file_ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
            for (chunk_idx, chunk) in chunk_content(&content, file_ext).iter().enumerate() {
                let title_text = extract_title(&chunk.text, &rel, chunk_idx);
                let filename_text = Path::new(&rel)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&rel)
                    .to_string();
                let mut doc = TantivyDocument::new();
                doc.add_text(project_field, &vault_name);
                doc.add_text(file_field, &rel);
                doc.add_text(filename_field, &filename_text);
                doc.add_text(title_field, &title_text);
                doc.add_u64(chunk_id_field, chunk_idx as u64);
                doc.add_text(body_field, &chunk.text);
                doc.add_u64(line_start_field, chunk.line_start as u64);
                writer.add_document(doc)?;
            }
        }
    }

    writer.commit()?;
    // The index dir was recreated, so a cached reader would keep serving the
    // old snapshot.
    invalidate_reader_cache(vault_path, CacheCategory::Vault);

    // Save metadata for the vault index. BM25 is rebuilt in full above, while
    // vectors are synced incrementally against the previous `vector_files`.
    let mut meta = IndexMeta {
        vector_files: IndexMeta::load(vault_path).vector_files,
        ..IndexMeta::default()
    };
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
    // Vector indexing (embed feature) — uses vault-specific embedding model
    // with fallback to global config.
    // ---------------------------------------------------------------------------
    #[cfg_attr(not(feature = "embed"), allow(unused_mut))]
    let mut vectors = VectorSync::not_run("disabled");

    #[cfg(feature = "embed")]
    {
        let emb_cfg = crate::config::vault_embedding_config(vault_path);
        if emb_cfg.enabled {
            let eligible: Vec<(String, ProjectFile)> = files
                .iter()
                .map(|fp| {
                    let rel = fp
                        .strip_prefix(vault_path)
                        .unwrap_or(fp)
                        .to_string_lossy()
                        .to_string();
                    (rel.clone(), (vault_name.clone(), rel, fp.clone()))
                })
                .collect();
            vectors = match sync_vectors(
                vault_path,
                &emb_cfg,
                &eligible,
                &meta.files,
                &meta.vector_files,
            ) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[alcove] vault vector indexing failed: {}", e);
                    VectorSync::not_run(&format!("error: {}", e))
                }
            };
            if vectors.synced {
                meta.vector_files = meta.files.clone();
                let _ = meta.save(vault_path);
            }
        }
    }

    // Document relationship graph for the vault (same backend as project docs).
    #[cfg(feature = "doc-graph")]
    let vault_doc_graph_status = {
        let vault_files: Vec<ProjectFile> = files
            .iter()
            .map(|fp| {
                let rel = fp
                    .strip_prefix(vault_path)
                    .unwrap_or(fp)
                    .to_string_lossy()
                    .to_string();
                (vault_name.clone(), rel, fp.clone())
            })
            .collect();
        match super::graph::rebuild_doc_graph(vault_path, &vault_files, None) {
            Ok(status) => status,
            Err(e) => {
                eprintln!("[alcove] vault doc graph build failed: {e}");
                format!("error: {e}")
            }
        }
    };

    let result = json!({
        "status": "ok",
        "vault": vault_name,
        "files": file_count,
        "index_path": dir.to_string_lossy(),
        "vectors_indexed": vectors.indexed,
        "vector_errors": vectors.errors,
        "vector_status": vectors.status,
        "embedding_model": vectors.model,
    });
    #[cfg(feature = "doc-graph")]
    let result = {
        let mut r = result;
        r["doc_graph"] = json!(vault_doc_graph_status);
        r
    };
    Ok(result)
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
    // Back up the embedding cache so stale embeddings are not reused.
    backup_embedding_cache(&vault_path.join(".alcove"))?;

    build_vault_index(vault_path)
}
