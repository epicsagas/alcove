# Roadmap

Project started: 2026-03-06. Current version: 0.11.7.

## v0.12 — Benchmark & Quality Gates

**Goal:** Validate search quality with data, establish regression baseline.

- [ ] Benchmark regression gates in CI (precision, latency, throughput thresholds)
- [ ] Complete search quality benchmark — verify Precision@5 ≥ 0.75 on ground truth
- [ ] Tune BM25 tokenizer and CJK ngram parameters from benchmark results
- [ ] Evaluate hybrid RRF weights across query categories
- [ ] Expand ground truth to 100+ queries covering edge cases

## v0.13 — API Hardening

**Goal:** Lock public API surface, clean up deprecated paths.

- [ ] Freeze public API surface (CLI flags, MCP tool schemas, REST endpoints)
- [ ] Document all MCP tools with schema examples
- [ ] Remove `#[deprecated]` items and feature-flag guards that are no longer needed
- [ ] Add relevance feedback loop — track which results users actually use

## v0.14 — Incremental Indexing

**Goal:** Avoid full re-index on embedding model changes or large vault updates.

- [ ] Incremental embedding updates (delta-only on changed files)
- [ ] Streaming index rebuild with progress reporting
- [ ] Background index warm-up on server start

## v0.15 — Code Intelligence

**Goal:** Deeper code structure understanding beyond symbol extraction.

- [ ] Cross-reference indexing (call graphs, import dependencies)
- [ ] Symbol-level search (find function, trait, struct by name)
- [ ] Language-specific chunking (function/class boundaries, not just headings)

## v1.0 — Stable Release

**Goal:** Ship v1.0 with validated quality and locked API.

- [ ] All v0.12–v0.15 items verified in production
- [ ] Security audit pass
- [ ] Migration guide from 0.x to 1.0

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
