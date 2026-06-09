# eval-project Product Requirements Document

## Overview

eval-project provides a self-hosted documentation search server that teams can deploy alongside their existing project documentation. The server indexes Markdown files from configured vault directories and exposes a search API through the MCP protocol.

## Target Users

- Development teams maintaining multiple project documentation vaults.
- AI agent frameworks that need real-time access to project context.
- Individual developers who want fast local search across their notes and documentation.

## Core Features

### Interactive Setup Command

The `eval-project init` command walks users through initial configuration:

1. Select vault directories to index.
2. Choose an embedding model (remote API or local Ollama).
3. Configure index storage location.
4. Set up MCP transport (stdio or HTTP).

The setup command generates a `eval-project.toml` configuration file in the current directory.

### Global Search

Users can search across all configured project vaults from a single query. Results are ranked by relevance using hybrid BM25 and vector similarity. Each result includes the document title, heading context, a snippet of matching text, and the source project name.

### Cross-Project Documentation

Multiple vaults can be registered as separate projects. Each project maintains its own index segment but shares the same embedding model and search infrastructure. This allows queries to span projects or target a specific vault.

### Embedding Model Support

The server supports multiple embedding backends:

- OpenAI `text-embedding-3-small` (default, 1536 dimensions).
- OpenAI `text-embedding-3-large` (3072 dimensions, higher quality).
- Local models via Ollama (e.g., `nomic-embed-text`, `mxbai-embed-large`).

Model selection is configured per-instance and applies to all projects. Switching models requires a full re-index.

### Benchmark Quality Commands

The `eval-project bench` subcommand provides tools for measuring search quality:

- `bench run` -- Execute a benchmark suite against the current index.
- `bench compare` -- Compare results between two benchmark runs.
- `bench report` -- Generate a human-readable quality report.

Benchmark suites are defined in TOML files that specify query sets and expected relevant documents.

## Vault Management

### Vault Registration

Vaults are registered in the configuration file with a project name, path, and optional file filter patterns:

```toml
[[projects]]
name = "my-api"
path = "/docs/my-api"
include = ["*.md", "*.mdx"]
exclude = ["drafts/**"]
```

### Incremental Indexing

The file watcher monitors vault directories for changes and performs incremental re-indexing. Only modified files are re-parsed and re-indexed. Deleted files are removed from the index on the next commit cycle.

### Manual Re-index

The `rebuild_index` MCP tool or the `eval-project reindex` CLI command triggers a full re-index. This is necessary after changing the embedding model configuration.

## Non-Functional Requirements

- Query latency must remain under 100 ms for corpora up to 50,000 documents.
- Index startup time must be under 5 seconds for typical corpora.
- Memory usage must stay under 500 MB for the default configuration.
- The server must gracefully handle malformed Markdown files without crashing.

## Future Considerations

- PDF and plain text file support beyond Markdown.
- Multi-user authentication for HTTP transport deployments.
- Federated search across multiple eval-project instances.
- Real-time index replication for high-availability setups.
