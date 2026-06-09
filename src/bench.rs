use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::is_reserved_dir_name;

use anyhow::{Context, Result};
use console::style;
use serde::{Deserialize, Serialize};
use serde_json::json;
use walkdir::WalkDir;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct RelevantSection {
    pub file: String,
    pub heading: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct GroundTruthEntry {
    pub text: String,
    pub relevant_files: Vec<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub difficulty: Option<String>,
    #[serde(default)]
    pub relevant_sections: Option<Vec<RelevantSection>>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct BenchConfig {
    pub metrics: String,
    pub scope: String,
    pub output: String,
    pub queries_path: Option<PathBuf>,
    pub iterations: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EnvInfo {
    os: String,
    rust_version: String,
    alcove_version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DataStats {
    file_count: u64,
    total_size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrecisionAtK {
    k: usize,
    precision: f64,
    recall: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    ndcg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    map: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PerQueryPrecision {
    query: String,
    precision_at_k: Vec<PrecisionAtK>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mrr: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    difficulty: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PrecisionResults {
    grep: Vec<PerQueryPrecision>,
    ranked: Vec<PerQueryPrecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hybrid: Option<Vec<PerQueryPrecision>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LatencyEntry {
    query: String,
    avg_us: u128,
    p50_us: u128,
    p95_us: u128,
    p99_us: u128,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LatencyResults {
    grep: Vec<LatencyEntry>,
    ranked: Vec<LatencyEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hybrid: Option<Vec<LatencyEntry>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ThroughputResults {
    full_rebuild_ms: u128,
    incremental_no_change_ms: u128,
    stale_detection_us: u128,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiskUsageResults {
    index_bytes: u64,
    docs_bytes: u64,
    ratio_percent: f64,
}

// ---------------------------------------------------------------------------
// Chunk-level evaluation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkLevelPrecision {
    query: String,
    chunk_precision_at_k: Vec<PrecisionAtK>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkLevelResults {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    grep: Vec<ChunkLevelPrecision>,
    ranked: Vec<ChunkLevelPrecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hybrid: Option<Vec<ChunkLevelPrecision>>,
}

// ---------------------------------------------------------------------------
// Regression detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RegressionStatus {
    Pass,
    Warn,
    Regression,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetricComparison {
    pub metric_name: String,
    pub baseline: f64,
    pub current: f64,
    pub change_percent: f64,
    pub status: RegressionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub struct RegressionReport {
    pub baseline_timestamp: String,
    pub comparisons: Vec<MetricComparison>,
    pub passed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BenchResults {
    timestamp: String,
    environment: EnvInfo,
    data_stats: DataStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    precision: Option<PrecisionResults>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latency: Option<LatencyResults>,
    #[serde(skip_serializing_if = "Option::is_none")]
    throughput: Option<ThroughputResults>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disk_usage: Option<DiskUsageResults>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunk_precision: Option<ChunkLevelResults>,
    #[serde(skip_serializing_if = "Option::is_none")]
    regression: Option<RegressionReport>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) fn percentile(sorted_data: &[u128], p: f64) -> u128 {
    if sorted_data.is_empty() {
        return 0;
    }
    let idx = ((p / 100.0) * (sorted_data.len() - 1) as f64).round() as usize;
    sorted_data[idx.min(sorted_data.len() - 1)]
}

pub(crate) fn collect_data_stats(docs_root: &Path) -> DataStats {
    let mut file_count: u64 = 0;
    let mut total_size_bytes: u64 = 0;
    for entry in WalkDir::new(docs_root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        // Skip hidden/system directories
        let rel = path
            .strip_prefix(docs_root)
            .unwrap_or(path)
            .to_string_lossy();
        let starts_hidden = Path::new(rel.as_ref())
            .components()
            .any(|c| is_reserved_dir_name(&c.as_os_str().to_string_lossy()));
        if starts_hidden {
            continue;
        }
        if crate::config::is_doc_file(path) {
            file_count += 1;
            total_size_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    DataStats {
        file_count,
        total_size_bytes,
    }
}

fn dir_size(path: &Path) -> u64 {
    WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

pub(crate) fn extract_retrieved_files(result: &serde_json::Value, mode: &str) -> Vec<String> {
    let matches = result["matches"].as_array();
    match matches {
        Some(arr) => arr
            .iter()
            .filter_map(|m| match mode {
                "ranked" | "hybrid" => {
                    let project = m.get("project").and_then(|v| v.as_str()).unwrap_or("");
                    let file = m.get("file").and_then(|v| v.as_str()).unwrap_or("");
                    if project.is_empty() {
                        Some(file.to_string())
                    } else {
                        Some(format!("{}/{}", project, file))
                    }
                }
                "grep" => {
                    // Grep results may or may not have project field
                    let project = m.get("project").and_then(|v| v.as_str());
                    let file = m.get("file").and_then(|v| v.as_str()).unwrap_or("");
                    match project {
                        Some(p) if !p.is_empty() => Some(format!("{}/{}", p, file)),
                        _ => Some(file.to_string()),
                    }
                }
                _ => None,
            })
            .collect(),
        None => Vec::new(),
    }
}

pub(crate) fn compute_precision_at_k(
    retrieved: &[String],
    relevant: &[String],
    k_values: &[usize],
) -> Vec<PrecisionAtK> {
    // Normalize relevant file paths for comparison
    let relevant_normalized: Vec<String> = relevant
        .iter()
        .map(|r| r.replace('\\', "/").to_lowercase())
        .collect();

    k_values
        .iter()
        .map(|&k| {
            let top_k: Vec<String> = retrieved
                .iter()
                .take(k)
                .map(|r| r.replace('\\', "/").to_lowercase())
                .collect();
            let hits = top_k
                .iter()
                .filter(|r| relevant_normalized.iter().any(|rel| *r == rel))
                .count() as f64;
            let precision = if k > 0 { hits / k as f64 } else { 0.0 };
            let recall = if relevant.is_empty() {
                1.0
            } else {
                hits / relevant.len() as f64
            };
            PrecisionAtK {
                k,
                precision,
                recall,
                ndcg: Some(compute_ndcg_at_k(retrieved, relevant, k)),
                map: Some(compute_map_at_k(retrieved, relevant, k)),
            }
        })
        .collect()
}

/// Normalized Discounted Cumulative Gain at K (binary relevance).
pub(crate) fn compute_ndcg_at_k(retrieved: &[String], relevant: &[String], k: usize) -> f64 {
    let relevant_normalized: Vec<String> = relevant
        .iter()
        .map(|r| r.replace('\\', "/").to_lowercase())
        .collect();

    // DCG@K: sum of rel_i / log2(i+1) for i=1..K
    let dcg: f64 = retrieved
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, doc)| {
            let is_relevant = relevant_normalized
                .iter()
                .any(|r| *r == doc.replace('\\', "/").to_lowercase());
            let rel: f64 = if is_relevant { 1.0 } else { 0.0 };
            rel / (i as f64 + 2.0).log2() // log2(rank+1), rank is 1-indexed
        })
        .sum();

    // IDCG@K: ideal ordering — all relevant docs at the top
    let n_relevant = relevant.len().min(k);
    if n_relevant == 0 {
        return 1.0; // vacuously perfect
    }
    let idcg: f64 = (0..n_relevant).map(|i| 1.0 / (i as f64 + 2.0).log2()).sum();

    if idcg == 0.0 { 0.0 } else { dcg / idcg }
}

/// Mean Average Precision at K.
pub(crate) fn compute_map_at_k(retrieved: &[String], relevant: &[String], k: usize) -> f64 {
    let relevant_normalized: Vec<String> = relevant
        .iter()
        .map(|r| r.replace('\\', "/").to_lowercase())
        .collect();

    if relevant.is_empty() {
        return 1.0;
    }

    let mut hits = 0usize;
    let mut sum_precision = 0.0;

    for (i, doc) in retrieved.iter().take(k).enumerate() {
        let is_relevant = relevant_normalized
            .iter()
            .any(|r| *r == doc.replace('\\', "/").to_lowercase());
        if is_relevant {
            hits += 1;
            sum_precision += hits as f64 / (i + 1) as f64;
        }
    }

    sum_precision / relevant.len().min(k) as f64
}

/// Mean Reciprocal Rank — 1/rank of the first relevant result.
pub(crate) fn compute_mrr(retrieved: &[String], relevant: &[String]) -> f64 {
    let relevant_normalized: Vec<String> = relevant
        .iter()
        .map(|r| r.replace('\\', "/").to_lowercase())
        .collect();

    if relevant.is_empty() {
        return 1.0;
    }

    for (i, doc) in retrieved.iter().enumerate() {
        if relevant_normalized
            .iter()
            .any(|r| *r == doc.replace('\\', "/").to_lowercase())
        {
            return 1.0 / (i + 1) as f64;
        }
    }
    0.0
}

// ---------------------------------------------------------------------------
// Chunk-level evaluation
// ---------------------------------------------------------------------------

struct RetrievedChunk {
    file: String,
    line_start: u64,
}

/// Extract chunk-level info (file + line_start) from ranked/hybrid search results.
fn extract_retrieved_chunks(result: &serde_json::Value, mode: &str) -> Vec<RetrievedChunk> {
    let matches = result["matches"].as_array();
    match matches {
        Some(arr) => arr
            .iter()
            .filter_map(|m| {
                let project = m.get("project").and_then(|v| v.as_str()).unwrap_or("");
                let file = m.get("file").and_then(|v| v.as_str()).unwrap_or("");
                let line_start = if mode == "grep" {
                    m.get("line").and_then(|v| v.as_u64())?
                } else {
                    m.get("line_start").and_then(|v| v.as_u64())?
                };
                let path = if !project.is_empty() {
                    format!("{}/{}", project, file)
                } else {
                    file.to_string()
                };
                Some(RetrievedChunk {
                    file: path.replace('\\', "/").to_lowercase(),
                    line_start,
                })
            })
            .collect(),
        None => Vec::new(),
    }
}

/// Resolve a heading string to a (line_start, line_end) range within a file.
/// Returns None if the file cannot be read or the heading is not found.
fn resolve_section_lines(docs_root: &Path, file: &str, heading: &str) -> Option<(usize, usize)> {
    let path = docs_root.join(file);
    let content = std::fs::read_to_string(&path).ok()?;
    let lines: Vec<&str> = content.lines().collect();

    // Find the heading line (case-insensitive match)
    let heading_lower = heading.to_lowercase();
    let heading_line = lines
        .iter()
        .position(|l| l.to_lowercase().starts_with(&heading_lower))?;

    // Find the next heading at the same or higher level (## or #)
    let heading_prefix: &str = lines[heading_line].split_whitespace().next().unwrap_or("");
    let prefix_len = heading_prefix.len();

    let mut end_line = lines.len();
    for (i, line) in lines.iter().enumerate().skip(heading_line + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#')
            && trimmed.len() > prefix_len.min(2)
            && trimmed.chars().take(prefix_len.max(2)).all(|c| c == '#')
        {
            end_line = i;
            break;
        }
    }

    // 1-based line numbers
    Some((heading_line + 1, end_line))
}

/// Compute chunk-level precision: does the retrieved chunk's line_start fall
/// within any of the relevant sections' line ranges?
fn compute_chunk_precision(
    chunks: &[RetrievedChunk],
    sections: &[RelevantSection],
    docs_root: &Path,
    k_values: &[usize],
) -> Vec<PrecisionAtK> {
    // Resolve each section to a line range
    let resolved: Vec<(String, usize, usize)> = sections
        .iter()
        .filter_map(|s| {
            let file_lower = s.file.replace('\\', "/").to_lowercase();
            resolve_section_lines(docs_root, &s.file, &s.heading)
                .map(|(start, end)| (file_lower, start, end))
        })
        .collect();

    if resolved.is_empty() {
        return k_values
            .iter()
            .map(|&k| PrecisionAtK {
                k,
                precision: 0.0,
                recall: 1.0, // vacuously perfect
                ndcg: None,
                map: None,
            })
            .collect();
    }

    k_values
        .iter()
        .map(|&k| {
            let top_k: Vec<&RetrievedChunk> = chunks.iter().take(k).collect();
            let hits = top_k
                .iter()
                .filter(|c| {
                    resolved.iter().any(|(file, start, end)| {
                        c.file == *file
                            && c.line_start as usize >= *start
                            && c.line_start as usize <= *end
                    })
                })
                .count() as f64;
            let precision = if k > 0 { hits / k as f64 } else { 0.0 };
            let recall = hits / resolved.len() as f64;
            PrecisionAtK {
                k,
                precision,
                recall,
                ndcg: None,
                map: None,
            }
        })
        .collect()
}

pub(crate) fn load_ground_truth(path: &Path) -> Result<Vec<GroundTruthEntry>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("Reading {}", path.display()))?;
    #[derive(Deserialize)]
    struct GroundTruth {
        #[serde(rename = "query")]
        queries: Vec<GroundTruthEntry>,
    }
    let gt: GroundTruth = toml::from_str(&content)
        .with_context(|| format!("Parsing ground truth TOML from {}", path.display()))?;
    Ok(gt.queries)
}

pub(crate) fn resolve_ground_truth_path(corpus: bool, explicit: Option<&Path>) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_path_buf();
    }
    if corpus {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        return manifest_dir
            .join("benches")
            .join("corpus")
            .join("ground_truth.toml");
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("benches")
        .join("ground_truth.toml")
}

// ---------------------------------------------------------------------------
// Regression detection functions
// ---------------------------------------------------------------------------

/// Thresholds for regression classification.
const PRECISION_REGRESSION_THRESHOLD: f64 = 5.0; // >5% precision drop → Regression
const LATENCY_REGRESSION_THRESHOLD: f64 = 20.0; // >20% latency increase → Regression
const THROUGHPUT_REGRESSION_THRESHOLD: f64 = 15.0; // >15% throughput drop → Regression
const WARN_HALF_FACTOR: f64 = 0.5; // within half the threshold → Warn

fn load_baseline(path: &Path) -> Result<BenchResults> {
    let data = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read baseline file: {}", path.display()))?;
    serde_json::from_str(&data).with_context(|| "Failed to parse baseline JSON".to_string())
}

fn save_baseline(path: &Path, results: &BenchResults) -> Result<()> {
    let json = serde_json::to_string_pretty(results)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, json)?;
    Ok(())
}

fn compare_with_baseline(current: &BenchResults, baseline: &BenchResults) -> RegressionReport {
    let mut comparisons = Vec::new();

    // Compare precision metrics
    if let (Some(cur_prec), Some(base_prec)) = (&current.precision, &baseline.precision) {
        // Aggregate per-K precision across all queries for ranked mode
        compare_precision_mode(
            &mut comparisons,
            "ranked",
            &base_prec.ranked,
            &cur_prec.ranked,
        );
    }

    // Compare latency (p50, p95)
    if let (Some(cur_lat), Some(base_lat)) = (&current.latency, &baseline.latency) {
        compare_latency_mode(&mut comparisons, "grep", &base_lat.grep, &cur_lat.grep);
        compare_latency_mode(
            &mut comparisons,
            "ranked",
            &base_lat.ranked,
            &cur_lat.ranked,
        );
    }

    // Compare throughput
    if let (Some(cur_tp), Some(base_tp)) = (&current.throughput, &baseline.throughput) {
        compare_metric(
            &mut comparisons,
            "throughput_full_rebuild",
            base_tp.full_rebuild_ms as f64,
            cur_tp.full_rebuild_ms as f64,
            THROUGHPUT_REGRESSION_THRESHOLD,
            false, // higher ms is worse
        );
        compare_metric(
            &mut comparisons,
            "throughput_incremental",
            base_tp.incremental_no_change_ms as f64,
            cur_tp.incremental_no_change_ms as f64,
            THROUGHPUT_REGRESSION_THRESHOLD,
            false,
        );
    }

    let passed = comparisons
        .iter()
        .all(|c| c.status != RegressionStatus::Regression);

    RegressionReport {
        baseline_timestamp: baseline.timestamp.clone(),
        comparisons,
        passed,
    }
}

/// Average precision across all queries for each K, then compare.
fn compare_precision_mode(
    comparisons: &mut Vec<MetricComparison>,
    mode: &str,
    base_queries: &[PerQueryPrecision],
    cur_queries: &[PerQueryPrecision],
) {
    if base_queries.is_empty() || cur_queries.is_empty() {
        return;
    }

    // Average precision@K across queries (use first K from each query)
    let base_avg = avg_first_k_precision(base_queries);
    let cur_avg = avg_first_k_precision(cur_queries);
    compare_metric(
        comparisons,
        &format!("avg_precision@K ({mode})"),
        base_avg,
        cur_avg,
        PRECISION_REGRESSION_THRESHOLD,
        true,
    );

    // Average NDCG
    let base_ndcg = avg_option_field(base_queries, |q| q.mrr);
    let cur_ndcg = avg_option_field(cur_queries, |q| q.mrr);
    if let (Some(b), Some(c)) = (base_ndcg, cur_ndcg) {
        compare_metric(
            comparisons,
            &format!("avg_mrr ({mode})"),
            b,
            c,
            PRECISION_REGRESSION_THRESHOLD,
            true,
        );
    }
}

fn avg_first_k_precision(queries: &[PerQueryPrecision]) -> f64 {
    let precisions: Vec<f64> = queries
        .iter()
        .filter_map(|q| q.precision_at_k.first().map(|p| p.precision))
        .collect();
    if precisions.is_empty() {
        0.0
    } else {
        precisions.iter().sum::<f64>() / precisions.len() as f64
    }
}

fn avg_option_field<F>(queries: &[PerQueryPrecision], extractor: F) -> Option<f64>
where
    F: Fn(&PerQueryPrecision) -> Option<f64>,
{
    let values: Vec<f64> = queries.iter().filter_map(extractor).collect();
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

fn compare_latency_mode(
    comparisons: &mut Vec<MetricComparison>,
    mode: &str,
    base: &[LatencyEntry],
    cur: &[LatencyEntry],
) {
    for (b, c) in base.iter().zip(cur.iter()) {
        compare_metric(
            comparisons,
            &format!("latency_{mode}_{}_p50", b.query),
            b.p50_us as f64,
            c.p50_us as f64,
            LATENCY_REGRESSION_THRESHOLD,
            false,
        );
        compare_metric(
            comparisons,
            &format!("latency_{mode}_{}_p95", b.query),
            b.p95_us as f64,
            c.p95_us as f64,
            LATENCY_REGRESSION_THRESHOLD,
            false,
        );
    }
}

/// Compare a single metric value against baseline.
/// `lower_is_worse`: true for precision/throughput (dropping is bad), false for latency (increasing is bad).
fn compare_metric(
    comparisons: &mut Vec<MetricComparison>,
    name: &str,
    baseline: f64,
    current: f64,
    threshold: f64,
    lower_is_worse: bool,
) {
    if baseline == 0.0 {
        return; // avoid division by zero
    }
    let change_pct = (current - baseline) / baseline * 100.0;

    let status = if lower_is_worse {
        // Negative change_pct = drop = bad
        if change_pct <= -threshold {
            RegressionStatus::Regression
        } else if change_pct <= -threshold * WARN_HALF_FACTOR {
            RegressionStatus::Warn
        } else {
            RegressionStatus::Pass
        }
    } else {
        // Positive change_pct = increase = bad (latency)
        if change_pct >= threshold {
            RegressionStatus::Regression
        } else if change_pct >= threshold * WARN_HALF_FACTOR {
            RegressionStatus::Warn
        } else {
            RegressionStatus::Pass
        }
    };

    comparisons.push(MetricComparison {
        metric_name: name.to_string(),
        baseline,
        current,
        change_percent: change_pct,
        status,
    });
}

fn resolve_docs_root(corpus: bool) -> Result<PathBuf> {
    if corpus {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        let corpus_dir = manifest_dir.join("benches").join("corpus");
        if !corpus_dir.is_dir() {
            anyhow::bail!(
                "Corpus directory not found: {}. Expected at benches/corpus/ in the alcove source.",
                corpus_dir.display()
            );
        }
        Ok(corpus_dir)
    } else {
        crate::setup::saved_docs_root()
            .ok_or_else(|| anyhow::anyhow!("No docs repository found. Run `alcove setup` first."))
    }
}

/// Expand tilde in cache_dir and create an EmbeddingService from config.
#[cfg(feature = "embed")]
fn create_embedding_service(
    emb_cfg: &crate::config::EmbeddingConfig,
) -> crate::embedding::EmbeddingService {
    let model = crate::embedding::resolve_model(&emb_cfg.model);
    let cache_dir = if emb_cfg.cache_dir.starts_with('~') {
        std::env::var("HOME")
            .ok()
            .map(|h| emb_cfg.cache_dir.replacen('~', &h, 1))
            .unwrap_or_else(|| emb_cfg.cache_dir.clone())
    } else {
        emb_cfg.cache_dir.clone()
    };
    crate::embedding::EmbeddingService::new(crate::config::EmbeddingConfig {
        model: model.as_str().to_string(),
        auto_download: emb_cfg.auto_download,
        cache_dir,
        enabled: true,
        query_cache_size: emb_cfg.query_cache_size,
    })
}

// ---------------------------------------------------------------------------
// Benchmark runners
// ---------------------------------------------------------------------------

fn run_precision_benchmark(
    docs_root: &Path,
    queries: &[GroundTruthEntry],
    _scope: &str,
) -> Result<(PrecisionResults, Option<ChunkLevelResults>)> {
    let k_values = [5, 10, 20];
    let limit = 20;

    let mut grep_results: Vec<PerQueryPrecision> = Vec::new();
    let mut ranked_results: Vec<PerQueryPrecision> = Vec::new();

    // Chunk-level evaluation accumulators
    let mut chunk_ranked: Vec<ChunkLevelPrecision> = Vec::new();
    #[cfg(feature = "embed")]
    let mut chunk_hybrid: Vec<ChunkLevelPrecision> = Vec::new();

    // Ensure index exists for ranked search
    if !crate::index::index_exists(docs_root) {
        eprintln!(
            "  {} Building search index for ranked benchmarks...",
            style("...").dim()
        );
        crate::index::build_index(docs_root)?;
    }

    for entry in queries {
        // Grep search
        let grep_result = crate::tools::tool_search_global(
            docs_root,
            json!({
                "query": entry.text,
                "scope": "global",
                "limit": limit,
                "mode": "grep",
            }),
        )?;
        let grep_files = extract_retrieved_files(&grep_result, "grep");
        let grep_pak = compute_precision_at_k(&grep_files, &entry.relevant_files, &k_values);
        let grep_mrr = compute_mrr(&grep_files, &entry.relevant_files);
        grep_results.push(PerQueryPrecision {
            query: entry.text.clone(),
            precision_at_k: grep_pak,
            mrr: Some(grep_mrr),
            category: entry.category.clone(),
            difficulty: entry.difficulty.clone(),
        });

        // Ranked search
        let ranked_result = crate::index::search_indexed(docs_root, &entry.text, limit, None)?;
        let ranked_files = extract_retrieved_files(&ranked_result, "ranked");
        let ranked_pak = compute_precision_at_k(&ranked_files, &entry.relevant_files, &k_values);
        let ranked_mrr = compute_mrr(&ranked_files, &entry.relevant_files);
        ranked_results.push(PerQueryPrecision {
            query: entry.text.clone(),
            precision_at_k: ranked_pak,
            mrr: Some(ranked_mrr),
            category: entry.category.clone(),
            difficulty: entry.difficulty.clone(),
        });

        // Chunk-level evaluation (only for entries with relevant_sections)
        if let Some(ref sections) = entry.relevant_sections
            && !sections.is_empty()
        {
            let chunks = extract_retrieved_chunks(&ranked_result, "ranked");
            let cpa = compute_chunk_precision(&chunks, sections, docs_root, &k_values);
            chunk_ranked.push(ChunkLevelPrecision {
                query: entry.text.clone(),
                chunk_precision_at_k: cpa,
            });
        }
    }

    // Hybrid search (only if embed feature is enabled)
    #[cfg(feature = "embed")]
    let hybrid_results = {
        let mut results: Vec<PerQueryPrecision> = Vec::new();
        let cfg = crate::config::load_config();
        let emb_cfg = cfg.embedding_config_with_defaults();
        if emb_cfg.enabled {
            let service = create_embedding_service(&emb_cfg);

            for entry in queries {
                match crate::index::search_hybrid(docs_root, &entry.text, &service, limit, None) {
                    Ok(hybrid_result) => {
                        let hybrid_files = extract_retrieved_files(&hybrid_result, "hybrid");
                        let hybrid_pak =
                            compute_precision_at_k(&hybrid_files, &entry.relevant_files, &k_values);
                        let hybrid_mrr = compute_mrr(&hybrid_files, &entry.relevant_files);
                        results.push(PerQueryPrecision {
                            query: entry.text.clone(),
                            precision_at_k: hybrid_pak,
                            mrr: Some(hybrid_mrr),
                            category: entry.category.clone(),
                            difficulty: entry.difficulty.clone(),
                        });

                        // Chunk-level evaluation for hybrid
                        if let Some(ref sections) = entry.relevant_sections
                            && !sections.is_empty()
                        {
                            let chunks = extract_retrieved_chunks(&hybrid_result, "hybrid");
                            let cpa =
                                compute_chunk_precision(&chunks, sections, docs_root, &k_values);
                            chunk_hybrid.push(ChunkLevelPrecision {
                                query: entry.text.clone(),
                                chunk_precision_at_k: cpa,
                            });
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "  {} Hybrid search failed for '{}': {}",
                            style("!").yellow(),
                            entry.text,
                            e
                        );
                    }
                }
            }
        }
        if results.is_empty() {
            None
        } else {
            Some(results)
        }
    };

    #[cfg(not(feature = "embed"))]
    let hybrid_results: Option<Vec<PerQueryPrecision>> = None;

    let chunk_results = if chunk_ranked.is_empty() {
        None
    } else {
        Some(ChunkLevelResults {
            grep: Vec::new(),
            ranked: chunk_ranked,
            #[cfg(feature = "embed")]
            hybrid: if chunk_hybrid.is_empty() {
                None
            } else {
                Some(chunk_hybrid)
            },
            #[cfg(not(feature = "embed"))]
            hybrid: None,
        })
    };

    Ok((
        PrecisionResults {
            grep: grep_results,
            ranked: ranked_results,
            hybrid: hybrid_results,
        },
        chunk_results,
    ))
}

fn run_latency_benchmark(
    docs_root: &Path,
    queries: &[GroundTruthEntry],
    _scope: &str,
    iterations: usize,
) -> Result<LatencyResults> {
    let limit = 20;
    let mut grep_entries: Vec<LatencyEntry> = Vec::new();
    let mut ranked_entries: Vec<LatencyEntry> = Vec::new();

    // Ensure index exists for ranked search
    if !crate::index::index_exists(docs_root) {
        eprintln!(
            "  {} Building search index for latency benchmarks...",
            style("...").dim()
        );
        crate::index::build_index(docs_root)?;
    }

    for entry in queries {
        // Warm-up: one run each
        let _ = crate::tools::tool_search_global(
            docs_root,
            json!({
                "query": entry.text,
                "scope": "global",
                "limit": limit,
                "mode": "grep",
            }),
        );
        let _ = crate::index::search_indexed(docs_root, &entry.text, limit, None);

        // Grep latency
        let mut grep_times: Vec<u128> = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let start = Instant::now();
            let _ = crate::tools::tool_search_global(
                docs_root,
                json!({
                    "query": entry.text,
                    "scope": "global",
                    "limit": limit,
                    "mode": "grep",
                }),
            )?;
            grep_times.push(start.elapsed().as_micros());
        }
        grep_times.sort();
        grep_entries.push(LatencyEntry {
            query: entry.text.clone(),
            avg_us: grep_times.iter().sum::<u128>() / grep_times.len() as u128,
            p50_us: percentile(&grep_times, 50.0),
            p95_us: percentile(&grep_times, 95.0),
            p99_us: percentile(&grep_times, 99.0),
        });

        // Ranked latency
        let mut ranked_times: Vec<u128> = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let start = Instant::now();
            let _ = crate::index::search_indexed(docs_root, &entry.text, limit, None);
            ranked_times.push(start.elapsed().as_micros());
        }
        ranked_times.sort();
        ranked_entries.push(LatencyEntry {
            query: entry.text.clone(),
            avg_us: ranked_times.iter().sum::<u128>() / ranked_times.len() as u128,
            p50_us: percentile(&ranked_times, 50.0),
            p95_us: percentile(&ranked_times, 95.0),
            p99_us: percentile(&ranked_times, 99.0),
        });
    }

    // Hybrid latency (only if embed feature is enabled)
    #[cfg(feature = "embed")]
    let hybrid_entries = {
        let mut entries: Vec<LatencyEntry> = Vec::new();
        let cfg = crate::config::load_config();
        let emb_cfg = cfg.embedding_config_with_defaults();
        if emb_cfg.enabled {
            let service = create_embedding_service(&emb_cfg);

            for entry in queries {
                // Warm-up
                let _ = crate::index::search_hybrid(docs_root, &entry.text, &service, limit, None);

                let mut times: Vec<u128> = Vec::with_capacity(iterations);
                for _ in 0..iterations {
                    let start = Instant::now();
                    let _ =
                        crate::index::search_hybrid(docs_root, &entry.text, &service, limit, None);
                    times.push(start.elapsed().as_micros());
                }
                times.sort();
                entries.push(LatencyEntry {
                    query: entry.text.clone(),
                    avg_us: times.iter().sum::<u128>() / times.len() as u128,
                    p50_us: percentile(&times, 50.0),
                    p95_us: percentile(&times, 95.0),
                    p99_us: percentile(&times, 99.0),
                });
            }
        }
        if entries.is_empty() {
            None
        } else {
            Some(entries)
        }
    };

    #[cfg(not(feature = "embed"))]
    let hybrid_entries: Option<Vec<LatencyEntry>> = None;

    Ok(LatencyResults {
        grep: grep_entries,
        ranked: ranked_entries,
        hybrid: hybrid_entries,
    })
}

fn run_throughput_benchmark(docs_root: &Path) -> Result<ThroughputResults> {
    // Full rebuild
    eprintln!("  {} Measuring full rebuild...", style("...").dim());
    let start = Instant::now();
    crate::index::rebuild_index(docs_root)?;
    let full_rebuild_ms = start.elapsed().as_millis();

    // Incremental (no change)
    eprintln!(
        "  {} Measuring incremental build (no changes)...",
        style("...").dim()
    );
    let start = Instant::now();
    crate::index::build_index(docs_root)?;
    let incremental_no_change_ms = start.elapsed().as_millis();

    // Stale detection
    eprintln!("  {} Measuring stale detection...", style("...").dim());
    let start = Instant::now();
    let _ = crate::index::is_index_stale(docs_root);
    let stale_detection_us = start.elapsed().as_micros();

    Ok(ThroughputResults {
        full_rebuild_ms,
        incremental_no_change_ms,
        stale_detection_us,
    })
}

fn run_disk_usage_benchmark(docs_root: &Path) -> Result<DiskUsageResults> {
    let index_dir = docs_root.join(".alcove").join("index");
    let index_bytes = if index_dir.exists() {
        dir_size(&index_dir)
    } else {
        0
    };
    let docs_bytes = collect_data_stats(docs_root).total_size_bytes;
    let ratio_percent = if docs_bytes > 0 {
        (index_bytes as f64 / docs_bytes as f64) * 100.0
    } else {
        0.0
    };
    Ok(DiskUsageResults {
        index_bytes,
        docs_bytes,
        ratio_percent,
    })
}

// ---------------------------------------------------------------------------
// Output formatters
// ---------------------------------------------------------------------------

pub(crate) fn format_human(results: &BenchResults) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "\n{} {}\n",
        style("Alcove Benchmark Results").bold(),
        style(&results.timestamp).dim()
    ));
    out.push_str(&format!(
        "  OS: {}  |  Rust: {}  |  Alcove: {}\n",
        results.environment.os,
        results.environment.rust_version,
        results.environment.alcove_version,
    ));
    out.push_str(&format!(
        "  Docs: {} files, {} bytes\n",
        results.data_stats.file_count, results.data_stats.total_size_bytes,
    ));

    if let Some(ref precision) = results.precision {
        out.push_str(&format!(
            "\n{}\n",
            style("Search Quality (Precision@K / Recall@K)").bold()
        ));
        out.push_str(&format!("{}\n", "-".repeat(70)));

        for (mode, queries) in [("Grep", &precision.grep), ("Ranked", &precision.ranked)] {
            out.push_str(&format!("\n  {}:\n", style(mode).cyan()));
            for q in queries {
                out.push_str(&format!("    \"{}\"\n", q.query));
                for pak in &q.precision_at_k {
                    out.push_str(&format!(
                        "      P@{}: {:.3}  R@{}: {:.3}",
                        pak.k, pak.precision, pak.k, pak.recall
                    ));
                    if let Some(ndcg) = pak.ndcg {
                        out.push_str(&format!("  NDCG: {:.3}", ndcg));
                    }
                    if let Some(map) = pak.map {
                        out.push_str(&format!("  MAP: {:.3}", map));
                    }
                    out.push('\n');
                }
                if let Some(mrr) = q.mrr {
                    out.push_str(&format!("      MRR: {:.3}\n", mrr));
                }
            }
        }
        if let Some(ref hybrid) = precision.hybrid {
            out.push_str(&format!("\n  {}:\n", style("Hybrid").cyan()));
            for q in hybrid {
                out.push_str(&format!("    \"{}\"\n", q.query));
                for pak in &q.precision_at_k {
                    out.push_str(&format!(
                        "      P@{}: {:.3}  R@{}: {:.3}",
                        pak.k, pak.precision, pak.k, pak.recall
                    ));
                    if let Some(ndcg) = pak.ndcg {
                        out.push_str(&format!("  NDCG: {:.3}", ndcg));
                    }
                    if let Some(map) = pak.map {
                        out.push_str(&format!("  MAP: {:.3}", map));
                    }
                    out.push('\n');
                }
                if let Some(mrr) = q.mrr {
                    out.push_str(&format!("      MRR: {:.3}\n", mrr));
                }
            }
        }
    }

    if let Some(ref latency) = results.latency {
        out.push_str(&format!("\n{}\n", style("Search Latency").bold()));
        out.push_str(&format!("{}\n", "-".repeat(70)));

        for (mode, entries) in [("Grep", &latency.grep), ("Ranked", &latency.ranked)] {
            out.push_str(&format!("\n  {}:\n", style(mode).cyan()));
            for e in entries {
                out.push_str(&format!(
                    "    \"{}\"  avg={}us  p50={}us  p95={}us  p99={}us\n",
                    e.query, e.avg_us, e.p50_us, e.p95_us, e.p99_us
                ));
            }
        }
        if let Some(ref hybrid) = latency.hybrid {
            out.push_str(&format!("\n  {}:\n", style("Hybrid").cyan()));
            for e in hybrid {
                out.push_str(&format!(
                    "    \"{}\"  avg={}us  p50={}us  p95={}us  p99={}us\n",
                    e.query, e.avg_us, e.p50_us, e.p95_us, e.p99_us
                ));
            }
        }
    }

    if let Some(ref throughput) = results.throughput {
        out.push_str(&format!("\n{}\n", style("Index Throughput").bold()));
        out.push_str(&format!("{}\n", "-".repeat(70)));
        out.push_str(&format!(
            "  Full rebuild:       {} ms\n",
            throughput.full_rebuild_ms
        ));
        out.push_str(&format!(
            "  Incremental (noop): {} ms\n",
            throughput.incremental_no_change_ms
        ));
        out.push_str(&format!(
            "  Stale detection:    {} us\n",
            throughput.stale_detection_us
        ));
    }

    if let Some(ref disk) = results.disk_usage {
        out.push_str(&format!("\n{}\n", style("Disk Usage").bold()));
        out.push_str(&format!("{}\n", "-".repeat(70)));
        out.push_str(&format!("  Index:  {} bytes\n", disk.index_bytes));
        out.push_str(&format!("  Docs:   {} bytes\n", disk.docs_bytes));
        out.push_str(&format!("  Ratio:  {:.1}%\n", disk.ratio_percent));
    }

    if let Some(ref chunk) = results.chunk_precision {
        out.push_str(&format!("\n{}\n", style("Chunk-Level Accuracy").bold()));
        out.push_str(&format!("{}\n", "-".repeat(70)));

        for (mode, queries) in [("Ranked", &chunk.ranked)] {
            out.push_str(&format!("\n  {}:\n", style(mode).cyan()));
            for q in queries {
                out.push_str(&format!("    \"{}\"\n", q.query));
                for pak in &q.chunk_precision_at_k {
                    out.push_str(&format!(
                        "      Chunk-P@{}: {:.3}  Chunk-R@{}: {:.3}\n",
                        pak.k, pak.precision, pak.k, pak.recall
                    ));
                }
            }
        }
        if let Some(ref hybrid) = chunk.hybrid {
            out.push_str(&format!("\n  {}:\n", style("Hybrid").cyan()));
            for q in hybrid {
                out.push_str(&format!("    \"{}\"\n", q.query));
                for pak in &q.chunk_precision_at_k {
                    out.push_str(&format!(
                        "      Chunk-P@{}: {:.3}  Chunk-R@{}: {:.3}\n",
                        pak.k, pak.precision, pak.k, pak.recall
                    ));
                }
            }
        }
    }

    if let Some(ref regression) = results.regression {
        out.push_str(&format!(
            "\n{} (vs {})\n",
            style("Regression Analysis").bold(),
            regression.baseline_timestamp
        ));
        out.push_str(&format!("{}\n", "-".repeat(70)));

        let overall = if regression.passed {
            style("PASS — No regressions detected").green().to_string()
        } else {
            style("FAIL — Regressions detected").red().to_string()
        };
        out.push_str(&format!("  Overall: {}\n\n", overall));

        for c in &regression.comparisons {
            let icon = match c.status {
                RegressionStatus::Pass => style("✓").green().to_string(),
                RegressionStatus::Warn => style("⚠").yellow().to_string(),
                RegressionStatus::Regression => style("✗").red().to_string(),
            };
            let change = if c.change_percent >= 0.0 {
                format!("+{:.1}%", c.change_percent)
            } else {
                format!("{:.1}%", c.change_percent)
            };
            out.push_str(&format!(
                "  {} {:40} baseline={:.3} current={:.3} ({})\n",
                icon, c.metric_name, c.baseline, c.current, change
            ));
        }
    }

    out
}

pub(crate) fn format_markdown(results: &BenchResults) -> String {
    let mut md = String::new();

    md.push_str(&format!(
        "# Alcove Benchmark Report\n\n**Date:** {}\n\n",
        results.timestamp
    ));
    md.push_str(&format!(
        "**Environment:** {} | Rust {} | Alcove {}\n\n",
        results.environment.os,
        results.environment.rust_version,
        results.environment.alcove_version,
    ));
    md.push_str(&format!(
        "**Data:** {} files, {} bytes\n\n",
        results.data_stats.file_count, results.data_stats.total_size_bytes,
    ));

    // Summary table: grep vs ranked comparison
    if let Some(ref precision) = results.precision {
        md.push_str("## 1. Summary\n\n");
        md.push_str("| Mode | Avg P@5 | Avg P@10 | Avg P@20 |\n");
        md.push_str("|------|---------|----------|----------|\n");

        for (label, queries) in [("grep", &precision.grep), ("ranked", &precision.ranked)] {
            let avg_p5 = average_precision_at_k(queries, 5);
            let avg_p10 = average_precision_at_k(queries, 10);
            let avg_p20 = average_precision_at_k(queries, 20);
            md.push_str(&format!(
                "| {} | {:.3} | {:.3} | {:.3} |\n",
                label, avg_p5, avg_p10, avg_p20
            ));
        }
        if let Some(ref hybrid) = precision.hybrid {
            let avg_p5 = average_precision_at_k(hybrid, 5);
            let avg_p10 = average_precision_at_k(hybrid, 10);
            let avg_p20 = average_precision_at_k(hybrid, 20);
            md.push_str(&format!(
                "| hybrid | {:.3} | {:.3} | {:.3} |\n",
                avg_p5, avg_p10, avg_p20
            ));
        }
        md.push('\n');
    }

    // Search speed table
    if let Some(ref latency) = results.latency {
        md.push_str("## 2. Search Speed\n\n");
        md.push_str("| Query | Mode | Avg (us) | P50 (us) | P95 (us) | P99 (us) |\n");
        md.push_str("|-------|------|----------|----------|----------|----------|\n");

        for (mode_label, entries) in [("grep", &latency.grep), ("ranked", &latency.ranked)] {
            for e in entries {
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} |\n",
                    e.query, mode_label, e.avg_us, e.p50_us, e.p95_us, e.p99_us
                ));
            }
        }
        if let Some(ref hybrid) = latency.hybrid {
            for e in hybrid {
                md.push_str(&format!(
                    "| {} | hybrid | {} | {} | {} | {} |\n",
                    e.query, e.avg_us, e.p50_us, e.p95_us, e.p99_us
                ));
            }
        }
        md.push('\n');
    }

    // Index build performance
    if let Some(ref throughput) = results.throughput {
        md.push_str("## 3. Index Build Performance\n\n");
        md.push_str("| Operation | Time |\n");
        md.push_str("|-----------|------|\n");
        md.push_str(&format!(
            "| Full rebuild | {} ms |\n",
            throughput.full_rebuild_ms
        ));
        md.push_str(&format!(
            "| Incremental (no change) | {} ms |\n",
            throughput.incremental_no_change_ms
        ));
        md.push_str(&format!(
            "| Stale detection | {} us |\n",
            throughput.stale_detection_us
        ));
        md.push('\n');
    }

    // Search quality tables
    if let Some(ref precision) = results.precision {
        md.push_str("## 4. Search Quality\n\n");
        for (label, queries) in [("Grep", &precision.grep), ("Ranked", &precision.ranked)] {
            md.push_str(&format!("### {}\n\n", label));
            md.push_str("| Query | P@5 | R@5 | NDCG@5 | MAP@5 | MRR |\n");
            md.push_str("|-------|-----|-----|--------|-------|-----|\n");
            for q in queries {
                let p5 = q.precision_at_k.first();
                let p_str = p5.map_or("-".into(), |p| format!("{:.3}", p.precision));
                let r_str = p5.map_or("-".into(), |p| format!("{:.3}", p.recall));
                let ndcg_str = p5
                    .and_then(|p| p.ndcg)
                    .map_or("-".into(), |v| format!("{:.3}", v));
                let map_str = p5
                    .and_then(|p| p.map)
                    .map_or("-".into(), |v| format!("{:.3}", v));
                let mrr_str = q.mrr.map_or("-".into(), |v| format!("{:.3}", v));
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} |\n",
                    q.query, p_str, r_str, ndcg_str, map_str, mrr_str
                ));
            }
            md.push('\n');
        }
        if let Some(ref hybrid) = precision.hybrid {
            md.push_str("### Hybrid\n\n");
            md.push_str("| Query | P@5 | R@5 | NDCG@5 | MAP@5 | MRR |\n");
            md.push_str("|-------|-----|-----|--------|-------|-----|\n");
            for q in hybrid {
                let p5 = q.precision_at_k.first();
                let p_str = p5.map_or("-".into(), |p| format!("{:.3}", p.precision));
                let r_str = p5.map_or("-".into(), |p| format!("{:.3}", p.recall));
                let ndcg_str = p5
                    .and_then(|p| p.ndcg)
                    .map_or("-".into(), |v| format!("{:.3}", v));
                let map_str = p5
                    .and_then(|p| p.map)
                    .map_or("-".into(), |v| format!("{:.3}", v));
                let mrr_str = q.mrr.map_or("-".into(), |v| format!("{:.3}", v));
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {} | {} |\n",
                    q.query, p_str, r_str, ndcg_str, map_str, mrr_str
                ));
            }
            md.push('\n');
        }
    }

    // Disk usage
    if let Some(ref disk) = results.disk_usage {
        md.push_str("## 5. Disk Usage\n\n");
        md.push_str(&format!("- **Index:** {} bytes\n", disk.index_bytes));
        md.push_str(&format!("- **Docs:** {} bytes\n", disk.docs_bytes));
        md.push_str(&format!("- **Ratio:** {:.1}%\n", disk.ratio_percent));
        md.push('\n');
    }

    // Chunk-level accuracy
    if let Some(ref chunk) = results.chunk_precision {
        md.push_str("## 6. Chunk-Level Accuracy\n\n");
        md.push_str("| Query | Mode | Chunk-P@5 | Chunk-R@5 | Chunk-P@10 | Chunk-R@10 |\n");
        md.push_str("|-------|------|-----------|-----------|------------|------------|\n");
        for q in &chunk.ranked {
            let vals: Vec<String> = q
                .chunk_precision_at_k
                .iter()
                .flat_map(|pak| {
                    vec![
                        format!("{:.3}", pak.precision),
                        format!("{:.3}", pak.recall),
                    ]
                })
                .collect();
            md.push_str(&format!(
                "| {} | ranked | {} |\n",
                q.query,
                vals.join(" | ")
            ));
        }
        if let Some(ref hybrid) = chunk.hybrid {
            for q in hybrid {
                let vals: Vec<String> = q
                    .chunk_precision_at_k
                    .iter()
                    .flat_map(|pak| {
                        vec![
                            format!("{:.3}", pak.precision),
                            format!("{:.3}", pak.recall),
                        ]
                    })
                    .collect();
                md.push_str(&format!(
                    "| {} | hybrid | {} |\n",
                    q.query,
                    vals.join(" | ")
                ));
            }
        }
        md.push('\n');
    }

    // Regression analysis
    if let Some(ref regression) = results.regression {
        md.push_str(&format!(
            "## 7. Regression Analysis (vs {})\n\n",
            regression.baseline_timestamp
        ));
        let overall = if regression.passed {
            "**PASS** — No regressions detected"
        } else {
            "**FAIL** — Regressions detected"
        };
        md.push_str(&format!("Overall: {}\n\n", overall));
        md.push_str("| Metric | Baseline | Current | Change | Status |\n");
        md.push_str("|--------|----------|---------|--------|--------|\n");
        for c in &regression.comparisons {
            let change = if c.change_percent >= 0.0 {
                format!("+{:.1}%", c.change_percent)
            } else {
                format!("{:.1}%", c.change_percent)
            };
            let status = match c.status {
                RegressionStatus::Pass => "✓ pass",
                RegressionStatus::Warn => "⚠ warn",
                RegressionStatus::Regression => "✗ regression",
            };
            md.push_str(&format!(
                "| {} | {:.3} | {:.3} | {} | {} |\n",
                c.metric_name, c.baseline, c.current, change, status
            ));
        }
        md.push('\n');
    }

    // Conclusion
    md.push_str("## 8. Conclusion\n\n");
    if let (Some(latency), Some(precision)) = (&results.latency, &results.precision) {
        let avg_grep = latency
            .grep
            .iter()
            .map(|e| e.avg_us)
            .sum::<u128>()
            .checked_div(latency.grep.len() as u128)
            .unwrap_or(0);
        let avg_ranked = latency
            .ranked
            .iter()
            .map(|e| e.avg_us)
            .sum::<u128>()
            .checked_div(latency.ranked.len() as u128)
            .unwrap_or(0);
        let avg_grep_p5 = average_precision_at_k(&precision.grep, 5);
        let avg_ranked_p5 = average_precision_at_k(&precision.ranked, 5);
        let speed_ratio = if avg_grep > 0 && avg_ranked > 0 {
            avg_grep as f64 / avg_ranked as f64
        } else {
            0.0
        };
        md.push_str(&format!(
            "- Ranked search is {:.1}x {} than grep (avg {}us vs {}us)\n",
            if speed_ratio >= 1.0 {
                speed_ratio
            } else {
                1.0 / speed_ratio
            },
            if avg_ranked < avg_grep {
                "faster"
            } else {
                "slower"
            },
            avg_ranked,
            avg_grep,
        ));
        md.push_str(&format!(
            "- Ranked P@5: {:.3} vs Grep P@5: {:.3}\n",
            avg_ranked_p5, avg_grep_p5
        ));
    }
    if let Some(ref throughput) = results.throughput {
        md.push_str(&format!(
            "- Full index rebuild: {}ms, incremental (no change): {}ms\n",
            throughput.full_rebuild_ms, throughput.incremental_no_change_ms
        ));
    }

    md
}

