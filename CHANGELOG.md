# Changelog

All notable changes to alcove will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
