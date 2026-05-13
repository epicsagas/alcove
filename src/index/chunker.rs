use std::path::Path;

/// Returns true for markdown file extensions (`md`, `markdown`).
pub(crate) fn is_markdown_ext(ext: &str) -> bool {
    ext == "md" || ext == "markdown"
}

// ---------------------------------------------------------------------------
// Chunking config
// ---------------------------------------------------------------------------

/// Model-aware chunk sizing parameters.
///
/// Derives safe character limits from the embedding model's max token count,
/// using a conservative ~2 chars/token ratio (covers multilingual text).
pub(crate) struct ChunkConfig {
    pub(crate) prose_size: usize,
    pub(crate) prose_overlap: usize,
    pub(crate) code_size: usize,
    pub(crate) code_overlap: usize,
    pub(crate) md_heading_max: usize,
}

impl ChunkConfig {
    /// Derive chunk sizes from a model's max sequence length (tokens).
    ///
    /// Uses ~2.0 chars/token (conservative for CJK) with 75% utilization
    /// to leave headroom for special tokens, prefix, and overlap.
    #[cfg(feature = "embed")]
    pub(crate) fn for_max_tokens(max_tokens: usize) -> Self {
        let safe_chars = (max_tokens as f64 * 2.0 * 0.75) as usize;
        let prose_size = safe_chars.min(2000);
        let prose_overlap = (prose_size as f64 * 0.2) as usize;
        let code_size = (prose_size as f64 * 0.6) as usize;
        let code_overlap = (code_size as f64 * 0.2) as usize;
        let md_heading_max = (prose_size as f64 * 1.3) as usize;
        Self {
            prose_size,
            prose_overlap,
            code_size,
            code_overlap,
            md_heading_max,
        }
    }
}

// Default constants (used by BM25-only path without embedding model).
pub(crate) const CHUNK_SIZE: usize = 1500;
pub(crate) const CHUNK_OVERLAP: usize = 300;
pub(crate) const MD_HEADING_MAX_CHARS: usize = 2400;

/// Smaller limits for source-code files — keeps chunks inside function boundaries.
pub(crate) const CODE_CHUNK_SIZE: usize = 800;
pub(crate) const CODE_CHUNK_OVERLAP: usize = 150;

const DEFAULT_CHUNK_CONFIG: ChunkConfig = ChunkConfig {
    prose_size: CHUNK_SIZE,
    prose_overlap: CHUNK_OVERLAP,
    code_size: CODE_CHUNK_SIZE,
    code_overlap: CODE_CHUNK_OVERLAP,
    md_heading_max: MD_HEADING_MAX_CHARS,
};

