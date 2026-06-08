# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository Purpose

This repository serves **three roles**:

1. **Obsidian vault (Second Brain)** — ZK + PARA + LYT hybrid knowledge management system
2. **Alcove docs root** — private project documentation served to AI agents via the `alcove` MCP server
3. **Git repository** — versioned with Conventional Commits, remote: `git@github.com:epicsagas/alcove-docs.git`

Documents here must **never** be copied into public project repos.

## Actual Location

`$VAULT_PATH` — accessed by both obsidian-forge and alcove MCP server.

## Structure

```
├── 00-Inbox/               # Capture zone (obsidian-forge AI classification target)
├── 01-Projects/            # Project hub MOC (Dataview queries, no project folders here)
├── 02-Areas/               # Ongoing interest areas (Rust, MCP, AI, Fintech, DevOps, OSS)
├── 03-Resources/           # Reference materials (AWS, Laws-Of-Software-Engineering, etc.)
├── 10-Zettelkasten/        # Permanent concept notes (layer/wiki tagged, 300+ chars)
├── 99-Archives/
│   └── projects/           # ALL project doc folders live here
│       ├── alcove/         # alcove MCP server project docs
│       ├── obsidian-forge/ # obsidian-forge daemon project docs
│       ├── collet/         # ... and all other active/inactive projects
│       └── ...
├── _template/              # Template for new project docs
├── Home.md                 # Vault dashboard / root MOC
├── TAGGING.md              # Tagging and frontmatter conventions
└── vault.toml              # Per-vault obsidian-forge config
```

**Project doc folders live in `99-Archives/projects/`** — not at the vault root. Do not move them to `01-Projects/`.

## Karpathy 3-Layer Architecture

| Layer | Location | Content | Automation |
|-------|----------|---------|------------|
| **Raw** | `99-Archives/projects/`, `00-Inbox/`, `02-Areas/` | Project docs, captured notes, area notes | `of process-all`, git sync |
| **Wiki** | `10-Zettelkasten/` | Refined atomic concept notes (300+ chars) | Manual curation + `of strengthen-graph` |
| **Graph** | Wikilinks across all layers | Connections between Raw and Wiki | `of strengthen-graph` |

### Layer Rules
- Raw layer is the source of truth for project state
- Wiki layer: 300+ chars (excluding frontmatter), tagged `layer/wiki`, must link to ≥1 Project/Area
- Graph layer is auto-generated — do not manually edit bridge notes
- Content flows Raw → Wiki (manual distillation) → Graph (automation)

### Tags
- **Hierarchy:** Use `topics/<name>`, `status/<name>`, `type/<name>` (e.g., `topics/rust`, `status/evergreen`)
- **Layers:** `layer/raw`, `layer/wiki` (mandatory)
- **Limits:** Max 7 tags per file, project tag first, case-sensitive
- See `TAGGING.md` for the full hierarchical schema and frontmatter rules.

## Project Doc Schema

| File | Purpose |
|------|---------|
| `PRD.md` | Product requirements and goals |
| `ARCHITECTURE.md` | Tech stack, module structure, data flow |
| `PROGRESS.md` | Release history, current milestones, blockers |
| `DECISIONS.md` | Architecture Decision Records (ADRs) |
| `CONVENTIONS.md` | Naming, patterns, forbidden practices |
| `SECRETS_MAP.md` | Env var names and rotation policy (never values) |
| `DEBT.md` | Technical debt and known workarounds |

Supplementary folders: `reports/`, `specs/`, `plans/`, `research/`, `archive/`, `strategy/`

## Knowledge Governance

The vault's health is maintained via automated policies.

- **Linter:** `$HARNESS_DIR/scripts/ontology_lint.py` — enforces character counts and mandatory metadata.
- **Graph Strengthening:** `of strengthen-graph` — auto-discovers links between silos.
- **Compliance:** All new notes must adhere to the **3-Layer Architecture** and hierarchical tagging.

## Obsidian Forge (`of`)

Rust CLI daemon for vault lifecycle management.

- **Binary**: `~/.cargo/bin/of`
- **Global config**: `~/.obsidian-forge/config.toml` (AI provider, daemon settings, vault registry)
- **Per-vault config**: `vault.toml` in vault root
- **API keys**: `~/.obsidian-forge/.env` (never in config files)

### CLI Commands

```bash
of update-mocs --vault SecondBrain      # Regenerate project hub MOCs
of strengthen-graph --vault SecondBrain  # Graph strengthening pipeline
of process-all --vault SecondBrain       # Process inbox with AI classification
of graph health                          # Graph health report
of sync --vault SecondBrain              # Full sync: MOC → graph → git commit
of vault list                            # List registered vaults
```

## Alcove MCP Server

alcove serves these docs to AI agents over stdio JSON-RPC 2.0. The BM25 search index (tantivy) lives in `.alcove/` (gitignored, machine-local).

- **Change detection**: mtime + file size fingerprint
- **CJK support**: NgramTokenizer (min=2, max=3) for Korean/Japanese/Chinese
- **Auto-rebuild**: `search_project_docs` triggers rebuild if index is stale
- **Manual rebuild**: `alcove index` CLI or `rebuild_index` MCP tool

## Search Benchmark

alcove includes a built-in search quality benchmark framework for measuring and tracking retrieval performance.

- **Ground truth**: `benches/ground_truth.toml` — 50 queries across 10 categories (architecture, product, cross-project, CJK, code, config, error-messages, navigation, edge-case, negative-control)
- **Metrics**: Precision@K, Recall@K, NDCG@K, MAP@K, MRR, chunk-level accuracy
- **Regression detection**: `--baseline` compares current results against a saved baseline JSON
- **Baseline**: `benches/baseline.json` — reference benchmark at v0.11.7

```bash
# Run precision benchmark
alcove bench --metrics precision

# Save current results as baseline
alcove bench --output json --save-baseline benches/baseline.json

# Compare against baseline (CI-friendly)
alcove bench --baseline benches/baseline.json

# Full output with markdown report
alcove bench --metrics precision --output markdown --output-file report.md
```

**Regression thresholds**: precision drop >5%, latency increase >20%, throughput drop >15%. Changes within half the threshold trigger a Warn status.

## Working With Docs

- Add new project docs: copy `_template/` into a new subfolder under `99-Archives/projects/`
- After bulk doc changes: run `alcove index` to rebuild the BM25 search index
- Check doc health: `alcove validate` or `of graph health`
- Run linter: `python3 $HARNESS_DIR/scripts/ontology_lint.py` (mandatory before ship)
- Diagrams: Mermaid format only (ASCII art forbidden)
- Git commits: use `git-cc` skill — format: `type(scope): description` (no emoji, no co-authorship footer)

## Document Authoring Rules

- All internal docs stay in this repo — never commit to public project repos
- When updating ARCHITECTURE.md or DECISIONS.md, add a `> 최종 업데이트: YYYY-MM-DD` line
- PROGRESS.md tracks release history as a table; add rows for each release
- DEBT.md entries include ID prefix (e.g., `D-006`) for cross-reference in CONVENTIONS.md
- Tagging: follow `TAGGING.md` — max 7 tags, project tag first, use `topics/`, `status/`, `type/` hierarchy
- Wikilinks `[[]]` for vault-internal references, markdown links `[]()` for same-project links
