---
name: alcove
description: >
  Access private project documentation stored in Alcove.
  Use when the user asks about project design, architecture, requirements,
  progress, conventions, technical debt, or environment configuration.
  Also use to initialize docs for a new project.
  Also use when the user asks to organize, clean up, or audit project documentation.
triggers:
  - project design
  - architecture
  - requirements
  - progress
  - conventions
  - technical debt
  - environment config
  - initialize docs
  - organize docs
  - clean up docs
  - audit docs
  - document cleanup
  - search all projects
  - find across projects
  - knowledge base
  - previously saved notes
---

# Alcove

## When to Use

- User asks about **how this project is designed, architected, or specified**
- User asks about **project status, progress, or next steps**
- User asks about **coding conventions, naming rules, or forbidden patterns**
- User asks about **environment variables, secrets, or deployment config**
- User asks about **technical debt, known issues, or workarounds**
- User asks about **past decisions and their rationale**
- User wants to **initialize documentation for a new project**
- User asks to **organize, clean up, or audit project documentation**
- You need context grounded in internal docs instead of guessing

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

Case-insensitive keyword search across project docs. Use for:
- Finding where a specific feature/component is documented
- Locating decision rationale
- Checking if a convention exists

**Parameters:**
- `query` (required) — search keyword
- `scope` (optional) — `"project"` (default, CWD only) or `"global"` (all projects)
- `mode` (optional) — `"grep"` (default) or `"ranked"` (BM25 relevance scoring)
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
- User asks in Korean: "전체", "모든 프로젝트", "다른 프로젝트에서는"

**When to use ranked mode:**
- Large doc sets where grep returns too many results
- User needs the most relevant matches, not all matches
- Run `rebuild_index` first if index is stale

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

### `rebuild_index`

Build or rebuild the BM25 full-text search index. Required for `mode: "ranked"` search. Index is automatically built after `init_project`, but run this manually after bulk document changes.

### `init_project`

Initialize docs for a new project from the standard template. Automatically rebuilds the search index.

**Arguments:**
- `project_name` (required) — folder name in alcove
- `project_path` (optional) — absolute path to project repo for public docs (README, CHANGELOG)
- `overwrite` (optional) — overwrite existing files (default: false)

## Agent Instructions

### Scope principle

**Always scope to the current project unless the user explicitly says otherwise.**
- Phrases like "check docs", "remaining items", "doc status", "clean up", "audit" → current project only.
- If the intent is ambiguous between current project and all projects, **ask the user** before proceeding.
- Only use global scope or scan multiple projects when the user explicitly names them or uses keywords like "all projects", "across projects", "everywhere".

### Answering project questions

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
4. Summarize key decisions, constraints, and implications. Avoid dumping full files unless explicitly asked.
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
| alcove → project repo | Generate **public-facing** docs derived from internal content | PRD 기반으로 README 생성 |
| project repo → alcove | Restructure/incorporate reference materials into internal docs | API 스펙 분석 → ARCHITECTURE.md 보강 |
| Raw internal → project repo | **NEVER** | PRD.md를 project repo에 복사 금지 |

#### Action handling

- **`resolve_exposed_internal_docs`**: Project repo에 내부 문서(PRD, ARCHITECTURE 등)가 있으면:
  1. alcove 버전과 diff 비교
  2. project repo 버전에 추가 내용이 있으면 alcove에 먼저 병합
  3. 사용자 확인 후 project repo에서 제거

- **`move_reports_to_doc_repo`**: 분석/벤치마크/감사 보고서는 alcove `reports/`로 이동.

- **`incorporate_to_doc_repo`**: Project repo의 참고 자료를 alcove 내부 문서로 재구조화.

- **`generate_public_docs`**: Project repo에 없는 공개 문서를 내부 문서 기반으로 생성. 내부 정보 노출 금지.

- **`create_missing_internal`**: 누락된 필수 내부 문서를 `init_project`로 생성.

4. 모든 파일 이동/삭제 전 **반드시 사용자 확인**을 받을 것.
5. 완료 후 `audit_project`를 다시 실행하여 정리 결과를 확인.

### Before writing code

Always check `CONVENTIONS.md` first to ensure generated code follows project-specific rules (naming, error handling, import order, forbidden patterns).
