---
name: alcove
description: >
  Grounds the agent in authoritative internal project documentation stored in a private Alcove docs repository.
  Covers project design, architecture, requirements, progress tracking, coding conventions,
  technical debt, secrets mapping, and environment configuration.
  Also initializes, organizes, audits, and validates project documentation.
  Activates whenever the agent needs authoritative project information — regardless of input language.
---

# Alcove

## Invocation

This skill can be called explicitly via slash command in supported agents:

```
/alcove                          Summarize current project docs and status
/alcove status                   Show current progress and next steps
/alcove architecture             Explain the tech stack and system design
/alcove conventions              List coding rules and naming conventions
/alcove decisions                Review architecture decision records
/alcove debt                     List known issues and technical debt
/alcove search auth flow         Search docs for a specific topic
/alcove what conventions apply?  Ask a doc question directly
```

It also activates automatically when the agent needs authoritative project context — see "When to Use" below.

## When to Use

- User asks about **how this project is designed, architected, or specified**
- User asks about **project status, progress, or next steps**
- User asks about **coding conventions, naming rules, or forbidden patterns**
- User asks about **environment variables, secrets, or deployment config**
- User asks about **technical debt, known issues, or workarounds**
- User asks about **past decisions and their rationale**
- User wants to **initialize documentation for a new project**
- User asks to **organize, clean up, or audit project documentation**
- **The answer may exist in project docs** — check alcove before answering, not after

## How It Works

Uses MCP server `alcove` via stdio. The server auto-detects the active project by matching CWD path components against folders in `DOCS_ROOT`. No per-project configuration needed — one global install covers all projects.

## Document Structure

Each project in alcove follows this standard:

### Doc-repo Required (always present)

| File | Contains |
|------|----------|
| `PRD.md` | Requirements, goals, scope, constraints |
| `ARCHITECTURE.md` | Tech stack, directory structure, data model, API design, security |
| `PROGRESS.md` | Current phase, milestones, blockers, next actions |
| `DECISIONS.md` | Architecture Decision Records (ADR) with rationale |
| `CONVENTIONS.md` | Naming, patterns, import order, forbidden practices |
| `SECRETS_MAP.md` | Environment variable names and rotation policy (never values) |
| `DEBT.md` | Technical debt, known vulnerabilities, workarounds |

### Doc-repo Supplementary (project-specific)

| File | When Present |
|------|-------------|
| `DEPLOYMENT.md` | Service has infra/CI/CD pipeline |
| `INTEGRATION.md` | 2+ external service connections |
| `reports/*.md` | Audits, benchmarks, competitive analyses |

## Available Tools

### `get_project_docs_overview`

List all docs with tier classification. **Call this first** to see what's available.

### `search_project_docs`

Search across project docs with automatic mode selection — uses BM25 ranked search when the index is available, falls back to grep otherwise. No manual mode selection needed.

**Parameters:**
- `query` (required) — search keyword or phrase
- `scope` (optional) — `"project"` (default, CWD only) or `"global"` (all projects)
- `limit` (optional) — max results (default: 20)

**Scope rule — IMPORTANT:**
- **Default is ALWAYS current project only.** Do NOT scan all projects unless the user explicitly requests it.
- If the request is ambiguous, ask the user first: "Current project only, or all projects?"
- Ambiguous phrases that do NOT imply global scope (treat as current project, or ask):
  - "docs repo", "documentation", "check the docs", "review docs"
  - "remaining items", "what's missing", "status check", "doc health"
  - "look through everything", "go over everything", "summarize docs"
  - "clean up docs", "organize docs", "doc audit"
- Never assume global scope from vague references to a "docs repo" or "documentation".

**When to use global scope (explicit signals only):**
- User says "all projects", "everywhere", "across projects"
- User references previously saved notes, knowledge, or past decisions
- User wants to compare how different projects handle the same topic

### `get_doc_file`

Read a specific file. Common patterns:
- `get_doc_file("PRD.md")` — understand what we're building
- `get_doc_file("ARCHITECTURE.md")` — understand how to build it
- `get_doc_file("PROGRESS.md")` — understand current status
- `get_doc_file("CONVENTIONS.md")` — understand coding rules before writing code
- `get_doc_file("DECISIONS.md")` — check existing decisions before proposing changes
- `get_doc_file("DEBT.md")` — check known issues before investigating bugs

### `list_projects`

List all projects in alcove. Shows required doc completeness per project.

### `audit_project`

