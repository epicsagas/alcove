use std::path::{Path, PathBuf};
use std::time::Instant;

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
pub struct GroundTruthEntry {
    pub text: String,
    pub relevant_files: Vec<String>,
    #[serde(default)]
    pub project: Option<String>,
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

#[derive(Debug, Serialize)]
pub struct EnvInfo {
    os: String,
    rust_version: String,
    alcove_version: String,
}

#[derive(Debug, Serialize)]
pub struct DataStats {
    file_count: u64,
    total_size_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct PrecisionAtK {
    k: usize,
    precision: f64,
    recall: f64,
}

#[derive(Debug, Serialize)]
pub struct PerQueryPrecision {
    query: String,
    precision_at_k: Vec<PrecisionAtK>,
}

#[derive(Debug, Serialize)]
pub struct PrecisionResults {
    grep: Vec<PerQueryPrecision>,
    ranked: Vec<PerQueryPrecision>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hybrid: Option<Vec<PerQueryPrecision>>,
}

#[derive(Debug, Serialize)]
pub struct LatencyEntry {
    query: String,
    avg_us: u128,
    p50_us: u128,
    p95_us: u128,
    p99_us: u128,
}

#[derive(Debug, Serialize)]
pub struct LatencyResults {
    grep: Vec<LatencyEntry>,
    ranked: Vec<LatencyEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hybrid: Option<Vec<LatencyEntry>>,
}

#[derive(Debug, Serialize)]
pub struct ThroughputResults {
    full_rebuild_ms: u128,
    incremental_no_change_ms: u128,
    stale_detection_us: u128,
}

#[derive(Debug, Serialize)]
pub struct DiskUsageResults {
    index_bytes: u64,
    docs_bytes: u64,
    ratio_percent: f64,
}

#[derive(Debug, Serialize)]
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
            .any(|c| {
                c.as_os_str()
                    .to_string_lossy()
                    .starts_with('.')
            });
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

pub(crate) fn default_ground_truth_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("benches")
        .join("ground_truth.toml")
}

fn get_docs_root() -> Result<PathBuf> {
    crate::cli::saved_docs_root().ok_or_else(|| {
        anyhow::anyhow!("No docs repository found. Run `alcove setup` first.")
    })
}

// ---------------------------------------------------------------------------
// Benchmark runners
// ---------------------------------------------------------------------------

fn run_precision_benchmark(
    docs_root: &Path,
    queries: &[GroundTruthEntry],
    _scope: &str,
) -> Result<PrecisionResults> {
    let k_values = [5, 10, 20];
    let limit = 20;

    let mut grep_results: Vec<PerQueryPrecision> = Vec::new();
    let mut ranked_results: Vec<PerQueryPrecision> = Vec::new();

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
        grep_results.push(PerQueryPrecision {
            query: entry.text.clone(),
            precision_at_k: grep_pak,
        });

        // Ranked search
        let ranked_result = crate::index::search_indexed(docs_root, &entry.text, limit, None)?;
        let ranked_files = extract_retrieved_files(&ranked_result, "ranked");
        let ranked_pak = compute_precision_at_k(&ranked_files, &entry.relevant_files, &k_values);
        ranked_results.push(PerQueryPrecision {
            query: entry.text.clone(),
            precision_at_k: ranked_pak,
        });
    }