/// File extensions that receive code-aware (smaller) chunking.
pub(crate) fn is_code_ext(ext: &str) -> bool {
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

pub(crate) struct Chunk {
    pub(crate) text: String,
    pub(crate) line_start: usize,
}

/// Chunk `content` using sensible size limits for the given file extension.
///
/// Markdown files (`md` / `markdown`) are routed through [`chunk_content_md`]
/// which splits on `##` headings first.  Code files use smaller chunks
/// (800 chars / 150 overlap) so function bodies are less likely to be split
/// across chunk boundaries.  All other files use the default prose limits
/// (1 500 / 300).
pub(crate) fn chunk_content(content: &str, ext: &str) -> Vec<Chunk> {
    chunk_content_with_config(content, ext, &DEFAULT_CHUNK_CONFIG)
}

/// Chunk `content` using model-aware size limits.
///
/// See [`ChunkConfig::for_max_tokens`] to derive config from a model's token limit.
pub(crate) fn chunk_content_with_config(
    content: &str,
    ext: &str,
    config: &ChunkConfig,
) -> Vec<Chunk> {
    if is_markdown_ext(ext) {
        return chunk_content_md_with_config(content, config);
    }

    let (chunk_size, overlap_size) = if is_code_ext(ext) {
        (config.code_size, config.code_overlap)
    } else {
        (config.prose_size, config.prose_overlap)
    };

    chunk_content_char_based(content, chunk_size, overlap_size, 0)
}

// ---------------------------------------------------------------------------
// Markdown heading-aware chunking
// ---------------------------------------------------------------------------

/// Markdown chunker with default limits (used by tests and BM25 path).
#[cfg(test)]
fn chunk_content_md(content: &str) -> Vec<Chunk> {
    chunk_content_md_with_config(content, &DEFAULT_CHUNK_CONFIG)
}

/// Chunk a markdown document by `##` (level-2) heading boundaries.
///
/// Algorithm:
/// 1. Scan lines; every `## ` line starts a new section.
/// 2. Sections shorter than 50 chars are merged into the following section.
/// 3. Sections longer than `config.md_heading_max` are sub-split with the
///    same line-based algorithm (prose limits from config).
/// 4. If the document contains no `##` headings the function falls back to
///    char-based prose splitting.
fn chunk_content_md_with_config(content: &str, config: &ChunkConfig) -> Vec<Chunk> {
    const MIN_SECTION_CHARS: usize = 50;

    // ── 1. Split into raw sections at every `##` boundary ──────────────────
    let lines: Vec<&str> = content.lines().collect();

    // Collect indices of `##` heading lines.
    let heading_indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.trim_start().starts_with("## "))
        .map(|(i, _)| i)
        .collect();

    // Fall back to char-based splitting when there are no `##` headings.
    if heading_indices.is_empty() {
        return chunk_content_char_based(content, config.prose_size, config.prose_overlap, 0);
    }

    // Build sections: each section is (line_start, heading_text, text).
    // Lines before the first `##` are treated as a preamble section with an
    // empty heading; they are merged into the first real section below.
    struct Section {
        line_start: usize,
        heading: String,
        text: String,
    }

    let mut raw_sections: Vec<Section> = Vec::new();

    // Preamble (before first `##`).
    if heading_indices[0] > 0 {
        let preamble = lines[..heading_indices[0]].join("\n");
        raw_sections.push(Section {
            line_start: 0,
            heading: String::new(),
            text: preamble,
        });
    }

    for (k, &start) in heading_indices.iter().enumerate() {
        let end = if k + 1 < heading_indices.len() {
            heading_indices[k + 1]
        } else {
            lines.len()
        };
        let heading = lines[start]
            .trim_start()
            .strip_prefix("## ")
            .unwrap_or("")
            .trim()
            .to_string();
        let text = lines[start..end].join("\n");
        raw_sections.push(Section {
            line_start: start,
            heading,
            text,
        });
    }

    // ── 2. Merge sections that are too short into the next section ──────────
    let mut merged: Vec<Section> = Vec::new();
    let mut pending: Option<Section> = None;

    for sec in raw_sections {
        match pending {
            None => {
                if sec.text.chars().count() < MIN_SECTION_CHARS {
                    // Start accumulating.
                    pending = Some(sec);
                } else {
                    merged.push(sec);
                }
            }
            Some(prev) => {
                // Merge prev into current section.
                let combined_text = if prev.text.is_empty() {
                    sec.text
                } else if sec.text.is_empty() {
                    prev.text
                } else {
                    format!("{}\n{}", prev.text, sec.text)
                };
                let merged_sec = Section {
                    line_start: prev.line_start,
                    heading: if prev.heading.is_empty() {
                        sec.heading
                    } else {
                        prev.heading
                    },
                    text: combined_text,
                };
                if merged_sec.text.chars().count() < MIN_SECTION_CHARS {
                    pending = Some(merged_sec);
                } else {
                    pending = None;
                    merged.push(merged_sec);
                }
            }
        }
    }
    // Flush any remaining pending section.
    if let Some(sec) = pending {
        if !merged.is_empty() {
            let last = merged.last_mut().unwrap();
            last.text = if last.text.is_empty() {
                sec.text
            } else {
                format!("{}\n{}", last.text, sec.text)
            };
        } else {
            merged.push(sec);
        }
    }

    // ── 3. Emit chunks; sub-split oversized sections ────────────────────────
    let mut chunks: Vec<Chunk> = Vec::new();

    for sec in merged {
        if sec.text.chars().count() <= config.md_heading_max {
            chunks.push(Chunk {
                text: sec.text,
                line_start: sec.line_start + 1,
            });
        } else {
            // Secondary split: prepend the heading to every sub-chunk so that
            // `extract_title` can recover it.
            let prefix = if sec.heading.is_empty() {
                String::new()
            } else {
                format!("## {}\n", sec.heading)
            };
            let sub = chunk_content_char_based(
                &sec.text,
                config.prose_size,
                config.prose_overlap,
                sec.line_start,
            );
            for mut sub_chunk in sub {
                if !prefix.is_empty() && !sub_chunk.text.starts_with("## ") {
                    sub_chunk.text = format!("{}{}", prefix, sub_chunk.text);
                }
                chunks.push(sub_chunk);
            }
        }
    }

    if chunks.is_empty() {
        // Safety net: return at least one chunk.
        chunks.push(Chunk {
            text: content.to_string(),
            line_start: 1,
        });
    }

    chunks
}

