# Roadmap

Project started: 2026-03-06. Current version: 0.11.7.

## v1.0 — Stable Release

**Goal:** API lock, production readiness, publish-ready documentation.

- [ ] Freeze public API surface (CLI flags, MCP tool schemas, REST endpoints)
- [ ] Benchmark regression gates in CI (precision, latency, throughput thresholds)
- [ ] Complete search quality benchmark — verify Precision@5 ≥ 0.75 on ground truth
- [ ] Document all MCP tools with schema examples
- [ ] Remove `#[deprecated]` items and feature-flag guards that are no longer needed

## v1.1 — Search Quality

**Goal:** Improve retrieval accuracy, especially CJK and cross-lingual queries.

- [ ] Tune BM25 tokenizer and CJK ngram parameters from benchmark results
- [ ] Evaluate hybrid RRF weights across query categories
- [ ] Add relevance feedback loop — track which results users actually use
- [ ] Expand ground truth to 100+ queries covering edge cases

## v1.2 — Incremental Indexing

**Goal:** Avoid full re-index on embedding model changes or large vault updates.

- [ ] Incremental embedding updates (delta-only on changed files)
- [ ] Streaming index rebuild with progress reporting
- [ ] Background index warm-up on server start

## v1.3 — Code Intelligence

**Goal:** Deeper code structure understanding beyond symbol extraction.

- [ ] Cross-reference indexing (call graphs, import dependencies)
- [ ] Symbol-level search (find function, trait, struct by name)
- [ ] Language-specific chunking (function/class boundaries, not just headings)

## v2.0 — Multi-User

**Goal:** Team deployment with access control.

- [ ] Multi-user authentication (API key, OAuth)
- [ ] Per-project access control (read, write, admin)
- [ ] Shared index with tenant isolation
- [ ] Usage metrics and audit logging

## Future

- Federated search across multiple alcove instances
- Plugin system for custom tokenizers and scoring functions
- Web dashboard for search analytics and index management
- Managed cloud offering
