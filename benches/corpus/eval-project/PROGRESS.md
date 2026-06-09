# eval-project Progress Tracker

## Current Status: v0.11 (Active Development)

The project is in active development with a focus on benchmark quality and multi-language search. The core search pipeline is feature-complete and stable.

## Version History

### v0.11 -- Hybrid Search Improvements (2025-05-28)

- Added Reciprocal Rank Fusion for merging BM25 and vector results.
- Implemented parallel retrieval across both search paths using tokio tasks.
- Added configurable RRF k parameter (default 60).

### v0.10 -- CJK Tokenizer Support (2025-05-14)

- Integrated character bigram tokenizer for Korean and Chinese text.
- Added CJK-specific field boosts in BM25 configuration.
- Fixed Unicode normalization issues in heading extraction.

### v0.9 -- Vector Search (2025-04-30)

- Implemented vector embedding pipeline with pluggable model backends.
- Added cosine similarity search using HNSW index.
- Integrated hybrid search combining BM25 and vector results.

### v0.8 -- Heading-Based Chunking (2025-04-15)

- Rewrote chunking logic to split documents at heading boundaries.
- Added heading path metadata for hierarchical context in results.
- Improved snippet extraction to respect chunk boundaries.

### v0.7 -- MCP Server Protocol (2025-04-01)

- Implemented MCP specification version 2025-03-26.
- Registered search, get_document, list_projects, and rebuild_index tools.
- Added stdio transport for local agent integration.

### v0.6 -- File Watcher (2025-03-18)

- Added inotify-based file watcher for Linux and FSEvents for macOS.
- Implemented incremental re-indexing for changed files only.
- Added debounced batch updates to avoid thrashing on large vault changes.

### v0.5 -- Tantivy Integration (2025-03-05)

- Replaced naive search with Tantivy full-text index.
- Configured BM25 scoring with custom tokenizer pipeline.
- Added field-level boosts for title and heading fields.

### v0.4 -- Basic Search (2025-02-18)

- Implemented simple substring search over parsed Markdown content.
- Added project scoping for multi-vault queries.
- Basic result ranking by document modification time.

### v0.3 -- Markdown Parser (2025-02-01)

- Built a pulldown-cmark based parser for Markdown files.
- Extracted frontmatter, headings, code blocks, and links.
- Added metadata indexing for document properties.

### v0.2 -- Configuration System (2025-01-15)

- Designed TOML-based configuration with project vault definitions.
- Added `eval-project init` interactive setup command.
- Implemented include/exclude glob patterns for file filtering.

### v0.1 -- Project Scaffolding (2025-01-02)

- Initialized Rust project with clap CLI framework.
- Set up CI pipeline with check, clippy, fmt, and test stages.
- Established project structure with src/server, src/index, and src/ingest modules.

## Telemetry

The server collects anonymized usage telemetry to guide development priorities:

- **PostHog** -- Event tracking for feature usage (search queries, tool invocations, indexing frequency). All events are anonymized and contain no user content.
- **Sentry** -- Error and panic reporting with stack trace capture. Crash reports help identify stability issues across different corpus sizes and configurations.

Telemetry can be disabled entirely by setting `EVAL_PROJECT_TELEMETRY=off` in the environment.

## Upcoming Milestones

- v0.12 -- Benchmark framework with precision@k and NDCG evaluation.
- v0.13 -- PDF file support via Apache Tika extraction.
- v1.0 -- Stable API, documentation complete, production readiness review.
