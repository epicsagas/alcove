use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use tantivy::query::{BooleanQuery, Occur, TermQuery};
use tantivy::schema::{Field, IndexRecordOption};
use tantivy::{Index, TantivyDocument};
use walkdir::WalkDir;

use crate::config::{effective_config, is_reserved_dir_name};
#[cfg(not(test))]
use crate::config::load_config;

use super::cache::{CacheCategory, invalidate_reader_cache};
use super::chunker::{chunk_content, extract_title};
use super::lock::{index_dir, meta_path, is_locked, try_acquire_lock, release_lock};
use super::reader::read_file_content;
use super::schema::{IndexSchema, SCHEMA_VERSION, register_ngram_tokenizer};

// ---------------------------------------------------------------------------
// Index metadata (for incremental updates)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Default)]
pub(crate) struct IndexMeta {
    pub(crate) files: HashMap<String, [u64; 2]>, // path -> [mtime_secs, size]
    #[serde(default)]
    pub(crate) schema_version: u32,
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
        if is_reserved_dir_name(&name) {
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
        filter_changed_files(all_files, docs_root, &meta);

    let needs_full_rebuild = !dir.join("meta.json").exists() || meta.files.is_empty();

    let indexed_count = write_tantivy_index(
        &dir,
        schema,
        project_field,
        file_field,
        filename_field,
        title_field,
        chunk_id_field,
        body_field,
        line_start_field,
        &files_to_index,
        needs_full_rebuild,
    )?;

    let (vector_status, vectors_indexed, vector_errors, embedding_model) =
        run_vector_indexing(docs_root, skip_embedding, files_to_index)?;

    // Final metadata save after all indexing steps (Tantivy + Vector) are complete
    meta.files = current_files;
    meta.schema_version = SCHEMA_VERSION;
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

/// Step 1: Apply schema migration — wipe index dir if schema version is outdated.
fn apply_schema_migration(dir: &Path, meta: &mut IndexMeta) -> Result<()> {
    if meta.schema_version < SCHEMA_VERSION {
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
fn scan_all_files(docs_root: &Path) -> Result<(Vec<(String, String, PathBuf)>, u64)> {
    let mut all_files: Vec<(String, String, PathBuf)> = Vec::new();
    let mut project_count = 0u64;

    for entry in std::fs::read_dir(docs_root)
        .context("Failed to read DOCS_ROOT")?
        .flatten()
    {
        let path = entry.path();
        if !path.is_dir() { continue; }
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        if is_reserved_dir_name(&name) { continue; }
        project_count += 1;

        let proj_cfg = effective_config(&path);
        let docs_root_canonical = docs_root.canonicalize().unwrap_or_else(|_| docs_root.to_path_buf());
        for walk_entry in WalkDir::new(&path).into_iter().flatten().filter(|e| {
            e.file_type().is_file()
                && proj_cfg.is_indexable(e.path())
                && !e.path().file_name().unwrap_or_default().to_string_lossy().starts_with('_')
        }) {
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

    Ok((all_files, project_count))
}

/// Step 3: Separate files into those that need re-indexing vs. unchanged.
/// Returns (files_to_index, current_files_fingerprints, skipped_count).
fn filter_changed_files(
    all_files: Vec<(String, String, PathBuf)>,
    docs_root: &Path,
    meta: &IndexMeta,
) -> (Vec<(String, String, PathBuf)>, HashMap<String, [u64; 2]>, u64) {
    let mut current_files: HashMap<String, [u64; 2]> = HashMap::new();
    let mut files_to_index: Vec<(String, String, PathBuf)> = Vec::new();
    let mut skipped_count = 0u64;

    for (proj, rel_to_proj, full_path) in all_files {
        let rel_to_root = full_path.strip_prefix(docs_root).unwrap_or(&full_path).to_string_lossy().to_string();
        let fp = file_fingerprint(&full_path);
        current_files.insert(rel_to_root.clone(), fp);

        if meta.files.get(&rel_to_root).copied() == Some(fp) && meta.schema_version >= SCHEMA_VERSION {
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

fn chunk_files_parallel(
    files_to_index: &[(String, String, PathBuf)],
) -> Vec<(String, String, Vec<(usize, super::chunker::Chunk)>)> {
    use rayon::prelude::*;
    files_to_index
        .par_iter()
        .filter_map(|(proj, rel, full)| {
            let ext = full.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            read_file_content(full).ok().map(|content| {
                let chunks = chunk_content(&content, &ext).into_iter().enumerate().collect();
                (proj.clone(), rel.clone(), chunks)
            })
        })
        .collect()
}

fn write_chunks_to_index(
    writer: &mut tantivy::IndexWriter,
    all_chunks: &[(String, String, Vec<(usize, super::chunker::Chunk)>)],
    project_field: Field,
    file_field: Field,
    filename_field: Field,
    title_field: Field,
    chunk_id_field: Field,
    body_field: Field,
    line_start_field: Field,
    needs_full_rebuild: bool,
    pb: &ProgressBar,
) -> Result<u64> {
    let mut indexed_count = 0u64;
    for (proj, rel, chunks) in all_chunks {
        let ext = Path::new(rel).extension().and_then(|e| e.to_str()).unwrap_or("md").to_lowercase();
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
        for (chunk_idx, chunk) in chunks {
            let title_text = extract_title(&chunk.text, rel, *chunk_idx);
            let filename_text = Path::new(rel).file_stem().and_then(|s| s.to_str()).unwrap_or(rel).to_string();
            let mut doc = TantivyDocument::new();
            doc.add_text(project_field, proj);
            doc.add_text(file_field, rel);
            doc.add_text(filename_field, &filename_text);
            doc.add_text(title_field, &title_text);
            doc.add_u64(chunk_id_field, *chunk_idx as u64);
            doc.add_text(body_field, &chunk.text);
            doc.add_u64(line_start_field, chunk.line_start as u64);
            writer.add_document(doc)?;
        }
        indexed_count += 1;
        pb.inc(1);
    }
    Ok(indexed_count)
}

fn write_tantivy_index(
    dir: &Path,
    schema: tantivy::schema::Schema,
    project_field: Field,
    file_field: Field,
    filename_field: Field,
    title_field: Field,
    chunk_id_field: Field,
    body_field: Field,
    line_start_field: Field,
    files_to_index: &[(String, String, PathBuf)],
    needs_full_rebuild: bool,
) -> Result<u64> {
    if files_to_index.is_empty() {
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
    let all_chunks = chunk_files_parallel(files_to_index);
    let indexed_count = write_chunks_to_index(
        &mut writer, &all_chunks,
        project_field, file_field, filename_field, title_field,
        chunk_id_field, body_field, line_start_field,
        needs_full_rebuild, &pb,
    )?;
    pb.set_message("saving...");
    writer.commit()?;
    pb.finish_and_clear();
    Ok(indexed_count)
}

/// Embeds `pending` chunks and upserts them into the vector store.
/// Drains `pending` on success; clears it on embed error.
/// Updates `vectors_indexed` and `vector_errors` in place.
#[cfg(feature = "alcove-full")]
fn flush_embed_batch(
    pending: &mut Vec<(String, String, u64, String)>,
    service: &crate::embedding::EmbeddingService,
    store: &mut crate::vector::VectorStore,
    model: &crate::embedding::EmbeddingModelChoice,
    vectors_indexed: &mut u64,
    vector_errors: &mut u64,
) {
    if pending.is_empty() {
        return;
    }
    let count = pending.len() as u64;
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
                *vector_errors += count;
            } else {
                *vectors_indexed += count;
            }
        }
        Err(e) => {
            eprintln!("[alcove] embed failed: {}", e);
            *vector_errors += pending.len() as u64;
            pending.clear();
        }
    }
}

/// Reads, chunks, and embeds `to_embed` files in batches of `embed_batch`.
#[cfg(feature = "alcove-full")]
fn embed_files_in_batches(
    to_embed: &[(String, String, PathBuf)],
    service: &crate::embedding::EmbeddingService,
    store: &mut crate::vector::VectorStore,
    model: &crate::embedding::EmbeddingModelChoice,
    embed_batch: usize,
    vpb: &ProgressBar,
    vectors_indexed: &mut u64,
    vector_errors: &mut u64,
) {
    // Sequential read + chunk per file, embed in batches.
    // Parallel file reads caused RSS spikes (large PDFs × N threads).
    // Bottleneck is ONNX inference, not I/O — sequential reads keep
    // memory bounded to one file's chunks at a time.
    let mut pending: Vec<(String, String, u64, String)> = Vec::new();
    for (proj, rel, full) in to_embed {
        vpb.set_message(proj.clone());
        if let Ok(content) = read_file_content(full) {
            let file_ext = full.extension().and_then(|e| e.to_str()).unwrap_or("");
            for (i, chunk) in chunk_content(&content, file_ext).into_iter().enumerate() {
                pending.push((proj.clone(), rel.clone(), i as u64, chunk.text));
                if pending.len() >= embed_batch {
                    flush_embed_batch(&mut pending, service, store, model, vectors_indexed, vector_errors);
                }
            }
        }
        vpb.inc(1);
    }
    flush_embed_batch(&mut pending, service, store, model, vectors_indexed, vector_errors);
}

/// Runs the full embedding pipeline when the `alcove-full` feature is enabled.
/// Returns updated (vector_status, vectors_indexed, vector_errors, embedding_model).
#[cfg(feature = "alcove-full")]
fn run_full_vector_indexing(
    docs_root: &Path,
    files_to_index: Vec<(String, String, PathBuf)>,
) -> Result<(String, u64, u64, String)> {
    use crate::embedding::{EmbeddingModelChoice, EmbeddingService};
    use crate::vector::VectorStore;
    use crate::config::load_config;

    let cfg = load_config();
    let emb_cfg = cfg.embedding_config_with_defaults();
    if !emb_cfg.enabled {
        return Ok(("disabled".to_string(), 0, 0, String::new()));
    }

    let model = EmbeddingModelChoice::parse(&emb_cfg.model).unwrap_or_default();
    let service = EmbeddingService::new(crate::config::EmbeddingConfig {
        model: model.as_str().to_string(),
        auto_download: emb_cfg.auto_download,
        cache_dir: emb_cfg.cache_dir.clone(),
        enabled: true,
        query_cache_size: emb_cfg.query_cache_size,
    });
    let _ = service.ensure_model();
    if service.state() != crate::embedding::ModelState::Ready {
        return Ok(("model_not_ready".to_string(), 0, 0, model.as_str().to_string()));
    }

    let vector_path = docs_root.join(".alcove").join("vectors.db");
    let mut store = VectorStore::open(&vector_path, service.model_name(), service.dimension())
        .map_err(|e| { eprintln!("[alcove] Failed to open vector store: {}", e); e })?;
    let mut vectors_indexed = 0u64;
    let mut vector_errors = 0u64;

    if !files_to_index.is_empty() {
        let to_embed: Vec<(String, String, PathBuf)> = files_to_index
            .into_iter()
            .filter(|(proj, rel, _)| !matches!(store.has_file(proj, rel), Ok(true)))
            .collect();
        let vpb = ProgressBar::new(to_embed.len() as u64);
        vpb.set_style(
            ProgressStyle::default_bar()
                .template("  {spinner:.magenta}  {bar:35.white/dim}  {pos:>3}/{len}  {msg:.dim}")?
                .progress_chars("━━╌"),
        );
        vpb.enable_steady_tick(Duration::from_millis(80));
        vpb.set_message("embedding");
        // Batch size tuned to model size: small < 100 MB → 128, medium ≤ 800 MB → 64, large → 32
        let embed_batch: usize = match model.size_mb() { s if s < 100 => 128, s if s <= 800 => 64, _ => 32 };
        embed_files_in_batches(&to_embed, &service, &mut store, &model, embed_batch, &vpb, &mut vectors_indexed, &mut vector_errors);
        vpb.finish_and_clear();
    }
    if let Ok(meta) = store.meta() {
        vectors_indexed = meta.count as u64;
    }
    Ok(("ok".to_string(), vectors_indexed, vector_errors, model.as_str().to_string()))
}

fn run_vector_indexing(
    docs_root: &Path,
    skip_embedding: bool,
    files_to_index: Vec<(String, String, PathBuf)>,
) -> Result<(String, u64, u64, String)> {
    if skip_embedding {
        return Ok(("skipped".to_string(), 0, 0, String::new()));
    }
    #[cfg(feature = "alcove-full")]
    return run_full_vector_indexing(docs_root, files_to_index);
    #[cfg(not(feature = "alcove-full"))]
    Ok(("disabled".to_string(), 0, 0, String::new()))
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
