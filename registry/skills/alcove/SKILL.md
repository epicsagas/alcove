---
name: alcove
description: "Questions about project architecture, conventions, decisions, code structure, tech debt, env config, progress, or doc health. Also: init project, audit docs, lint, validate, promote note, rebuild index, search vaults."
---

# Alcove

CLI-based documentation server. Auto-detects project by matching CWD against `DOCS_ROOT` folders.

## Prerequisites

The `alcove` binary must be installed and on PATH. If not found, install via:
```bash
brew install epicsagas/tap/alcove    # macOS
cargo binstall alcove                # cross-platform
```

Verify: `alcove --version`

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

Unsure → `alcove search "QUERY" --scope project`. **Never contradict existing decisions.**

## Commands

### Search & Discovery

| CLI Command | Purpose |
|-------------|---------|
| `alcove search "QUERY" --scope project` | BM25/grep search within current project. Default scope. |
| `alcove search "QUERY" --scope global` | Search across all projects. Use only when user explicitly requests. |
| `alcove search "QUERY" --limit N --mode auto` | Control result count and search mode (auto, grep, ranked). |
| `alcove index-code --source PATH` | Index source code structure (tree-sitter) for code-aware search. |
| `alcove vault list` | List knowledge base vaults with doc counts. |

### Validation & Linting

| CLI Command | Purpose |
|-------------|---------|
| `alcove validate --format json` | Validate against `policy.toml` → pass/warn/fail. |
| `alcove lint --format json` | Broken links, orphans, stale markers, stale dates. |

### Index Management

| CLI Command | Purpose |
|-------------|---------|
| `alcove index` | Incremental index update (only changed files). |
| `alcove rebuild` | Full index rebuild from scratch. |

### Document Operations

| CLI Command | Purpose |
|-------------|---------|
| `alcove promote SOURCE [--project PROJ] [--mv]` | Import file from external vault into doc-repo. Use `--mv` to move instead of copy. |

### Setup & Maintenance

| CLI Command | Purpose |
|-------------|---------|
| `alcove setup` | Interactive setup: docs root, categories, diagram format. |
| `alcove doctor` | Check health of the alcove installation. |

### File Operations (use Read/Bash tools directly)

| Operation | How |
|-----------|-----|
| Read a doc file | `Read` tool with the full path under docs root. Find docs root via `alcove search` or `alcove.toml`. |
| List projects | `ls` the docs root directory. Discover via `alcove search --scope global` or check `alcove.toml`. |
| Get docs overview | Combine `ls` of project docs directory with reading tier info from `alcove.toml`. |
| Audit project | Run `alcove validate --format json` + `alcove lint --format json` together. |
| Configure project | Edit `alcove.toml` directly using the Edit tool. |
| Search vaults | Use `grep -ri "QUERY"` in vault directory paths (find via `alcove vault list`). |

## Rules

### Scope
**Default: current project.** Ambiguous → ask. Global only on explicit request.

### Before writing code
1. `CONVENTIONS.md` → project-specific rules
2. `CODE_INDEX.md` → compact module/type/function overview (avoids reading dozens of source files)
3. For research/reference material → search vaults via `grep -ri` in vault directories

### Answering questions
**Never answer from memory.** `ls` the project docs directory → `Read` the relevant file → summarize. Do not dump full files unless asked.

### Doc status disambiguation
| User says | Command |
|-----------|---------|
| validate, policy, compliance | `alcove validate --format json` |
| lint, broken link, orphan, stale | `alcove lint --format json` |
| audit, organize, cleanup, what's missing | Run both: `alcove validate --format json` && `alcove lint --format json` |
| changed, stale index, new files | `alcove index` |

Ambiguous → run both validate and lint (broadest).

### Acting on audit results
- **alcove → project repo**: OK for public-facing docs derived from internal content
- **project repo → alcove**: OK to restructure reference materials
- **Internal docs → project repo**: **NEVER** expose PRD/ARCHITECTURE/etc.
- **Always confirm** before moving/deleting files
- Re-run validate + lint after cleanup

### Promoting notes
Path provided → act immediately: `alcove promote SOURCE`. No matching project → `inbox/`. Then `alcove index`.

### After development
Proactively capture at natural stopping points:
- Architecture change → `ARCHITECTURE.md`
- Decision rationale → `DECISIONS.md`
- Bug/workaround → `DEBT.md`
- Coding pattern → `CONVENTIONS.md`
- Env var → `SECRETS_MAP.md`
- Progress → `PROGRESS.md`

Read → append with date → `alcove index`.