Audit docs across both alcove and the project repository. Returns:
- Doc-repo required file status: `populated`, `missing`, `template-unfilled`, `minimal`
- Cross-repo analysis: exposed internal docs, misplaced reports, missing public docs
- Structured `suggested_actions` with mandatory rules in `agent_instruction`

Use to organize documentation or before `init_project` to understand gaps.

### `check_doc_changes`

Detect document changes since the last index build. Reports added, modified, and deleted files. Useful for keeping the search index fresh and monitoring documentation drift.

**Parameters:**
- `auto_rebuild` (optional) — if `true` and changes are detected, automatically rebuilds the index (default: false)

**Returns:** `index_exists`, `is_stale`, `added`, `modified`, `deleted`, `unchanged_count`, `total_indexed`

### `rebuild_index`

Build or rebuild the BM25 full-text search index. Enables ranked search results in `search_project_docs`. Index is automatically built after `init_project`, but run this manually after bulk document changes.

### `validate_docs`

Validate project docs against team policy (`policy.toml`). Checks required files exist, templates are filled, required sections are present. Returns pass/warn/fail status per file.

### `init_project`

Initialize docs for a new project from the standard template. Automatically rebuilds the search index.

**Arguments:**
- `project_name` (required) — folder name in alcove
- `project_path` (optional) — absolute path to project repo for public docs (README, CHANGELOG)
- `overwrite` (optional) — overwrite existing files (default: false)
- `files` (optional) — specific files to create (e.g. `["PRD.md", "ARCHITECTURE.md"]`); if omitted, creates all required internal docs

## Agent Instructions

### Scope principle

**Always scope to the current project unless the user explicitly says otherwise.**
- Phrases like "check docs", "remaining items", "doc status", "clean up", "audit" → current project only.
- If the intent is ambiguous between current project and all projects, **ask the user** before proceeding.
- Only use global scope or scan multiple projects when the user explicitly names them or uses keywords like "all projects", "across projects", "everywhere".

### Answering project questions

**Never answer architecture, conventions, or environment questions from memory.** If the information may exist in docs, check alcove first — always ground answers in authoritative sources.

1. Call `get_project_docs_overview` to see available docs and their tiers.
2. Based on the question, read the most relevant file:
   - "What does this do?" → `PRD.md`
   - "How is this built?" → `ARCHITECTURE.md`
   - "What's the status?" → `PROGRESS.md`
   - "Why was X chosen?" → `DECISIONS.md`
   - "What style to use?" → `CONVENTIONS.md`
   - "What env vars needed?" → `SECRETS_MAP.md`
   - "Any known issues?" → `DEBT.md`
3. If unsure which file, use `search_project_docs` with keywords.
4. Summarize key decisions, constraints, and implications, citing relevant sections. Do not dump full files unless explicitly requested.
5. **Never contradict existing decisions** — if DECISIONS.md says "use JWT", don't suggest sessions.

### Initializing a new project

1. Call `init_project` with the project name and optionally the project repo path.
2. Inform the user which files were created.
3. Suggest they start by filling in PRD.md and ARCHITECTURE.md.

### Organizing project documentation

When the user asks to organize, clean up, or audit documentation:

1. Call `audit_project` — this scans both alcove and the project repository.
2. Present the findings to the user. Do NOT auto-execute any actions.
3. Follow the `suggested_actions` with these **mandatory rules**:

#### Document separation rules

| Direction | Allowed | Example |
|-----------|---------|---------|
| alcove → project repo | Generate **public-facing** docs derived from internal content | Generate README from PRD |
| project repo → alcove | Restructure/incorporate reference materials into internal docs | Analyze API spec → enhance ARCHITECTURE.md |
| Raw internal → project repo | **NEVER** | Never copy PRD.md into the project repo |

#### Action handling

- **`resolve_exposed_internal_docs`**: If internal docs (PRD, ARCHITECTURE, etc.) exist in the project repo:
  1. Diff against the alcove version
  2. Merge any additional content from the project repo version into alcove first
  3. Remove from the project repo only after user confirmation

- **`move_reports_to_doc_repo`**: Move analysis/benchmark/audit reports to alcove `reports/`.

- **`incorporate_to_doc_repo`**: Restructure project repo reference materials into alcove internal docs.

- **`generate_public_docs`**: Generate missing public docs from internal docs. Never expose internal information.

- **`create_missing_internal`**: Create missing required internal docs via `init_project`.

4. **Always confirm with the user** before moving or deleting any file.
5. Re-run `audit_project` after cleanup to verify results.

### Before writing code

Always check `CONVENTIONS.md` first to ensure generated code follows project-specific rules (naming, error handling, import order, forbidden patterns).