    // Hybrid search (only if alcove-full feature is enabled)
    #[cfg(feature = "alcove-full")]
    let hybrid_results = {
        let mut results: Vec<PerQueryPrecision> = Vec::new();
        let cfg = crate::config::load_config();
        let emb_cfg = cfg.embedding_config_with_defaults();
        if emb_cfg.enabled {
            let model = crate::embedding::EmbeddingModelChoice::parse(&emb_cfg.model)
                .unwrap_or_default();
            let cache_dir = if emb_cfg.cache_dir.starts_with('~') {
                std::env::var("HOME")
                    .ok()
                    .map(|h| emb_cfg.cache_dir.replacen('~', &h, 1))
                    .unwrap_or_else(|| emb_cfg.cache_dir.clone())
            } else {
                emb_cfg.cache_dir.clone()
            };
            let service = crate::embedding::EmbeddingService::new(
                crate::config::EmbeddingConfig {
                    model: model.as_str().to_string(),
                    auto_download: emb_cfg.auto_download,
                    cache_dir,
                    enabled: true,
                    query_cache_size: emb_cfg.query_cache_size,
                },
            );

            for entry in queries {
                match crate::index::search_hybrid(
                    docs_root,
                    &entry.text,
                    &service,
                    limit,
                    None,
                ) {
                    Ok(hybrid_result) => {
                        let hybrid_files = extract_retrieved_files(&hybrid_result, "hybrid");
                        let hybrid_pak = compute_precision_at_k(
                            &hybrid_files,
                            &entry.relevant_files,
                            &k_values,
                        );
                        results.push(PerQueryPrecision {
                            query: entry.text.clone(),
                            precision_at_k: hybrid_pak,
                        });
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

    #[cfg(not(feature = "alcove-full"))]
    let hybrid_results: Option<Vec<PerQueryPrecision>> = None;

    Ok(PrecisionResults {
        grep: grep_results,
        ranked: ranked_results,
        hybrid: hybrid_results,
    })
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

    // Hybrid latency (only if alcove-full feature is enabled)
    #[cfg(feature = "alcove-full")]
    let hybrid_entries = {
        let mut entries: Vec<LatencyEntry> = Vec::new();
        let cfg = crate::config::load_config();
        let emb_cfg = cfg.embedding_config_with_defaults();
        if emb_cfg.enabled {
            let model = crate::embedding::EmbeddingModelChoice::parse(&emb_cfg.model)
                .unwrap_or_default();
            let cache_dir = if emb_cfg.cache_dir.starts_with('~') {
                std::env::var("HOME")
                    .ok()
                    .map(|h| emb_cfg.cache_dir.replacen('~', &h, 1))
                    .unwrap_or_else(|| emb_cfg.cache_dir.clone())
            } else {
                emb_cfg.cache_dir.clone()
            };
            let service = crate::embedding::EmbeddingService::new(
                crate::config::EmbeddingConfig {
                    model: model.as_str().to_string(),
                    auto_download: emb_cfg.auto_download,
                    cache_dir,
                    enabled: true,
                    query_cache_size: emb_cfg.query_cache_size,
                },
            );

            for entry in queries {
                // Warm-up
                let _ = crate::index::search_hybrid(
                    docs_root,
                    &entry.text,
                    &service,
                    limit,
                    None,
                );

                let mut times: Vec<u128> = Vec::with_capacity(iterations);
                for _ in 0..iterations {
                    let start = Instant::now();
                    let _ = crate::index::search_hybrid(
                        docs_root,
                        &entry.text,
                        &service,
                        limit,
                        None,
                    );
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

    #[cfg(not(feature = "alcove-full"))]
    let hybrid_entries: Option<Vec<LatencyEntry>> = None;

    Ok(LatencyResults {
        grep: grep_entries,
        ranked: ranked_entries,
        hybrid: hybrid_entries,
    })
}

fn run_throughput_benchmark(docs_root: &Path) -> Result<ThroughputResults> {
    // Full rebuild
    eprintln!(
        "  {} Measuring full rebuild...",
        style("...").dim()
    );
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
    eprintln!(
        "  {} Measuring stale detection...",
        style("...").dim()
    );
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
        out.push_str(&format!("\n{}\n", style("Search Quality (Precision@K / Recall@K)").bold()));
        out.push_str(&format!("{}\n", "-".repeat(70)));

        for (mode, queries) in [
            ("Grep", &precision.grep),
            ("Ranked", &precision.ranked),
        ] {
            out.push_str(&format!("\n  {}:\n", style(mode).cyan()));
            for q in queries {
                out.push_str(&format!("    \"{}\"\n", q.query));
                for pak in &q.precision_at_k {
                    out.push_str(&format!(
                        "      P@{}: {:.3}  R@{}: {:.3}\n",
                        pak.k, pak.precision, pak.k, pak.recall
                    ));
                }
            }
        }
        if let Some(ref hybrid) = precision.hybrid {
            out.push_str(&format!("\n  {}:\n", style("Hybrid").cyan()));
            for q in hybrid {
                out.push_str(&format!("    \"{}\"\n", q.query));
                for pak in &q.precision_at_k {
                    out.push_str(&format!(
                        "      P@{}: {:.3}  R@{}: {:.3}\n",
                        pak.k, pak.precision, pak.k, pak.recall
                    ));
                }
            }
        }
    }

    if let Some(ref latency) = results.latency {
        out.push_str(&format!("\n{}\n", style("Search Latency").bold()));
        out.push_str(&format!("{}\n", "-".repeat(70)));

        for (mode, entries) in [
            ("Grep", &latency.grep),
            ("Ranked", &latency.ranked),
        ] {
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

        for (label, queries) in [
            ("grep", &precision.grep),
            ("ranked", &precision.ranked),
        ] {
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

        for (mode_label, entries) in [
            ("grep", &latency.grep),
            ("ranked", &latency.ranked),
        ] {
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
        md.push_str(&format!("| Full rebuild | {} ms |\n", throughput.full_rebuild_ms));
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
        for (label, queries) in [
            ("Grep", &precision.grep),
            ("Ranked", &precision.ranked),
        ] {
            md.push_str(&format!("### {}\n\n", label));
            md.push_str("| Query | P@5 | R@5 | P@10 | R@10 | P@20 | R@20 |\n");
            md.push_str("|-------|-----|-----|------|------|------|------|\n");
            for q in queries {
                let vals: Vec<String> = q
                    .precision_at_k
                    .iter()
                    .flat_map(|pak| {
                        vec![
                            format!("{:.3}", pak.precision),
                            format!("{:.3}", pak.recall),
                        ]
                    })
                    .collect();
                md.push_str(&format!(
                    "| {} | {} |\n",
                    q.query,
                    vals.join(" | ")
                ));
            }
            md.push('\n');
        }
        if let Some(ref hybrid) = precision.hybrid {
            md.push_str("### Hybrid\n\n");
            md.push_str("| Query | P@5 | R@5 | P@10 | R@10 | P@20 | R@20 |\n");
            md.push_str("|-------|-----|-----|------|------|------|------|\n");
            for q in hybrid {
                let vals: Vec<String> = q
                    .precision_at_k
                    .iter()
                    .flat_map(|pak| {
                        vec![
                            format!("{:.3}", pak.precision),
                            format!("{:.3}", pak.recall),
                        ]
                    })
                    .collect();
                md.push_str(&format!(
                    "| {} | {} |\n",
                    q.query,
                    vals.join(" | ")
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

    // Conclusion
    md.push_str("## 6. Conclusion\n\n");
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
            if speed_ratio >= 1.0 { speed_ratio } else { 1.0 / speed_ratio },
            if avg_ranked < avg_grep { "faster" } else { "slower" },
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

pub fn cmd_bench(
    metrics: &str,
    scope: &str,
    output: &str,
    queries: Option<&Path>,
    output_file: Option<&Path>,
) -> Result<()> {
    let docs_root = get_docs_root()?;

    let queries_path = queries
        .map(PathBuf::from)
        .unwrap_or_else(default_ground_truth_path);

    if !queries_path.exists() {
        anyhow::bail!(
            "Ground truth file not found: {}. Create it or specify --queries <path>",
            queries_path.display()
        );
    }

    let ground_truth = load_ground_truth(&queries_path)?;
    if ground_truth.is_empty() {
        anyhow::bail!("Ground truth file contains no queries: {}", queries_path.display());
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

    let precision = if run_precision {
        eprintln!(
            "  {} Running precision benchmarks...",
            style("...").dim()
        );
        Some(run_precision_benchmark(&docs_root, &ground_truth, &config.scope)?)
    } else {
        None
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

    let results = BenchResults {
        timestamp: chrono_now_rfc3339(),
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
    };

    let output_content = match config.output.as_str() {
        "json" => serde_json::to_string_pretty(&results)?,
        "markdown" => format_markdown(&results),
        _ => format_human(&results),
    };

    if let Some(path) = output_file {
        // Auto-detect format from extension if --output not explicitly set
        let effective_content = if output == "human" {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            match ext {
                "json" => serde_json::to_string_pretty(&results)?,
                "md" | "markdown" => format_markdown(&results),
                _ => output_content.clone(),
            }
        } else {
            output_content.clone()
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &effective_content)?;
        eprintln!(
            "  {} Results saved to {}",
            style("✓").green(),
            path.display()
        );
    }

    println!("{output_content}");

    Ok(())
}

fn chrono_now_rfc3339() -> String {
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
                    PrecisionAtK { k: 5, precision: 0.6, recall: 0.5 },
                    PrecisionAtK { k: 10, precision: 0.5, recall: 0.8 },
                ],
            },
            PerQueryPrecision {
                query: "b".to_string(),
                precision_at_k: vec![
                    PrecisionAtK { k: 5, precision: 0.4, recall: 0.3 },
                    PrecisionAtK { k: 10, precision: 0.3, recall: 0.6 },
                ],
            },
        ];
        assert!((average_precision_at_k(&queries, 5) - 0.5).abs() < f64::EPSILON);
        assert!((average_precision_at_k(&queries, 10) - 0.4).abs() < f64::EPSILON);
        assert!((average_precision_at_k(&[], 5) - 0.0).abs() < f64::EPSILON);
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
                        PrecisionAtK { k: 5, precision: 0.6, recall: 0.5 },
                        PrecisionAtK { k: 10, precision: 0.5, recall: 0.8 },
                        PrecisionAtK { k: 20, precision: 0.3, recall: 0.9 },
                    ],
                }],
                ranked: vec![PerQueryPrecision {
                    query: "architecture".to_string(),
                    precision_at_k: vec![
                        PrecisionAtK { k: 5, precision: 0.8, recall: 0.7 },
                        PrecisionAtK { k: 10, precision: 0.7, recall: 0.9 },
                        PrecisionAtK { k: 20, precision: 0.5, recall: 1.0 },
                    ],
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
        assert!(md.contains("## 6. Conclusion"));
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
        assert!(!matches.is_empty(), "grep should find 'architecture' in fixture docs");
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
        assert!(indexed > 0, "index should have indexed files, got: {build_result}");

        // Run precision benchmark
        let gt = load_ground_truth(&gt_path).unwrap();
        assert_eq!(gt.len(), 3);

        // Test ranked search
        let ranked_result =
            crate::index::search_indexed(dir.path(), "architecture", 20, None).unwrap();
        let ranked_files = extract_retrieved_files(&ranked_result, "ranked");
        assert!(!ranked_files.is_empty(), "ranked search should find results");

        // Compute precision
        let pak = compute_precision_at_k(&ranked_files, &gt[0].relevant_files, &[5, 10, 20]);
        assert!(pak[0].recall > 0.0, "should find ARCHITECTURE.md via ranked search");
    }
}
