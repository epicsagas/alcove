# Changelog

All notable changes to alcove will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.12.1] — 2026-06-11

### Fixed

- Auto-build BM25 index on HTTP server startup when no index exists, preventing `/search` from returning 503 on fresh or post-restart servers

## [0.12.0] — 2026-06-11

### Added

- TurboQuant 4-bit vector index backend (replaces hnsw_rs) (#30)
- Multi document-root support (#27)
- Heading-level markdown chunking (#28)
- PDF form-feed → `--- Page N ---` marker conversion (#31)
- Persistent embedding cache for full rebuilds
- `alcove path` command and doctor path summary (#26)
- Isolated eval corpus with `--corpus` flag for CI/regression testing
- IR metrics: NDCG@K, MAP@K, MRR alongside existing Precision@K and Recall@K
- Chunk-level precision evaluation for heading-based section accuracy
- Regression detection with configurable thresholds (`--baseline`, `--save-baseline`)
- Baseline reference benchmark (`benches/baseline.json`)

### Changed

- Migrated to llm-kernel 0.3.5 `vector-index` feature
- Upgraded llm-kernel from 0.3 to 0.3.5
- Replaced openssl-sys with rustls for all remaining dependencies

### Fixed

- File-level grep matching and deferred ONNX loading
- Ambiguous env detection and tilde expansion in config paths
- Duplicate project-resolution block in MCP handler

## [0.11.7] — 2026-06-07

### Added

- `EMBEDDING_MODELS.md`: curated model reference with RAM estimates, architecture, and performance notes

### Changed

- Embedding layer migrated from candle-transformers to fastembed-rs
- Adopted llm-kernel for model catalog, cache, and metadata re-exports
- Default embedding model changed to `ArcticEmbedXS`; curated supported model list to 6 options
- Removed redundant L2 normalization; embedding state tracks Cached/Uncached

### Fixed

- Port now read from plist in `api env` command instead of config defaults
- Replaced openssl-sys with rustls for fastembed and ureq dependencies
- aarch64 cross-compile and CI check failures resolved
- DirectML ORT symbols re-exported via llm-kernel
- Clippy: `collapsible_if` in launchd module

### CI

- Added aarch64 and DirectML cross-check jobs plus Linux test job

## [0.11.6] — 2026-06-04

### Fixed

- `/index` endpoints made synchronous for consistent client behavior

## [0.11.5] — 2026-06-03

### Fixed

- Restore `set_nonblocking(true)` before `tokio::net::TcpListener::from_std`

## [0.11.4] — 2026-06-03

### Added

- 15 REST API endpoints for skill-driven documentation access
- `alcove api env` subcommand for one-shot URL+token resolution (replaces `api url`)
- Multi doc-root support via `doc_roots` config field
- `validate_project_name` guard on `/projects/{name}/index` endpoint

### Changed

- API: `/rebuild` renamed to `/index`; added `/projects/{name}/index` endpoint
- Skill: migrated from MCP proxy to CLI-based documentation commands
- Install: skip MCP seeding, guide users to `registry/mcp.json`
- Config: `server.port` uses `Option<u16>` for kind-specific defaulting
- Install script consolidated to single file with brew + binstall cascade

### Fixed

- launchd: XML-escape token in plist and use kind-specific port
- CLI `api env`: outputs correct kind-specific default port
- Embedding: BGE-M3 loading, prefix detection efficiency, Arctic model fixes
- Test: corrected install.js extension in hooks test

## [0.11.3] — 2026-05-27

### Fixed

- Embedding: add truncation, fix pooling, model-aware chunking

## [0.11.2] — 2026-05-27

### Added

- Claude Code plugin manifest with MCP configuration (`.claude-plugin/`)

### Changed

- Plugins: dropped Antigravity direct MCP; hooks moved to per-plugin directories
- Plugins: adopted epic-harness cross-platform architecture

### Fixed

- Plugin hooks path corrected to `.claude-plugin/hooks.json`
- Antigravity: uses Node.js install script matching Claude Code pattern

## [0.11.1] — 2026-05-26

### Added

- Antigravity plugin package for agy ecosystem

### Fixed

- `vector_index` now inherits `embedding.enabled` when unset

## [0.11.0] — 2026-05-26

### Added

- Snowflake Arctic Embed models (XS/S/M/L) support
- New logo image

### Changed

- BGE-M3 prefix detection refactored for efficiency and readability
- Antigravity removed from supported agents; Codex plugin description enhanced
- Setup callout reorganized to Quick Start section

### Fixed

- BGE-M3 loading fixed; XLM-RoBERTa model loading improved

## [0.10.4] — 2026-05-25

### Fixed

- Setup wizard embedding model list now matches actual supported models
- Removed SnowflakeArcticEmbedXS/Q/S from setup (not in EmbeddingModelChoice)
- Added AllMiniLML6V2, MultilingualE5Large, BGEM3 to setup wizard

## [0.10.3] — 2026-05-25

### Changed

- Repositioned from "documentation server" to "memory server" across all plugin manifests and descriptions
- Claude Code and Codex removed from `register`/`setup` paths — plugin install only (`/plugin install`, `codex plugin install`)
- Added `skills`, `hooks`, `mcpServers` fields to both `.claude-plugin/plugin.json` and `.codex-plugin/plugin.json`
- Updated setup step 6 description in README and all 9 translations

## [0.10.2] — 2026-05-24

### Fixed

- MCP restart now uses `unload`/`load` instead of `stop`/`start` to ensure port is fully released before restart
- Set `SO_REUSEADDR` on HTTP server socket to allow rebinding during `TIME_WAIT`
- Increased startup polling timeout to handle slow tantivy index initialization

## [0.10.1] — 2026-05-24

### Fixed

- Plugin skill tool table synced with actual MCP tools (added `list_vaults`, `search_vault`, `backup_vault`; removed nonexistent `index_code_structure`)

## [0.10.0] — 2026-05-24

### Added

- Code structure indexing with 12 languages (Rust, TypeScript, Python, Go, Java, Kotlin, C, C++, Swift, Ruby, C#, JavaScript)
- Shorthand language names support (cpp, csharp, ts, js)
- XLM-RoBERTa embedding support for BGE-M3 with PyTorch fallback and `ALCOVE_EMBED_PROVIDER` env var override

### Changed

- Code indexing now indexes all available languages by default instead of auto-detecting
- Removed Gemini CLI agent and Antigravity MCP path references
- Removed Codex marketplace plugin definition
- Synced plugin skill tool table with actual MCP tools (added `list_vaults`, `search_vault`, `backup_vault`; removed nonexistent `index_code_structure`)

## [0.9.1] — 2026-05-21

### Changed

- Removed code-index feature flag — tree-sitter is now included in core builds
- Updated CI actions to latest stable versions

## [0.9.0] — 2026-05-19

### Changed

- Tracked Cargo.lock in git for reproducible builds
- Updated macOS CI runner and synced dependencies

## [0.8.12] — 2026-05-19

### Changed

- Applied cargo fmt to code-index module

## [0.8.11] — 2026-05-19

### Fixed

- Switched llm-transpile from git dependency to crates.io

## [0.8.10] — 2026-05-19

### Added

- Tree-sitter based code structure indexing for 12 languages
- llm-transpile post-processing layer for search result optimization
- Heading chunking, frontmatter exclusion, and parent-child search
- Linux and Windows one-line installers in README
- Codex plugin manifest with marketplace support
- Plugin manifest now declares MCP + skill fields

### Changed

- Reframed README introduction around 2026 agent pain points
- Upgraded badges to for-the-badge style with GitHub stats row
- Compressed SKILL.md from 275 to 88 lines for token efficiency
- Replaced fastembed with candle-transformers for cross-platform builds (#6)

## [0.8.9] — 2026-05-12

### Added

- Binary downloader install script replacing source-build, with Windows support
- Auto-update binary when plugin version is newer via SessionStart hook

### Fixed

- Removed duplicate hooks field and trailing comma in plugin.json
- Corrected archive filename and extraction path in install script
- Removed extra-artifacts from cargo-dist config

## [0.8.8] — 2026-05-12

### Added

- `register` subcommand for non-interactive MCP seeding
- Cross-platform Node.js installer replacing bash bootstrap in plugin hooks

### Fixed

- Added hooks path to plugin manifest
- Aligned binstall metadata with cargo-dist 0.31.0 tar.xz output

### Changed

- Added marketplace add step and clarified setup requirement in README

## [0.8.7] — 2026-05-11

### Fixed

- Restricted release targets to macOS + x86_64-linux (ort-sys/ONNX limitation)
- Release pipeline: mapped Apple signing secrets, disabled pending codesign/notarize
- Tests use home-based tempdir to avoid /tmp system path restriction

### Changed

- Migrated to dist-workspace.toml with macOS signing configuration
- Unified CI workflow — check/test/audit/sbom jobs

## [0.8.6] — 2026-05-10

### Added

- `backup_vault` MCP tool and CLI command
- `alcove doctor` — MCP and API service status checks
- Server lifecycle restructured into `mcp` and `api` subcommands
- Launchd process lifecycle: enable/disable/start/stop/restart

### Changed

- Restructured as Cargo workspace
- Updated CLI commands and obsidian-forge integration guide across all READMEs

## [0.8.5] — 2026-05-07

### Added

- `init_project` now generates GitHub community standard files

### Fixed

- Resolved all clippy warnings and unused-import warnings

## [0.8.4] — 2026-05-06

### Added

- Search benchmark framework for performance and quality measurement
- Multi-field boosting, title indexing, and project diversity in search

### Changed

- Decomposed index.rs into 7 submodules with IndexSchema struct
- Split cli.rs into agents/setup/commands submodules
- FileReader trait registry for extensible document parsing

## [0.8.3] — 2026-04-30

### Added

- Per-agent env var syntax, token wizard, and shell rc seeding in setup

### Fixed

- Added type field to agent configs, support env var interpolation
- Changed default port from 8080 to 57384

## [0.8.2] — 2026-04-30

### Added

- PostHog + Sentry telemetry with opt-out consent
- Telemetry events across MCP, serve, and setup commands

### Fixed

- Guarded unix-only `reap` command with cfg(unix) for Windows build
- Plugged promote_document telemetry gap

### Changed

- Reduced parallel jobs and test threads to cap memory usage
- Hoisted load_config out of project list loop

## [0.8.1] — 2026-04-29

### Added

- `reap` command and SessionEnd hook to clean orphaned processes
- Vault-level embedding config with hybrid vector search

## [0.8.0] — 2026-04-17

### Fixed

- Patched rand unsound vulnerability (0.10.0→0.10.1, 0.9.2→0.9.4)

### Changed

- Allowed dirty Cargo.lock during cargo publish for release workflow

## [0.7.12] — 2026-04-16

### Added

- Multi-vault knowledge base support with isolated caches and HTTP API
- Vault CLI commands: list, search, backup via MCP tools
- Hybrid MCP proxy mode with Claude plugin support
- Launchd process lifecycle: enable/disable/start/stop/restart
- `alcove lint` and `alcove promote` CLI commands
- HTTP RAG server mode (alcove-server feature)
- HNSW indexing for large-scale vector search
- Hybrid search with BM25 + vector RRF fusion
- Embedding module with lazy model download
- Multi-format document parsing (PDF, code, etc.)
- Per-IP rate limiting
- Bearer token auth and localhost-only CORS
- Query embedding cache with doctor diagnostics
- Memory budget management and security hardening

### Fixed

- ABBA deadlock, SSRF whitelist, /mcp rate limit, and correctness patches
- Path traversal and file size DoS in MCP tools
- Symlink cycle infinite loop in vault file counting
- TOML injection vulnerabilities in config
- Security: blocked ALCOVE_HOME from system-sensitive directories
- Cross-compilation failures for Linux musl and Darwin x86_64

### Changed

- Unified alcove home to `~/.alcove` (migrated from `~/.config/alcove`)
- Embedding cache moved to `~/.alcove/models`
- Cached IndexReader, eliminated double index open
- Bounded embed memory with file-batch streaming (32 files at a time)
- Background rebuild_index returns immediately
- Reduced function arguments via FullConfigParams struct

## [0.7.11] — 2026-04-07

### Added

- Glama MCP score badge and glama.json configuration
- Behavioral transparency and usage guidelines in MCP tool descriptions

### Changed

- Exposed alcove as a library crate

## [0.7.10] — 2026-03-25

### Changed

- Bumped lz4_flex dependency (dependabot)

## [0.7.9] — 2026-03-12

### Added

- Project-level configuration via `alcove.toml`
- Configurable additional file extensions in search index

## [0.7.8] — 2026-03-11

### Added

- CJK tokenizer support for improved search reliability

### Changed

- Reduced core complexity, improved type safety, added stale lock detection
- Added Smithery registry listing (smithery.yaml, server-card.json)

## [0.7.7] — 2026-03-10

### Changed

- Enhanced README with demo content and improved clarity
- Updated SKILL documentation

## [0.7.6] — 2026-03-09

### Added

- Homebrew and cargo-binstall install instructions

### Fixed

- Brew install command now uses fully qualified tap path

## [0.7.4] — 2026-03-09

### Changed

- Updated install instructions for cargo-binstall support

## [0.7.3] — 2026-03-09

### Added

- `alcove doctor` command and `check_doc_changes` MCP tool
- Multi-platform release workflow and cargo-binstall support

### Fixed

- Spanish and Portuguese translation fixes

## [0.7.1] — 2026-03-08

### Fixed

- Agent skill: enforce current-project-only scope by default; agents must ask the user before scanning all projects on ambiguous requests

## [0.7.0] — 2026-03-08

### Added

- BM25 ranked search powered by tantivy with auto-indexing
- `alcove search` CLI command with `--scope global` and `--mode` options
- `alcove index` CLI command to build/rebuild search index
- `rebuild_index` MCP tool for AI agent integration
- Cross-project global search via `scope: "global"`
- Copilot CLI as 10th supported agent (`~/.copilot/mcp-config.json`)
- Skill installation for Cline (`~/.cline/skills/alcove`) and Codex CLI (`~/.codex/skills/alcove`)
- Auto-create `docs_root` directory at `~/.config/alcove/docs` on first setup
- Incremental index rebuild — skips unchanged files based on mtime

### Fixed

- Antigravity MCP config path: `~/.antigravity/settings.json` → `~/.gemini/antigravity/mcp_config.json`
- Cline and Codex CLI skill directories were not installed during setup
- Uninstall now cleans Cline, Codex, and Copilot skill directories

### Changed

- Bump dependencies: console 0.16, dialoguer 0.12, thiserror 2
- Simplify README mermaid diagram (collapse agents/tools, remove Index subgraph)
- Clarify README wording: "read" → "read and manage", "read-only" → "scoped access"
- Increase test coverage from 152 to 216 tests
- Apply clippy suggestions for idiomatic Rust

## [0.6.0] — 2026-03-07

### Added

- `alcove validate` CLI command — validate docs against policy.toml
- `validate_docs` MCP tool for AI agent integration
- policy.toml support with project > team > default priority resolution
- Required file validation with alias support
- Section heading validation with min_items check
- `--format json` and `--exit-code` flags for CI/CD integration
- Integration tests with tempfile for all tool functions
- MCP dispatch routing tests with schema validation

### Changed

- Modularized codebase: decomposed main.rs into config, mcp, tools modules
- Increased test coverage from 22 to 85 unit tests

## [0.5.0] — 2026-03-07

### Changed

- Upgraded to Rust Edition 2024
- Bumped version for crates.io release

### Added

- i18n support for 10 languages (en, ko, ja, zh-CN, es, hi, pt-BR, de, fr, ru)
- Translated README files in docs/ folder
- ALCOVE_LANG env var for explicit locale override

## [0.4.0] — 2026-03-06

### Changed

- Moved translated READMEs from root to docs/ folder

### Added

- Additional translated READMEs (hi, pt-BR, de, fr, ru)

## [0.3.0] — 2026-03-06

### Changed

- Renamed project from `docs-bridge` to `alcove`
- Consolidated CLI: `alcove setup` handles all configuration (docs root, categories, diagram format, agents)
- Removed `skill`/`mcp`/`serve` subcommands — `setup` covers everything
- Setup now shows existing values as defaults, making reconfiguration easy
- Interactive document category selection with pre-checked existing config
- Simplified `install.sh` and `Makefile` to focus on binary install + setup delegation

### Added

- `dialoguer`-based TUI for all interactive prompts (replaces Python scripts)
- `clap` CLI with `setup` and `uninstall` subcommands
- `include_str!` embedded SKILL.md in binary — no external file dependency
- crates.io publishing metadata

## [0.2.0]

### Added

- Bidirectional document flow (docs-bridge ↔ project repo) with transformation rules
- Cross-repo audit: detect exposed internal docs, misplaced reports, missing public docs
- Document classification: `doc-repo-required`, `doc-repo-supplementary`, `project-repo`
- Config consolidation to `config.toml` with `docs_root`

## [0.1.0]

### Added

- Initial MCP server with stdio JSON-RPC 2.0
- Tools: overview, search, get_file, list_projects, audit, init
- Auto-detection of active project from CWD
- Support for 8 AI agents (Claude Code, Cursor, Claude Desktop, Cline, OpenCode, Codex, Antigravity, Gemini CLI)
