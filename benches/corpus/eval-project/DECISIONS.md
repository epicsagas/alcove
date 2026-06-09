# eval-project Architecture Decision Records

This document records the key technical decisions made during eval-project development, along with their context and rationale.

## ADR-001: Tantivy over Elasticsearch for Full-Text Search

**Date:** 2025-02-20
**Status:** Accepted

### Context

We needed a full-text search engine that could run embedded within our Rust server process. Elasticsearch was considered as the industry standard, but it requires a separate JVM process and HTTP communication overhead.

### Decision

Use Tantivy as the embedded search engine library.

### Rationale

- Tantivy is a native Rust library with zero external process dependencies.
- Index performance is comparable to Lucene for our corpus sizes (under 100k documents).
- Direct API access avoids HTTP serialization overhead on every query.
- Memory-mapped index files allow fast startup without warm-up periods.
- The single-process deployment model matches our target use case of local or single-server deployments.

### Trade-offs

- Tantivy has a smaller community than Lucene/Elasticsearch.
- Advanced features like distributed search are not available out of the box.
- Analyzer plugins require Rust development rather than configuration.

## ADR-002: SQLite over RocksDB for Metadata Storage

**Date:** 2025-03-01
**Status:** Accepted

### Context

We needed persistent storage for document metadata, vault configuration, and benchmark results. Both SQLite and RocksDB were evaluated.

### Decision

Use SQLite through the Rusqlite crate for all metadata storage.

### Rationale

- SQLite provides a well-understood relational model for structured metadata.
- Schema migrations are straightforward with SQL DDL.
- Benchmark results benefit from SQL aggregation queries for reporting.
- Rusqlite is mature and well-maintained in the Rust ecosystem.
- Debugging is easier with the SQLite CLI during development.

### Trade-offs

- Write throughput is lower than RocksDB for bulk operations.
- Single-writer limitation is acceptable since our write patterns are bursty during indexing.

## ADR-003: Heading-Based Chunking over Fixed-Size Windows

**Date:** 2025-04-10
**Status:** Accepted

### Context

Documents need to be split into chunks for embedding and search. The two main approaches are fixed-size sliding windows and semantic boundaries.

### Decision

Split documents at Markdown heading boundaries rather than using fixed-size windows.

### Rationale

- Heading-based chunks preserve semantic coherence within each section.
- Search results map directly to meaningful document sections.
- Heading metadata enables hierarchical context in search results (showing the full heading path).
- Users naturally organize documentation under headings, making this boundary reliable.

### Trade-offs

- Chunk sizes vary more widely than fixed windows (100 to 2000 characters).
- Very large sections may need sub-splitting at paragraph boundaries.
- Documents without headings produce a single large chunk.

## ADR-004: Reciprocal Rank Fusion for Hybrid Search

**Date:** 2025-05-20
**Status:** Accepted

### Context

We combine BM25 full-text search and vector similarity search. These two scoring systems produce incomparable raw scores that must be merged into a single ranked list.

### Decision

Use Reciprocal Rank Fusion (RRF) with k=60 to merge the two result sets.

### Rationale

- RRF operates on rank positions rather than raw scores, eliminating the need for score normalization.
- The k parameter provides smooth interpolation between the two rank lists.
- k=60 is a well-established default supported by academic literature.
- Implementation is simple and fast (linear in the number of results).

### Trade-offs

- RRF does not consider score magnitude, which may discard useful signal in some cases.
- The k parameter requires tuning for optimal results on specific corpora.

## ADR-005: Stdio as Default MCP Transport

**Date:** 2025-03-25
**Status:** Accepted

### Context

The MCP protocol supports multiple transports. We needed to choose a default for local agent integrations.

### Decision

Use stdio (standard input/output) as the default transport, with HTTP+SSE as an optional alternative.

### Rationale

- Stdio requires no network configuration or port management.
- AI agent frameworks (Claude Code, Cursor) natively support stdio MCP servers.
- Process lifecycle is managed by the parent agent (automatic start and stop).
- Eliminates firewall and CORS concerns for local deployments.

### Trade-offs

- Stdio limits the server to a single client connection.
- HTTP transport is necessary for multi-client or remote scenarios.
- Debugging stdio communication requires log file inspection rather than network tools.