pub(crate) fn average_precision_at_k(queries: &[PerQueryPrecision], k: usize) -> f64 {
    if queries.is_empty() {
        return 0.0;
    }
    let sum: f64 = queries
        .iter()
        .filter_map(|q| q.precision_at_k.iter().find(|pak| pak.k == k))
        .map(|pak| pak.precision)
        .sum();
    let count = queries.len() as f64;
    sum / count
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn cmd_bench(
    metrics: &str,
    scope: &str,
    output: &str,
    queries: Option<&Path>,
    output_file: Option<&Path>,
    baseline: Option<&Path>,
    save_baseline_path: Option<&Path>,
    corpus: bool,
) -> Result<()> {
    let docs_root = resolve_docs_root(corpus)?;

    let queries_path = resolve_ground_truth_path(corpus, queries);

    if !queries_path.exists() {
        anyhow::bail!(
            "Ground truth file not found: {}. Create it or specify --queries <path>",
            queries_path.display()
        );
    }

    let ground_truth = load_ground_truth(&queries_path)?;
    if ground_truth.is_empty() {
        anyhow::bail!(
            "Ground truth file contains no queries: {}",
            queries_path.display()
        );
    }

    eprintln!(
        "  {} Loaded {} benchmark queries from {}",
        style("...").dim(),
        ground_truth.len(),
        queries_path.display()
    );

    let config = BenchConfig {
        metrics: metrics.to_string(),
        scope: scope.to_string(),
        output: output.to_string(),
        queries_path: Some(queries_path),
        iterations: 50,
    };

    let run_precision = config.metrics == "all" || config.metrics == "precision";
    let run_latency = config.metrics == "all" || config.metrics == "latency";
    let run_throughput = config.metrics == "all" || config.metrics == "throughput";

    let data_stats = collect_data_stats(&docs_root);

    let (precision, chunk_precision) = if run_precision {
        eprintln!("  {} Running precision benchmarks...", style("...").dim());
        let (prec, chunk) = run_precision_benchmark(&docs_root, &ground_truth, &config.scope)?;
        (Some(prec), chunk)
    } else {
        (None, None)
    };

    let latency = if run_latency {
        eprintln!(
            "  {} Running latency benchmarks ({} iterations per query)...",
            style("...").dim(),
            config.iterations
        );
        Some(run_latency_benchmark(
            &docs_root,
            &ground_truth,
            &config.scope,
            config.iterations,
        )?)
    } else {
        None
    };

    let throughput = if run_throughput {
        Some(run_throughput_benchmark(&docs_root)?)
    } else {
        None
    };

    let disk_usage = if run_throughput || config.metrics == "all" {
        Some(run_disk_usage_benchmark(&docs_root)?)
    } else {
        None
    };

    let mut results = BenchResults {
        timestamp: now_rfc3339(),
        environment: EnvInfo {
            os: std::env::consts::OS.to_string(),
            rust_version: rustc_version(),
            alcove_version: env!("CARGO_PKG_VERSION").to_string(),
        },
        data_stats,
        precision,
        latency,
        throughput,
        disk_usage,
        chunk_precision,
        regression: None,
    };

    // Regression detection: compare against baseline if provided
    if let Some(baseline_path) = baseline {
        match load_baseline(baseline_path) {
            Ok(baseline_results) => {
                let report = compare_with_baseline(&results, &baseline_results);
                eprintln!(
                    "  {} Compared against baseline ({})",
                    style("...").dim(),
                    baseline_path.display()
                );
                results.regression = Some(report);
            }
            Err(e) => {
                eprintln!("  {} Failed to load baseline: {e}", style("✗").red());
            }
        }
    }

    // Save baseline if requested
    if let Some(save_path) = save_baseline_path {
        match save_baseline(save_path, &results) {
            Ok(()) => {
                eprintln!(
                    "  {} Baseline saved to {}",
                    style("✓").green(),
                    save_path.display()
                );
            }
            Err(e) => {
                eprintln!("  {} Failed to save baseline: {e}", style("✗").red());
            }
        }
    }

    let output_content = match config.output.as_str() {
        "json" => serde_json::to_string_pretty(&results)?,
        "markdown" => format_markdown(&results),
        _ => format_human(&results),
    };

    if let Some(path) = output_file {
        let file_content = if output == "human" {
            // Auto-detect format from file extension
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            match ext {
                "json" => serde_json::to_string_pretty(&results)?,
                "md" | "markdown" => format_markdown(&results),
                _ => output_content.clone(),
            }
        } else {
            // Already formatted, write directly
            output_content.clone()
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &file_content)?;
        eprintln!(
            "  {} Results saved to {}",
            style("✓").green(),
            path.display()
        );
    }

    println!("{output_content}");

    Ok(())
}

fn now_rfc3339() -> String {
    // Use std::time for a simple ISO-8601-ish timestamp without extra deps
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Format as YYYY-MM-DDTHH:MM:SSZ (UTC approximation)
    let days_since_epoch = secs / 86400;
    let time_of_day_secs = secs % 86400;
    let hours = time_of_day_secs / 3600;
    let minutes = (time_of_day_secs % 3600) / 60;
    let seconds = time_of_day_secs % 60;

    // Compute year/month/day from days since epoch
    let (year, month, day) = civil_from_days(days_since_epoch as i64);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

/// Convert days since Unix epoch to (year, month, day) using the civil date algorithm.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    // Howard Hinnant's algorithm
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

fn rustc_version() -> String {
    // Best-effort: try reading the rustc version at compile time.
    // If unavailable, report unknown.
    option_env!("RUSTC_VERSION")
        .unwrap_or("unknown")
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_fixture_docs(dir: &std::path::Path) {
        let proj = dir.join("test-project");
        std::fs::create_dir_all(&proj).unwrap();
        let mut f = std::fs::File::create(proj.join("ARCHITECTURE.md")).unwrap();
        write!(
            f,
            "# Architecture\n\nThis document describes the system architecture.\n\
             The MCP server uses stdio JSON-RPC 2.0 protocol.\n\
             Search is powered by tantivy BM25 ranking.\n"
        )
        .unwrap();
        let mut f = std::fs::File::create(proj.join("PRD.md")).unwrap();
        write!(
            f,
            "# Product Requirements\n\nAlcove is an MCP server for private docs.\n\
             Setup is done via `alcove setup` interactive command.\n\
             Supports 8 agents including Claude Code and Cursor.\n"
        )
        .unwrap();
    }

    fn make_ground_truth(path: &std::path::Path) {
        let mut f = std::fs::File::create(path).unwrap();
        write!(
            f,
            r#"[[query]]
text = "architecture"
relevant_files = ["test-project/ARCHITECTURE.md"]

[[query]]
text = "MCP server"
relevant_files = ["test-project/PRD.md", "test-project/ARCHITECTURE.md"]

[[query]]
text = "nonexistent-xyz"
relevant_files = []
"#
        )
        .unwrap();
    }

    // --- Pure logic tests ---

    #[test]
    fn test_percentile_basic() {
        let data = vec![10, 20, 30, 40, 50];
        assert_eq!(percentile(&data, 0.0), 10);
        assert_eq!(percentile(&data, 50.0), 30);
        assert_eq!(percentile(&data, 100.0), 50);
    }

    #[test]
    fn test_percentile_empty() {
        assert_eq!(percentile(&[], 50.0), 0);
    }

    #[test]
    fn test_percentile_single() {
        assert_eq!(percentile(&[42], 50.0), 42);
    }

    #[test]
    fn test_compute_precision_perfect() {
        let retrieved = vec![
            "proj/A.md".to_string(),
            "proj/B.md".to_string(),
            "proj/C.md".to_string(),
        ];
        let relevant = vec!["proj/A.md".to_string(), "proj/B.md".to_string()];
        let result = compute_precision_at_k(&retrieved, &relevant, &[1, 2, 3]);

        // P@1: 1/1 = 1.0, R@1: 1/2 = 0.5
        assert_eq!(result[0].k, 1);
        assert!((result[0].precision - 1.0).abs() < f64::EPSILON);
        assert!((result[0].recall - 0.5).abs() < f64::EPSILON);

        // P@2: 2/2 = 1.0, R@2: 2/2 = 1.0
        assert!((result[1].precision - 1.0).abs() < f64::EPSILON);
        assert!((result[1].recall - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_precision_no_overlap() {
        let retrieved = vec!["X.md".to_string(), "Y.md".to_string()];
        let relevant = vec!["A.md".to_string(), "B.md".to_string()];
        let result = compute_precision_at_k(&retrieved, &relevant, &[5]);

        assert!((result[0].precision - 0.0).abs() < f64::EPSILON);
        assert!((result[0].recall - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_precision_empty_relevant() {
        let retrieved = vec!["A.md".to_string()];
        let result = compute_precision_at_k(&retrieved, &[], &[1]);

        // P@1 = 0/1 = 0.0, R@1 = 1.0 (no relevant docs = perfect recall)
        assert!((result[0].precision - 0.0).abs() < f64::EPSILON);
        assert!((result[0].recall - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_precision_case_insensitive() {
        let retrieved = vec!["Proj/Architecture.md".to_string()];
        let relevant = vec!["proj/architecture.md".to_string()];
        let result = compute_precision_at_k(&retrieved, &relevant, &[1]);

        assert!((result[0].precision - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_extract_retrieved_files_ranked() {
        let json = serde_json::json!({
            "matches": [
                {"project": "alcove", "file": "ARCHITECTURE.md", "score": 5.0},
                {"project": "alcove", "file": "PRD.md", "score": 3.0},
            ]
        });
        let files = extract_retrieved_files(&json, "ranked");
        assert_eq!(files, vec!["alcove/ARCHITECTURE.md", "alcove/PRD.md"]);
    }

    #[test]
    fn test_extract_retrieved_files_grep() {
        let json = serde_json::json!({
            "matches": [
                {"file": "ARCHITECTURE.md", "line": 1, "snippet": "test"},
            ]
        });
        let files = extract_retrieved_files(&json, "grep");
        assert_eq!(files, vec!["ARCHITECTURE.md"]);
    }

    #[test]
    fn test_extract_retrieved_files_empty() {
        let json = serde_json::json!({"matches": []});
        let files = extract_retrieved_files(&json, "ranked");
        assert!(files.is_empty());
    }

    // --- Chunk-level tests ---

    #[test]
    fn test_extract_retrieved_chunks_ranked() {
        let json = serde_json::json!({
            "matches": [
                {"project": "alcove", "file": "ARCHITECTURE.md", "chunk_id": 0, "line_start": 1, "snippet": "text", "score": 3.0},
                {"project": "alcove", "file": "PRD.md", "chunk_id": 2, "line_start": 45, "snippet": "text", "score": 2.0},
            ]
        });
        let chunks = extract_retrieved_chunks(&json, "ranked");
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].file, "alcove/architecture.md");
        assert_eq!(chunks[0].line_start, 1);
        assert_eq!(chunks[1].file, "alcove/prd.md");
        assert_eq!(chunks[1].line_start, 45);
    }

    #[test]
    fn test_extract_retrieved_chunks_grep() {
        let json = serde_json::json!({
            "matches": [
                {"project": "alcove", "file": "test.md", "line": 10, "snippet": "hit"},
            ]
        });
        let chunks = extract_retrieved_chunks(&json, "grep");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].line_start, 10);
    }

    #[test]
    fn test_resolve_section_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.md");
        std::fs::write(
            &file_path,
            "# Title\n\nIntro text\n\n## Section One\n\nBody one\n\n## Section Two\n\nBody two\n",
        )
        .unwrap();

        let result = resolve_section_lines(dir.path(), "test.md", "## Section One");
        assert!(result.is_some());
        let (start, end) = result.unwrap();
        assert_eq!(start, 5); // "## Section One" is line 5 (1-based)
        assert_eq!(end, 8); // ends before "## Section Two" at line 9

        let result2 = resolve_section_lines(dir.path(), "test.md", "## Section Two");
        assert!(result2.is_some());
        let (start2, end2) = result2.unwrap();
        assert_eq!(start2, 9);
        assert_eq!(end2, 11); // last section extends to EOF (11 lines)
    }

    #[test]
    fn test_resolve_section_lines_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.md");
        std::fs::write(&file_path, "# Title\nNo headings here\n").unwrap();
        let result = resolve_section_lines(dir.path(), "test.md", "## Missing");
        assert!(result.is_none());
    }

    #[test]
    fn test_compute_chunk_precision() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("doc.md");
        std::fs::write(
            &file_path,
            "# Doc\n\n## Alpha\n\nAlpha content\n\n## Beta\n\nBeta content\n",
        )
        .unwrap();

        let chunks = vec![
            RetrievedChunk {
                file: "doc.md".into(),
                line_start: 4,
            }, // inside Alpha
            RetrievedChunk {
                file: "doc.md".into(),
                line_start: 8,
            }, // inside Beta
            RetrievedChunk {
                file: "other.md".into(),
                line_start: 1,
            }, // wrong file
        ];
        let sections = vec![RelevantSection {
            file: "doc.md".into(),
            heading: "## Alpha".into(),
        }];

        let pak = compute_chunk_precision(&chunks, &sections, dir.path(), &[2, 3]);
        assert_eq!(pak.len(), 2);
        // Top-2: 1 hit (Alpha) / 2 = 0.5
        assert!((pak[0].precision - 0.5).abs() < 1e-9);
        assert!((pak[0].recall - 1.0).abs() < 1e-9);
        // Top-3: 1 hit / 3 ≈ 0.333
        assert!(pak[1].precision > 0.0 && pak[1].precision < 0.5);
    }

    #[test]
    fn test_load_ground_truth() {
        let dir = tempfile::tempdir().unwrap();
        let gt_path = dir.path().join("ground_truth.toml");
        std::fs::write(
            &gt_path,
            r#"[[query]]
text = "hello"
relevant_files = ["a.md", "b.md"]
project = "global"

[[query]]
text = "world"
relevant_files = ["c.md"]
"#,
        )
        .unwrap();

        let entries = load_ground_truth(&gt_path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "hello");
        assert_eq!(entries[0].relevant_files, vec!["a.md", "b.md"]);
        assert_eq!(entries[1].text, "world");
        assert!(entries[1].project.is_none());
    }

    #[test]
    fn test_load_ground_truth_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let gt_path = dir.path().join("bad.toml");
        std::fs::write(&gt_path, "not valid toml {{{").unwrap();
        assert!(load_ground_truth(&gt_path).is_err());
    }

    #[test]
    fn test_average_precision_at_k() {
        let queries = vec![
            PerQueryPrecision {
                query: "a".to_string(),
                precision_at_k: vec![
                    PrecisionAtK {
                        k: 5,
                        precision: 0.6,
                        recall: 0.5,
                        ndcg: None,
                        map: None,
                    },
                    PrecisionAtK {
                        k: 10,
                        precision: 0.5,
                        recall: 0.8,
                        ndcg: None,
                        map: None,
                    },
                ],
                mrr: None,
                category: None,
                difficulty: None,
            },
            PerQueryPrecision {
                query: "b".to_string(),
                precision_at_k: vec![
                    PrecisionAtK {
                        k: 5,
                        precision: 0.4,
                        recall: 0.3,
                        ndcg: None,
                        map: None,
                    },
                    PrecisionAtK {
                        k: 10,
                        precision: 0.3,
                        recall: 0.6,
                        ndcg: None,
                        map: None,
                    },
                ],
                mrr: None,
                category: None,
                difficulty: None,
            },
        ];
        assert!((average_precision_at_k(&queries, 5) - 0.5).abs() < f64::EPSILON);
        assert!((average_precision_at_k(&queries, 10) - 0.4).abs() < f64::EPSILON);
        assert!((average_precision_at_k(&[], 5) - 0.0).abs() < f64::EPSILON);
    }

    // --- IR metrics tests ---

    #[test]
    fn test_ndcg_perfect_ranking() {
        let retrieved = vec!["a.md".into(), "b.md".into(), "c.md".into()];
        let relevant = vec!["a.md".into(), "b.md".into()];
        // All relevant at top → NDCG@5 should be 1.0
        let ndcg = compute_ndcg_at_k(&retrieved, &relevant, 5);
        assert!((ndcg - 1.0).abs() < 1e-9, "expected 1.0, got {ndcg}");
    }

    #[test]
    fn test_ndcg_partial_ranking() {
        let retrieved = vec!["x.md".into(), "a.md".into(), "y.md".into()];
        let relevant = vec!["a.md".into(), "b.md".into()];
        // Only 1 of 2 relevant in top-3, at position 2
        let ndcg = compute_ndcg_at_k(&retrieved, &relevant, 3);
        assert!(ndcg > 0.0 && ndcg < 1.0, "expected (0, 1), got {ndcg}");
    }

    #[test]
    fn test_ndcg_no_relevant() {
        let retrieved = vec!["x.md".into()];
        let ndcg = compute_ndcg_at_k(&retrieved, &[], 5);
        assert!((ndcg - 1.0).abs() < 1e-9, "vacuously perfect: {ndcg}");
    }

    #[test]
    fn test_ndcg_nothing_retrieved() {
        let ndcg = compute_ndcg_at_k(&[], &["a.md".into()], 5);
        assert!((ndcg - 0.0).abs() < 1e-9, "no results: {ndcg}");
    }

    #[test]
    fn test_map_perfect() {
        let retrieved = vec!["a.md".into(), "b.md".into(), "c.md".into()];
        let relevant = vec!["a.md".into(), "b.md".into()];
        let map = compute_map_at_k(&retrieved, &relevant, 5);
        assert!((map - 1.0).abs() < 1e-9, "expected 1.0, got {map}");
    }

    #[test]
    fn test_map_partial() {
        let retrieved = vec!["x.md".into(), "a.md".into()];
        let relevant = vec!["a.md".into(), "b.md".into()];
        let map = compute_map_at_k(&retrieved, &relevant, 5);
        // At pos 2, found 1st relevant → P@2 = 1/2 = 0.5; avg / 2 = 0.25
        assert!(map > 0.0 && map < 1.0, "expected partial, got {map}");
    }

    #[test]
    fn test_map_no_relevant() {
        let map = compute_map_at_k(&["a.md".into()], &[], 5);
        assert!((map - 1.0).abs() < 1e-9, "vacuously perfect: {map}");
    }

    #[test]
    fn test_mrr_first_position() {
        let retrieved = vec!["a.md".into(), "b.md".into()];
        let mrr = compute_mrr(&retrieved, &["a.md".into()]);
        assert!((mrr - 1.0).abs() < 1e-9, "expected 1.0, got {mrr}");
    }

    #[test]
    fn test_mrr_third_position() {
        let retrieved = vec!["x.md".into(), "y.md".into(), "a.md".into()];
        let mrr = compute_mrr(&retrieved, &["a.md".into()]);
        assert!((mrr - 1.0 / 3.0).abs() < 1e-9, "expected 0.333, got {mrr}");
    }

    #[test]
    fn test_mrr_not_found() {
        let mrr = compute_mrr(&["x.md".into()], &["a.md".into()]);
        assert!((mrr - 0.0).abs() < 1e-9, "expected 0.0, got {mrr}");
    }

    #[test]
    fn test_mrr_no_relevant() {
        let mrr = compute_mrr(&["a.md".into()], &[]);
        assert!((mrr - 1.0).abs() < 1e-9, "vacuously perfect: {mrr}");
    }

    // --- Output formatter tests ---

    fn sample_results() -> BenchResults {
        BenchResults {
            timestamp: "2026-04-30T00:00:00Z".to_string(),
            environment: EnvInfo {
                os: "macos".to_string(),
                rust_version: "1.85.0".to_string(),
                alcove_version: "0.8.1".to_string(),
            },
            data_stats: DataStats {
                file_count: 2,
                total_size_bytes: 1024,
            },
            precision: Some(PrecisionResults {
                grep: vec![PerQueryPrecision {
                    query: "architecture".to_string(),
                    precision_at_k: vec![
                        PrecisionAtK {
                            k: 5,
                            precision: 0.6,
                            recall: 0.5,
                            ndcg: None,
                            map: None,
                        },
                        PrecisionAtK {
                            k: 10,
                            precision: 0.5,
                            recall: 0.8,
                            ndcg: None,
                            map: None,
                        },
                        PrecisionAtK {
                            k: 20,
                            precision: 0.3,
                            recall: 0.9,
                            ndcg: None,
                            map: None,
                        },
                    ],
                    mrr: None,
                    category: None,
                    difficulty: None,
                }],
                ranked: vec![PerQueryPrecision {
                    query: "architecture".to_string(),
                    precision_at_k: vec![
                        PrecisionAtK {
                            k: 5,
                            precision: 0.8,
                            recall: 0.7,
                            ndcg: None,
                            map: None,
                        },
                        PrecisionAtK {
                            k: 10,
                            precision: 0.7,
                            recall: 0.9,
                            ndcg: None,
                            map: None,
                        },
                        PrecisionAtK {
                            k: 20,
                            precision: 0.5,
                            recall: 1.0,
                            ndcg: None,
                            map: None,
                        },
                    ],
                    mrr: None,
                    category: None,
                    difficulty: None,
                }],
                hybrid: None,
            }),
            latency: Some(LatencyResults {
                grep: vec![LatencyEntry {
                    query: "architecture".to_string(),
                    avg_us: 11000,
                    p50_us: 10500,
                    p95_us: 12000,
                    p99_us: 13000,
                }],
                ranked: vec![LatencyEntry {
                    query: "architecture".to_string(),
                    avg_us: 9500,
                    p50_us: 9200,
                    p95_us: 10000,
                    p99_us: 11000,
                }],
                hybrid: None,
            }),
            throughput: Some(ThroughputResults {
                full_rebuild_ms: 285,
                incremental_no_change_ms: 11,
                stale_detection_us: 500,
            }),
            disk_usage: Some(DiskUsageResults {
                index_bytes: 2048,
                docs_bytes: 1024,
                ratio_percent: 200.0,
            }),
            chunk_precision: None,
            regression: None,
        }
    }

    #[test]
    fn test_format_human_contains_key_info() {
        let results = sample_results();
        let output = format_human(&results);
        assert!(output.contains("Alcove Benchmark Results"));
        assert!(output.contains("0.8.1"));
        assert!(output.contains("Search Quality"));
        assert!(output.contains("Search Latency"));
        assert!(output.contains("Index Throughput"));
        assert!(output.contains("Disk Usage"));
        assert!(output.contains("285 ms"));
    }

    #[test]
    fn test_format_markdown_has_sections() {
        let results = sample_results();
        let md = format_markdown(&results);
        assert!(md.contains("# Alcove Benchmark Report"));
        assert!(md.contains("## 1. Summary"));
        assert!(md.contains("## 2. Search Speed"));
        assert!(md.contains("## 3. Index Build Performance"));
        assert!(md.contains("## 4. Search Quality"));
        assert!(md.contains("## 5. Disk Usage"));
        assert!(md.contains("## 8. Conclusion"));
        assert!(md.contains("| grep |"));
        assert!(md.contains("| ranked |"));
        assert!(md.contains("285 ms"));
    }

    #[test]
    fn test_format_markdown_conclusion() {
        let results = sample_results();
        let md = format_markdown(&results);
        // ranked (9500us) faster than grep (11000us)
        assert!(md.contains("faster"));
        assert!(md.contains("9500"));
        assert!(md.contains("11000"));
    }

    #[test]
    fn test_results_serialize_to_json() {
        let results = sample_results();
        let json = serde_json::to_string(&results).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["environment"]["os"], "macos");
        assert_eq!(parsed["environment"]["alcove_version"], "0.8.1");
        assert_eq!(parsed["data_stats"]["file_count"], 2);
        assert!(parsed["precision"].is_object());
        assert!(parsed["latency"].is_object());
        assert!(parsed["throughput"].is_object());
        assert!(parsed["disk_usage"].is_object());
    }

    // --- Integration: fixture-based search benchmark ---

    #[test]
    fn test_grep_search_on_fixture() {
        let dir = tempfile::tempdir().unwrap();
        make_fixture_docs(dir.path());

        // Run grep search via tools::tool_search
        let result = crate::tools::tool_search(
            &dir.path().join("test-project"),
            serde_json::json!({"query": "architecture", "limit": 20}),
            None,
        )
        .unwrap();

        let matches = result["matches"].as_array().unwrap();
        assert!(
            !matches.is_empty(),
            "grep should find 'architecture' in fixture docs"
        );
    }

    #[test]
    fn test_precision_on_fixture() {
        let dir = tempfile::tempdir().unwrap();
        make_fixture_docs(dir.path());

        // Run grep search and compute precision
        let result = crate::tools::tool_search(
            &dir.path().join("test-project"),
            serde_json::json!({"query": "architecture", "limit": 20}),
            None,
        )
        .unwrap();

        let files = extract_retrieved_files(&result, "grep");
        assert!(!files.is_empty());

        let relevant = vec!["ARCHITECTURE.md".to_string()];
        let pak = compute_precision_at_k(&files, &relevant, &[5, 10, 20]);

        // Should find ARCHITECTURE.md since it contains "architecture"
        assert!(
            pak[0].recall > 0.0,
            "grep should find the relevant file, got recall={}",
            pak[0].recall
        );
    }

    #[test]
    fn test_collect_data_stats_on_fixture() {
        let dir = tempfile::tempdir().unwrap();
        make_fixture_docs(dir.path());

        let stats = collect_data_stats(dir.path());
        assert_eq!(stats.file_count, 2, "should count 2 markdown files");
        assert!(stats.total_size_bytes > 0, "files should have content");
    }

    #[test]
    fn test_full_bench_flow_with_fixture() {
        let dir = tempfile::tempdir().unwrap();
        make_fixture_docs(dir.path());

        let gt_path = dir.path().join("ground_truth.toml");
        make_ground_truth(&gt_path);

        // Build index
        let build_result = crate::index::build_index_unlocked(dir.path())
            .expect("build_index should succeed on fixture");
        let indexed = build_result["indexed"].as_u64().unwrap_or(0);
        assert!(
            indexed > 0,
            "index should have indexed files, got: {build_result}"
        );

        // Run precision benchmark
        let gt = load_ground_truth(&gt_path).unwrap();
        assert_eq!(gt.len(), 3);

        // Test ranked search
        let ranked_result =
            crate::index::search_indexed(dir.path(), "architecture", 20, None).unwrap();
        let ranked_files = extract_retrieved_files(&ranked_result, "ranked");
        assert!(
            !ranked_files.is_empty(),
            "ranked search should find results"
        );

        // Compute precision
        let pak = compute_precision_at_k(&ranked_files, &gt[0].relevant_files, &[5, 10, 20]);
        assert!(
            pak[0].recall > 0.0,
            "should find ARCHITECTURE.md via ranked search"
        );
    }
}
