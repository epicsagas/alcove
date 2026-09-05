# General Code Review Prompt

<!-- Canonical agnostic baseline — mirrors the rs-guard built-in DEFAULT_PROMPT.
     Use this as-is for any language or framework, or extend the
     "## Project-Specific Focus

## rust Guardrails
- No `unwrap()` or `expect()` outside `#[cfg(test)]` or `main()`.
- Prefer `?` and `anyhow::Context` for error propagation.
- Avoid `unsafe` blocks unless justified and documented.
- `tokio::spawn` tasks must be awaited or joined; no detached tasks.
- All public functions and types require doc comments (`#![deny(missing_docs)]`).

[RS_GUARD_VERDICT_METADATA]
Verdict: POSITIVE or NEGATIVE
CriticalIssues: <count>
SecurityIssues: <count>
ImportantIssues: <count>
Suggestions: <count>
