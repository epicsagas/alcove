---
name: alcove
description: "HTTP API-based documentation server (always running). Questions about project architecture, conventions, decisions, code structure, tech debt, env config, progress, or doc health. Also: init project, audit docs, lint, validate, promote note, rebuild index, search vaults."
---

# Alcove

HTTP API-based documentation server (always running). Auto-detects project by matching CWD against `DOCS_ROOT` folders.

## Prerequisites

The alcove API server must be running. Check and start with:

```bash
alcove api status   # check if server is running
alcove api start    # start if not running
```

- **Base URL**: `http://localhost:58301`
- **Auth**: `Authorization: Bearer $ALCOVE_TOKEN` (if token configured during setup)
- **Health check**: `curl -s http://localhost:58301/health`

All commands below use the Bash tool to run `curl`. If `ALCOVE_TOKEN` is set, add `-H "Authorization: Bearer $ALCOVE_TOKEN"` to every request.

## When to Use

Any question about project design, status, conventions, decisions, env config, tech debt, code structure, or doc health. **Check alcove before answering, not after.**

## Document Routing

| Question | File |
|----------|------|
| "What does this do?" | `PRD.md` |
| "How is this built?" / code structure | `ARCHITECTURE.md` / `CODE_INDEX.md` |
| "What's the status?" | `PROGRESS.md` |
| "Why was X chosen?" | `DECISIONS.md` |
| "What style to use?" | `CONVENTIONS.md` |
| "What env vars needed?" | `SECRETS_MAP.md` |
| "Any known issues?" | `DEBT.md` |

Unsure → search via API. **Never contradict existing decisions.**

## API Reference

### Search & Discovery

Use the search endpoint. Project is auto-detected from CWD.

```bash
# Search current project (default)
curl -s 'http://localhost:58301/search?q=QUERY'

# Search with options
curl -s 'http://localhost:58301/search?q=QUERY&limit=10&mode=hybrid'

# Search a specific project
curl -s 'http://localhost:58301/search?q=QUERY&project=PROJ'

# Search across all projects
curl -s 'http://localhost:58301/search?q=QUERY&limit=20'

# POST search (JSON body)
curl -s -X POST http://localhost:58301/v1/search \
  -H 'Content-Type: application/json' \
  -d '{"q": "QUERY", "limit": 10, "project": "proj", "mode": "hybrid"}'
```

Response: `{"query": "...", "results": [...], "mode": "...", "truncated": false}`

### API Proxy (Tool Calls)

All 16 alcove tools are available via the HTTP API proxy. Use the Bash tool:

```bash
curl -s -X POST http://localhost:58301/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"TOOL_NAME","arguments":{}}}'
```

Available tools:

| Tool | Arguments | Purpose |
|------|-----------|---------|
| `get_project_docs_overview` | `{}` | List all docs for current project with sizes and classification |
| `search_project_docs` | `{"query": "...", "limit": 10, "scope": "project"}` | BM25/grep search. Scope: `project` (default) or `global` |
| `get_doc_file` | `{"relative_path": "PRD.md"}` | Read a specific doc file |
| `list_projects` | `{}` | List all projects in the doc-repo |
| `audit_project` | `{}` | Audit doc health (missing, outdated, misplaced) |
| `init_project` | `{"project_name": "name", "project_path": "/abs/path"}` | Initialize docs from templates |
| `validate_docs` | `{}` | Validate against `policy.toml` |
| `lint_project` | `{"project": "name"}` (optional) | Broken links, orphans, stale markers |
| `rebuild_index` | `{}` | Full index rebuild |
| `check_doc_changes` | `{"auto_rebuild": true}` | Check changed files since last index |
| `promote_document` | `{"source": "/abs/path", "project": "name", "copy": true}` | Import file into doc-repo |
| `configure_project` | `{"project_name": "name", "core_files": [...], ...}` | Update project settings in alcove.toml |
| `index_code_structure` | `{"source_path": "/abs/path", "language": "rust"}` | Index source code via tree-sitter |
| `search_vault` | `{"query": "...", "vault": "name"}` | Search knowledge base vaults |
| `list_vaults` | `{}` | List vaults with doc counts |
| `backup_vault` | `{"vault_name": "name"}` | Git snapshot of vault state |

### Health Check

```bash
curl -s http://localhost:58301/health
# → {"status": "ok", "version": "x.y.z", "docs_root_configured": true, "projects": N}
```

## Rules

### Scope
**Default: current project.** Ambiguous → ask. Global only on explicit request.

### Before writing code
1. `CONVENTIONS.md` → project-specific rules
2. `CODE_INDEX.md` → compact module/type/function overview (avoids reading dozens of source files)
3. For research/reference material → search vaults via `search_vault` tool

### Answering questions
**Never answer from memory.** Call `get_project_docs_overview` → `get_doc_file` for the relevant file → summarize. Do not dump full files unless asked.

### Doc status disambiguation
| User says | Tool call |
|-----------|-----------|
| validate, policy, compliance | `validate_docs` |
| lint, broken link, orphan, stale | `lint_project` |
| audit, organize, cleanup, what's missing | `audit_project` (runs both validate + lint) |
| changed, stale index, new files | `check_doc_changes` with `auto_rebuild: true` |

Ambiguous → call `audit_project` (broadest).

### Acting on audit results
- **alcove → project repo**: OK for public-facing docs derived from internal content
- **project repo → alcove**: OK to restructure reference materials
- **Internal docs → project repo**: **NEVER** expose PRD/ARCHITECTURE/etc.
- **Always confirm** before moving/deleting files
- Re-run validate + lint after cleanup

### Promoting notes
Path provided → act immediately: call `promote_document` with the source path. No matching project → `inbox/`. Then call `rebuild_index`.

### After development
Proactively capture at natural stopping points:
- Architecture change → `ARCHITECTURE.md`
- Decision rationale → `DECISIONS.md`
- Bug/workaround → `DEBT.md`
- Coding pattern → `CONVENTIONS.md`
- Env var → `SECRETS_MAP.md`
- Progress → `PROGRESS.md`

Read → append with date → call `rebuild_index`.
