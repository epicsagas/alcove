# eval-project Code Structure Index

## Source Code Parsing Pipeline

eval-project uses tree-sitter to parse source code files into a searchable AST-based index. This enables structural queries like "find all public functions that return a Result type" alongside traditional text search.

## Tree-Sitter Integration

### Supported Languages

The code indexer includes tree-sitter grammars for:

- **Rust** -- Functions, structs, enums, trait implementations, module declarations.
- **TypeScript** -- Functions, classes, interfaces, type aliases, exports.
- **Python** -- Functions, classes, decorators, module-level constants.
- **Go** -- Functions, structs, interfaces, package declarations.

### Parsing Process

1. The file watcher detects source code files based on extension mapping (`.rs`, `.ts`, `.py`, `.go`).
2. The tree-sitter parser produces a concrete syntax tree (CST).
3. A language-specific visitor walks the CST and extracts named nodes.
4. Extracted nodes are converted to index entries with type, name, location, and signature metadata.

### Index Entry Schema

Each code structure entry contains:

- `symbol` -- The declared name (e.g., `SearchResult`, `build_index`).
- `kind` -- The syntactic category (function, struct, enum, trait, impl, method).
- `visibility` -- Access level (public, private, crate-local).
- `signature` -- Full declaration text up to the opening brace.
- `location` -- File path, line number, and byte offset.
- `doc_comment` -- Attached documentation comment text, if present.

## AST-Based Chunking

Source code files are chunked differently from Markdown documents. Instead of heading boundaries, the chunker uses syntactic boundaries:

- Each top-level declaration (function, struct, enum) becomes a separate chunk.
- Nested items (methods within impl blocks) are grouped under their parent.
- Import sections and module-level constants form header chunks.
- Comments attached to declarations are included in the chunk.

This approach preserves code context better than line-based splitting.

## Search Integration

### Symbol Search

The `search` MCP tool recognizes structural queries through pattern matching:

- Queries containing `fn`, `struct`, `enum`, `trait` keywords trigger symbol search mode.
- Symbol results include the full signature and surrounding context.
- Results are ranked by name similarity and relevance to the surrounding documentation.

### Cross-Reference Tracking

The indexer tracks import and usage relationships:

- Module imports map to the source files they reference.
- Type annotations link declarations to their definitions.
- Function calls create edges between caller and callee.

These relationships enable "find all callers of `build_index`" queries that traverse the reference graph.

## Performance Considerations

- Tree-sitter parsing adds approximately 2 ms per file on average for Rust source files.
- The code index adds roughly 20% overhead to total index size compared to text-only indexing.
- Incremental re-indexing only reparses changed files, keeping update times fast.
- The AST cache stores parsed trees in memory during indexing to avoid redundant parsing.

## Configuration

Enable code indexing in `eval-project.toml`:

```toml
[code_index]
enabled = true
languages = ["rust", "typescript", "python", "go"]
chunk_nested = true
include_doc_comments = true
```
