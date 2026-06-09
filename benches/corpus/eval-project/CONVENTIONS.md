# eval-project Coding Conventions

## Rust Style Guide

This project follows the standard Rust API guidelines with project-specific additions.

### Module Organization

- One public type per file when the type has non-trivial implementation.
- Module `mod.rs` files re-export public items and contain only documentation and imports.
- Test modules live in a `tests` subdirectory adjacent to the source file, gated by `#[cfg(test)]`.

### Error Handling

- Use `anyhow::Result` for application-level functions where the caller does not need to match specific error variants.
- Use `thiserror` for library-level error types where callers need programmatic error handling.
- Never panic in library code. Use `Result` for all fallible operations.
- Wrap external error types with context using `anyhow::Context` or custom error variants.

```rust
// Preferred: contextual error chain
let index = tantivy::Index::open_in_dir(&index_path)
    .context(format!("Failed to open index at {}", index_path.display()))?;
```

### Naming Conventions

- Types: `UpperCamelCase` (e.g., `SearchResult`, `ChunkMetadata`).
- Functions and methods: `snake_case` (e.g., `build_index`, `parse_heading`).
- Constants: `SCREAMING_SNAKE_CASE` (e.g., `DEFAULT_RRF_K`, `MAX_CHUNK_SIZE`).
- Crate names: `kebab-case` (e.g., `eval-project`).

### Async Patterns

- Use `tokio` as the async runtime exclusively.
- Prefer `tokio::spawn` for fire-and-forget tasks (file watcher, periodic commits).
- Use `tokio::join!` for concurrent operations that all must succeed.
- Avoid `async fn` in trait implementations; use the `async-trait` crate when necessary.

## Commit Message Format

We follow Conventional Commits:

```
type(scope): description

[optional body]
```

Common types: `feat`, `fix`, `refactor`, `docs`, `test`, `perf`, `chore`.
Scopes: `search`, `index`, `ingest`, `mcp`, `cli`, `config`, `bench`.

Examples:
- `feat(search): add RRF result merging for hybrid queries`
- `fix(index): handle empty heading chunks gracefully`
- `perf(ingest): batch file watcher events with 500ms debounce`

## Testing Standards

- Unit tests cover individual functions and methods.
- Integration tests exercise the full pipeline from file ingestion to search query.
- Benchmark tests use the `criterion` crate and live in `benches/`.
- Test data uses synthetic Markdown files in `fixtures/` -- never real project documentation.
- All tests must pass with `--test-threads=1` for deterministic file system state.

## Documentation

- Public API items must have doc comments with examples.
- Module-level documentation goes in the module file, not a separate README.
- Architecture documentation lives in `ARCHITECTURE.md` at the project root.
- Decision records go in `DECISIONS.md` following the ADR format.

## Logging

- Use the `tracing` crate for all structured logging.
- Log levels: `error` for failures, `warn` for degraded states, `info` for lifecycle events, `debug` for operational details, `trace` for verbose output.
- Never log document content or search queries at `info` level or above.
- Use span attributes for request tracing: project name, query ID, operation type.
