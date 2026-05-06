use std::path::Path;

// ---------------------------------------------------------------------------
// Chunking
// ---------------------------------------------------------------------------

pub(crate) const CHUNK_SIZE: usize = 1500; // chars per chunk (prose / markdown)
pub(crate) const CHUNK_OVERLAP: usize = 300; // overlap between chunks (prose)

/// Smaller limits for source-code files — keeps chunks inside function boundaries.
pub(crate) const CODE_CHUNK_SIZE: usize = 800;
pub(crate) const CODE_CHUNK_OVERLAP: usize = 150;

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
/// Code files use smaller chunks (800 chars / 150 overlap) so function
/// bodies are less likely to be split across chunk boundaries.
/// All other files use the default prose limits (1 500 / 300).
pub(crate) fn chunk_content(content: &str, ext: &str) -> Vec<Chunk> {
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
