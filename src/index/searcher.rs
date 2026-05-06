use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Value as JsonValue, json};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{self, Field};
use tantivy::{DocAddress, Index, Score, TantivyDocument};

use super::cache::{CacheCategory, get_cached_reader};
use super::lock::{index_dir, is_locked};
use super::schema::register_ngram_tokenizer;

// ---------------------------------------------------------------------------
// Query sanitization
// ---------------------------------------------------------------------------

/// Escape special characters in tantivy query syntax.
/// Characters like +, -, (, ), {, }, [, ], ^, ~, *, ?, \, /, : have special
/// meaning in the tantivy query parser. We escape them so user input is treated
/// as a literal phrase search.
pub(crate) fn sanitize_query(query: &str) -> String {
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

/// Build a multi-field BooleanQuery that searches `body`, `title`, and `filename`
/// with different boost weights so that title/filename matches rank higher.
///
/// We need separate QueryParser instances per field because Tantivy's QueryParser
/// distributes boost evenly across all its fields. To get per-field boost control,
/// we use individual parsers and wrap results with BoostQuery.
pub(crate) fn build_search_query(
    sanitized: &str,
    index: &Index,
    body_field: Field,
    title_field: Field,
    filename_field: Field,
) -> Box<dyn tantivy::query::Query> {
    use tantivy::query::{BoostQuery, BooleanQuery, Occur};

    let mut clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();

    // Body field (weight 1.0)
    let body_parser = QueryParser::for_index(index, vec![body_field]);
    if let Ok(q) = body_parser.parse_query(sanitized) {
        clauses.push((Occur::Should, q));
    }

    // Title field (weight 3.0)
    let title_parser = QueryParser::for_index(index, vec![title_field]);
    if let Ok(q) = title_parser.parse_query(sanitized) {
        clauses.push((Occur::Should, Box::new(BoostQuery::new(q, 3.0))));
    }

    // Filename field (weight 2.0)
    let filename_parser = QueryParser::for_index(index, vec![filename_field]);
    if let Ok(q) = filename_parser.parse_query(sanitized) {
        clauses.push((Occur::Should, Box::new(BoostQuery::new(q, 2.0))));
    }

    Box::new(BooleanQuery::new(clauses))
}

// ---------------------------------------------------------------------------
// Project diversity filter
// ---------------------------------------------------------------------------

/// Remove results so that no project appears more than `max_per_project` times.
pub(crate) fn apply_project_diversity(results: &mut Vec<serde_json::Value>, max_per_project: usize) {
    let mut project_counts: HashMap<String, usize> = HashMap::new();
    results.retain(|r| {
        let project = r["project"].as_str().unwrap_or("");
        let count = project_counts.entry(project.to_string()).or_insert(0);
        if *count >= max_per_project {
            return false;
        }
        *count += 1;
        true
    });
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
pub(crate) fn search_with_index(
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

    // Resolve optional fields — older indexes may lack them.
    let title_field = schema.get_field("title").ok();
    let filename_field = schema.get_field("filename").ok();

    // Use the process-level cached reader (creates one on first call per path).
    let reader = get_cached_reader(&dir, index, CacheCategory::Project)?;
    let searcher = reader.searcher();

    // Build the search query: multi-field if title/filename fields exist,
    // fall back to body-only for older indexes.
    let parsed_query: Box<dyn tantivy::query::Query> =
        if let (Some(tf), Some(ff)) = (title_field, filename_field) {
            build_search_query(sanitized, index, body_field, tf, ff)
        } else {
            let body_parser = QueryParser::for_index(index, vec![body_field]);
            body_parser
                .parse_query(sanitized)
                .context("Failed to parse search query")?
        };

    // Fetch 3x candidates for per-file deduplication.
    // Using 3x instead of 5x reduces wasted doc fetches while still giving
    // enough headroom to deduplicate down to `limit` unique files.
    let top_docs: Vec<(Score, DocAddress)> = searcher
        .search(&parsed_query, &TopDocs::with_limit(limit * 3).order_by_score())
        .context("Search failed")?;

    // Deduplicate: keep only the best-scoring chunk per (project, file) pair.
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

        if results.len() >= limit * 2 {
            break;
        }
    }

    // Project diversity: limit same-project results when no project_filter is set.
    if project_filter.is_none() {
        apply_project_diversity(&mut results, 2);
    }
    results.truncate(limit);

    // Note: after project diversity filtering, results may be fewer than `limit`
    // even though the original candidate set was larger. The `truncated` flag
    // indicates whether the final (post-diversity) result set hit the limit.
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

    // Hybrid fallback: if one side is empty, report the appropriate mode.
    let mode = if bm25_results.is_empty() && !vector_results.is_empty() {
        "vector-only"
    } else if !bm25_results.is_empty() && vector_results.is_empty() {
        "bm25-only"
    } else {
        "hybrid-bm25-vector"
    };

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
        "mode": mode,
        "embedding_status": match &vector_error {
            Some(e) => e.as_str(),
            None => "ready",
        },
        "matches": results,
        "truncated": truncated,
    }))
}

// ---------------------------------------------------------------------------
// Vault search
// ---------------------------------------------------------------------------

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
                let mut snippet_cache: std::collections::HashMap<SnippetKey, (String, u64)> = std::collections::HashMap::new();
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
pub(crate) fn search_vault_bm25_inner(
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

    let reader = get_cached_reader(&dir, index, CacheCategory::Vault)?;
    let searcher = reader.searcher();

    // Resolve optional fields — older indexes may lack them.
    let title_field = schema.get_field("title").ok();
    let filename_field = schema.get_field("filename").ok();

    let parsed_query: Box<dyn tantivy::query::Query> =
        if let (Some(tf), Some(ff)) = (title_field, filename_field) {
            build_search_query(sanitized, index, body_field, tf, ff)
        } else {
            let body_parser = QueryParser::for_index(index, vec![body_field]);
            body_parser
                .parse_query(sanitized)
                .context("Failed to parse vault search query")?
        };

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