/// Internal helper: line-based char chunker with configurable size/overlap and
/// an optional line offset applied to every `line_start` value.
fn chunk_content_char_based(
    content: &str,
    chunk_size: usize,
    overlap_size: usize,
    line_offset: usize,
) -> Vec<Chunk> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return vec![];
    }

    let mut chunks = Vec::new();
    let mut current_chars: usize = 0;
    let mut chunk_lines: Vec<String> = Vec::new();
    let mut chunk_start_line: usize = 0;

    for (i, line) in lines.iter().enumerate() {
        let line_len = line.chars().count().saturating_add(1);
        if current_chars + line_len > chunk_size && !chunk_lines.is_empty() {
            chunks.push(Chunk {
                text: chunk_lines.join("\n"),
                line_start: line_offset + chunk_start_line + 1,
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
            line_start: line_offset + chunk_start_line + 1,
        });
    }

    chunks
}

// ---------------------------------------------------------------------------
// Title extraction
// ---------------------------------------------------------------------------

/// Extract a title for a chunk.
///
/// For the first chunk of a file, the first markdown heading (`#`, `##`, `###`)
/// is used.  If no heading is found, or for subsequent chunks, the filename
/// without extension is used as a fallback.
pub(crate) fn extract_title(chunk_text: &str, filename: &str, chunk_idx: usize) -> String {
    if chunk_idx == 0 {
        for line in chunk_text.lines() {
            let trimmed = line.trim();
            if let Some(heading) = trimmed.strip_prefix("# ") {
                return heading.trim().to_string();
            } else if let Some(heading) = trimmed.strip_prefix("## ") {
                return heading.trim().to_string();
            } else if let Some(heading) = trimmed.strip_prefix("### ") {
                return heading.trim().to_string();
            }
        }
    }
    // Fallback: filename without extension
    Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename)
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a multi-line string where each line is `line_len` 'x' chars.
    /// Total character count is approximately `total_chars` (rounded up to the
    /// nearest line boundary).
    fn make_lines(total_chars: usize, line_len: usize) -> String {
        let n_lines = total_chars.div_ceil(line_len);
        (0..n_lines)
            .map(|_| "x".repeat(line_len))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ── chunk_content_md ────────────────────────────────────────────────────

    #[test]
    fn md_two_headings_produce_two_chunks() {
        // Each section body is well above MIN_SECTION_CHARS (50) so they must
        // not be merged together.
        let intro_body = "x".repeat(80);
        let details_body = "y".repeat(80);
        let doc = format!(
            "## Introduction\n{intro_body}\n\n## Details\n{details_body}\n",
            intro_body = intro_body,
            details_body = details_body,
        );

        let chunks = chunk_content_md(&doc);
        assert!(
            chunks.len() >= 2,
            "expected at least 2 chunks, got {}",
            chunks.len()
        );
        // First chunk must contain the first `##` heading.
        assert!(
            chunks[0].text.contains("## Introduction"),
            "first chunk should contain '## Introduction'"
        );
        // Second chunk must contain the second `##` heading.
        assert!(
            chunks[1].text.contains("## Details"),
            "second chunk should contain '## Details'"
        );
    }

    #[test]
    fn md_no_headings_falls_back_to_char_based() {
        // A document with no `##` headings should behave like the char-based
        // prose chunker (CHUNK_SIZE = 1500).
        let doc = make_lines(4000, 100);
        let chunks_md = chunk_content_md(&doc);
        let chunks_ref = chunk_content_char_based(&doc, CHUNK_SIZE, CHUNK_OVERLAP, 0);
        assert_eq!(
            chunks_md.len(),
            chunks_ref.len(),
            "fallback should match char-based chunker"
        );
    }

    #[test]
    fn md_oversized_section_is_sub_split() {
        // Build a section that exceeds MD_HEADING_MAX_CHARS.
        // Use multi-line body so the char-based secondary splitter can split it.
        let body = make_lines(MD_HEADING_MAX_CHARS + 500, 100);
        let doc = format!("## Big Section\n{}\n", body);

        let chunks = chunk_content_md(&doc);
        assert!(
            chunks.len() >= 2,
            "oversized section should be sub-split into at least 2 chunks, got {}",
            chunks.len()
        );
        // Every sub-chunk should preserve the parent heading.
        for chunk in &chunks {
            assert!(
                chunk.text.contains("## Big Section"),
                "sub-chunk missing parent heading: {:?}",
                &chunk.text[..chunk.text.len().min(80)]
            );
        }
    }

    #[test]
    fn md_empty_section_merged_with_next() {
        // A `##` heading followed immediately by another `##` heading with no
        // body text between them — the empty section should not produce an
        // isolated tiny chunk.
        let doc = "## Empty\n\
                   ## Real Section\n\
                   This section has real content that is long enough.\n";

        let chunks = chunk_content_md(doc);
        // At least one chunk must exist.
        assert!(!chunks.is_empty());
        let all_text = chunks
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            all_text.contains("Real Section"),
            "merged output should contain 'Real Section'"
        );
    }

    // ── chunk_content routing ───────────────────────────────────────────────

    #[test]
    fn chunk_content_routes_md_extension() {
        let body = "x".repeat(80);
        let doc = format!(
            "## Heading One\n{body}\n## Heading Two\n{body}\n",
            body = body
        );
        let via_router = chunk_content(&doc, "md");
        let direct = chunk_content_md(&doc);
        assert_eq!(via_router.len(), direct.len());
    }

    #[test]
    fn chunk_content_routes_markdown_extension() {
        let body = "x".repeat(80);
        let doc = format!("## Alpha\n{body}\n## Beta\n{body}\n", body = body);
        let via_router = chunk_content(&doc, "markdown");
        let direct = chunk_content_md(&doc);
        assert_eq!(via_router.len(), direct.len());
    }

    // ── regression: existing char-based chunker ─────────────────────────────

    #[test]
    fn non_md_content_still_chunked_correctly() {
        // A Rust source file should use code chunk sizes, not MD sizes.
        // Use multi-line content so the line-based splitter can cut it.
        let code = make_lines(CODE_CHUNK_SIZE * 3, 80);
        let chunks = chunk_content(&code, "rs");
        // Should produce multiple chunks.
        assert!(
            chunks.len() >= 2,
            "code file should be split into multiple chunks"
        );
    }

    #[test]
    fn prose_content_chunked_at_prose_size() {
        let prose = make_lines(CHUNK_SIZE * 2 + 100, 100);
        let chunks = chunk_content(&prose, "txt");
        assert!(
            chunks.len() >= 2,
            "prose file should be split into multiple chunks"
        );
    }
}
