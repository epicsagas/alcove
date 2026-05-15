//! Search-result post-processing layer that compresses alcove results via
//! `llm-transpile` when the `transpile` feature is enabled.
//!
//! ## Activation
//! Set the environment variable `ALCOVE_TRANSPILE=true` (or `1`/`yes`) at
//! runtime to enable compression. The feature flag `transpile` must also be
//! compiled in.
//!
//! ## Fidelity
//! `ALCOVE_FIDELITY` controls the compression level:
//! - `lossless`   → `FidelityLevel::Lossless` (no compression, metadata only)
//! - `compressed` → `FidelityLevel::Compressed` (maximum reduction)
//! - anything else (default) → `FidelityLevel::Semantic` (balanced)
//!
//! ## Output
//! The MCP response JSON gains a `transpile_stats` field:
//! ```json
//! {
//!   "transpile_stats": {
//!     "original_tokens":    1890,
//!     "compressed_tokens":  1240,
//!     "reduction_pct":      34.4,
//!     "fidelity":          "semantic"
//!   }
//! }
//! ```
//!
//! If transpilation is disabled or fails, the original result is returned
//! unchanged and `transpile_stats` is absent.

#[cfg(feature = "transpile")]
use llm_transpile::{FidelityLevel, InputFormat, transpile};

use serde_json::Value;

#[cfg(feature = "transpile")]
use serde_json::json;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Post-process a search result JSON value by compressing snippet text.
///
/// `result` is the JSON returned by `tool_search` / `tool_search_global`.
/// Returns a potentially modified copy with `transpile_stats` injected.
///
/// This function is always compiled regardless of feature flags; the actual
/// compression is gated inside by `#[cfg(feature = "transpile")]`.
pub fn maybe_transpile_result(result: Value) -> Value {
    if !transpile_enabled() {
        return result;
    }
    #[cfg(feature = "transpile")]
    {
        return run_transpile(result);
    }
    #[allow(unreachable_code)]
    result
}

/// Returns `true` when the caller has opted into transpilation at runtime.
pub fn transpile_enabled() -> bool {
    matches!(
        std::env::var("ALCOVE_TRANSPILE")
            .unwrap_or_default()
            .to_lowercase()
            .as_str(),
        "true" | "1" | "yes"
    )
}

// ---------------------------------------------------------------------------
// Internal compression logic (only compiled when feature = "transpile")
// ---------------------------------------------------------------------------

#[cfg(feature = "transpile")]
fn run_transpile(mut result: Value) -> Value {
    let fidelity = parse_fidelity();

    // Collect all snippet strings from matches
    let Some(matches) = result.get("matches").and_then(|m| m.as_array()) else {
        return result;
    };

    // Build a single markdown document from all snippets for batch compression
    let combined;
    let original_tokens_approx: usize;

    {
        let mut parts: Vec<String> = Vec::with_capacity(matches.len());
        for m in matches {
            let snippet = m
                .get("snippet")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .trim();
            if snippet.is_empty() {
                continue;
            }
            let file = m.get("file").and_then(|f| f.as_str()).unwrap_or("?");
            parts.push(format!("## {file}\n{snippet}"));
        }
        combined = parts.join("\n\n");
        original_tokens_approx = llm_transpile::token_count(&combined);
    }

    if combined.is_empty() || original_tokens_approx == 0 {
        return result;
    }

    // Compute a rough budget: target 75% of original tokens
    let budget = ((original_tokens_approx as f64) * 0.75) as usize;

    // Run transpilation
    let compressed = match transpile(&combined, InputFormat::Markdown, fidelity, Some(budget)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[alcove/transpile] compression error: {e}");
            return result;
        }
    };

    let compressed_tokens = llm_transpile::token_count(&compressed);
    let reduction_pct = if original_tokens_approx > 0 {
        (1.0 - compressed_tokens as f64 / original_tokens_approx as f64) * 100.0
    } else {
        0.0
    };

    // Inject transpile_stats into the result
    result["transpile_stats"] = json!({
        "original_tokens":   original_tokens_approx,
        "compressed_tokens": compressed_tokens,
        "reduction_pct":     (reduction_pct * 10.0).round() / 10.0,
        "fidelity":          fidelity_label(fidelity),
    });

    // Replace all snippets with the compressed combined text (best-effort:
    // we store the full compressed blob in a top-level field so agents can
    // access it; individual snippets remain unmodified for backward compat).
    result["compressed_context"] = Value::String(compressed);

    result
}

#[cfg(feature = "transpile")]
fn parse_fidelity() -> FidelityLevel {
    match std::env::var("ALCOVE_FIDELITY")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "lossless" => FidelityLevel::Lossless,
        "compressed" => FidelityLevel::Compressed,
        _ => FidelityLevel::Semantic, // default
    }
}

#[cfg(feature = "transpile")]
fn fidelity_label(f: FidelityLevel) -> &'static str {
    match f {
        FidelityLevel::Lossless => "lossless",
        FidelityLevel::Semantic => "semantic",
        FidelityLevel::Compressed => "compressed",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serial_test::serial;

    #[test]
    #[serial]
    fn transpile_disabled_by_default() {
        // SAFETY: serialized by #[serial]; no concurrent env access
        unsafe { std::env::remove_var("ALCOVE_TRANSPILE") };
        assert!(!transpile_enabled());
    }

    #[test]
    #[serial]
    fn transpile_enabled_when_env_set() {
        // SAFETY: serialized by #[serial]; no concurrent env access
        unsafe { std::env::set_var("ALCOVE_TRANSPILE", "true") };
        assert!(transpile_enabled());
        unsafe { std::env::remove_var("ALCOVE_TRANSPILE") };
    }

    #[test]
    #[serial]
    fn transpile_enabled_accepts_1_and_yes() {
        for val in ["1", "yes", "YES", "True"] {
            // SAFETY: serialized by #[serial]; no concurrent env access
            unsafe { std::env::set_var("ALCOVE_TRANSPILE", val) };
            assert!(
                transpile_enabled(),
                "should be enabled for ALCOVE_TRANSPILE={val}"
            );
        }
        unsafe { std::env::remove_var("ALCOVE_TRANSPILE") };
    }

    #[test]
    #[serial]
    fn maybe_transpile_passthrough_when_disabled() {
        // SAFETY: serialized by #[serial]; no concurrent env access
        unsafe { std::env::remove_var("ALCOVE_TRANSPILE") };
        let input = json!({
            "query": "test",
            "matches": [{"file": "a.md", "snippet": "hello world", "line": 1}]
        });
        let output = maybe_transpile_result(input.clone());
        // Without the feature enabled, output should be unchanged
        assert_eq!(output["query"], input["query"]);
        assert!(output.get("transpile_stats").is_none());
    }
}
