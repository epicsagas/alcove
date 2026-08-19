//! Document relationship graph — wikilink/backlink topology backed by
//! llm-kernel's `graph` SQLite backend.
//!
//! Nodes are indexed documents (id = docs-root-relative path), edges are
//! `[[wikilink]]` / `[text](path)` references resolved to their target file.
//! This is a pure-topology use of the graph module: the memory-oriented node
//! fields (`importance`, `access_count`) are left at defaults and unused
//! (see `ponytail:` note on [`DocGraph::upsert_doc`]).

#![cfg(feature = "doc-graph")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use llm_kernel::graph::{EdgeDirection, GraphBackend, GraphEdge, GraphNode, SqliteGraph};

use crate::lint::{
    build_filename_map, md_link_re, resolve_link_path, strip_code_blocks, wiki_link_re,
};

/// Relation tag stored on edges.
const REL_WIKILINK: &str = "wikilink";
const REL_LINK: &str = "link";
/// Stable timestamp for unused created/updated columns.
const TS: &str = "1970-01-01T00:00:00Z";

/// Wrapper over llm-kernel's `SqliteGraph` storing document reference topology.
///
/// Lives in its own SQLite file (`.alcove/docgraph.db`) to avoid any schema
/// collision with the vector/embedding-cache DBs (graph uses `nodes`/`edges`,
/// alcove uses `vectors`/`meta`).
///
/// `pub`: the lib target compiles this module but only the bin target (MCP
/// tools) constructs `DocGraph` — crate-private visibility trips `-D
/// dead_code` on the lib build.
pub struct DocGraph {
    backend: SqliteGraph,
}

/// A resolved document reference — doc id + title + relation type, plus the
/// target node's temporal-validity metadata (empty when unset).
#[derive(Debug, Clone)]
pub struct DocLink {
    pub doc_id: String,
    pub title: String,
    pub relation: String,
    pub valid_until: String,
    pub last_verified: String,
}

impl DocGraph {
    /// Open (creating if needed) the graph DB under `.alcove/docgraph.db`.
    /// `SqliteGraph::open` applies the schema + migrations automatically.
    pub fn open(docs_root: &Path) -> Result<Self> {
        let alcove_dir = docs_root.join(".alcove");
        std::fs::create_dir_all(&alcove_dir)?;
        let db_path = alcove_dir.join("docgraph.db");
        let backend = SqliteGraph::open(&db_path)
            .with_context(|| format!("opening doc graph {}", db_path.display()))?;
        Ok(Self { backend })
    }

    /// Upsert a document node.
    ///
    /// `ponytail:` GraphNode carries memory-graph fields (importance,
    /// access_count, accessed_at) that are dead weight for pure topology.
    /// `..Default::default()` leaves them at defaults — a known ceiling we
    /// accept rather than forking a topology-only node type.
    fn upsert_doc(&self, doc_id: &str, title: &str) -> Result<()> {
        self.backend.upsert_node(&GraphNode {
            id: doc_id.to_string(),
            node_type: "doc".to_string(),
            title: title.to_string(),
            created: TS.to_string(),
            updated: TS.to_string(),
            ..Default::default()
        })?;
        Ok(())
    }

    /// Replace all out-edges of `doc_id` with `edges` (delete-then-insert).
    ///
    /// Deletes only the node's *outgoing* edges by id, not every touching edge
    /// (llm-kernel's `remove_edges_for_node` is bidirectional and would also
    /// wipe incoming backlinks from other docs). Delete + insert run in one
    /// transaction (`with_tx`, llm-kernel 0.26.1) so a failure mid-refresh
    /// rolls back to the previous edges. The out-edge listing happens before
    /// the tx — alcove is the single writer, so no race in practice.
    fn refresh_out_edges(&self, doc_id: &str, edges: &[GraphEdge]) -> Result<()> {
        let existing_ids: Vec<String> = self
            .backend
            .edges_for_node_dir(doc_id, EdgeDirection::Out, None)?
            .into_iter()
            .map(|e| e.id)
            .collect();
        self.backend
            .with_tx(|conn| {
                for id in &existing_ids {
                    llm_kernel::graph::store::delete_edge(conn, id)?;
                }
                if !edges.is_empty() {
                    // append_edges() opens its own tx — nested tx fails, so
                    // use the per-edge insert inside this one.
                    for edge in edges {
                        llm_kernel::graph::store::append_edge(conn, edge)?;
                    }
                }
                Ok(())
            })
            .map_err(|e| anyhow::anyhow!(e))
    }

    /// Documents that link *to* `doc_id` (backlinks).
    ///
    /// An expired doc (valid_until in the past) recalls nothing — same
    /// exclusion rule as for link targets, applied to the queried node.
    pub fn backlinks(&self, doc_id: &str) -> Result<Vec<DocLink>> {
        if self.is_expired(doc_id)? {
            return Ok(Vec::new());
        }
        let edges = self
            .backend
            .edges_for_node_dir(doc_id, EdgeDirection::In, None)?;
        self.edges_to_doc_links(edges, |e| e.source.clone())
    }

    /// Documents that `doc_id` links *to* (outgoing references).
    ///
    /// An expired doc recalls nothing (see [`backlinks`](Self::backlinks)).
    pub fn related_docs(&self, doc_id: &str) -> Result<Vec<DocLink>> {
        if self.is_expired(doc_id)? {
            return Ok(Vec::new());
        }
        let edges = self
            .backend
            .edges_for_node_dir(doc_id, EdgeDirection::Out, None)?;
        self.edges_to_doc_links(edges, |e| e.target.clone())
    }

    /// Whether the node exists with `valid_until` set and in the past.
    pub fn is_expired(&self, doc_id: &str) -> Result<bool> {
        Ok(self
            .backend
            .read_node(doc_id)?
            .is_some_and(|n| expired_before(&n.valid_until, &now_iso())))
    }

    /// Stamp `last_verified` on a node. Returns `false` for an unknown id.
    pub fn mark_verified(&self, doc_id: &str, now: &str) -> Result<bool> {
        self.backend.mark_verified(doc_id, now).map_err(Into::into)
    }

    /// Count nodes whose `valid_until` is set and earlier than `now`.
    pub fn count_expired_nodes(&self, now: &str) -> Result<u64> {
        self.backend.count_expired_nodes(now).map_err(Into::into)
    }

    /// Read a document node's title, if present.
    // Used by this module's tests; the bin tools surface titles via
    // backlinks/related_docs only. Would trip -D dead_code on the bin target.
    #[allow(dead_code)]
    pub fn read_doc_title(&self, doc_id: &str) -> Result<Option<String>> {
        Ok(self.backend.read_node(doc_id)?.map(|n| n.title))
    }

    fn edges_to_doc_links(
        &self,
        edges: Vec<GraphEdge>,
        key: impl Fn(&GraphEdge) -> String,
    ) -> Result<Vec<DocLink>> {
        let now = now_iso();
        let mut out = Vec::with_capacity(edges.len());
        for edge in edges {
            let doc_id = key(&edge);
            let Some(node) = self.backend.read_node(&doc_id)? else {
                continue;
            };
            // Temporal validity: a link is dropped from recall when *either*
            // endpoint has expired (valid_until set and in the past) — an
            // exclusion rule at query time, per the #37 lifecycle discussion.
            // `node` is the doc_id endpoint; check the other endpoint only
            // when it differs (the queried endpoint was already checked by
            // the caller via `is_expired`).
            let other = if doc_id == edge.target {
                &edge.source
            } else {
                &edge.target
            };
            let other_expired = if other == &doc_id {
                false
            } else {
                self.backend
                    .read_node(other)?
                    .is_some_and(|n| expired_before(&n.valid_until, &now))
            };
            if expired_before(&node.valid_until, &now) || other_expired {
                continue;
            }
            out.push(DocLink {
                doc_id,
                title: node.title,
                relation: edge.relation,
                valid_until: node.valid_until,
                last_verified: node.last_verified,
            });
        }
        Ok(out)
    }

    /// All doc node ids currently stored (for prune diffing).
    fn all_doc_ids(&self) -> Vec<String> {
        // query_nodes(filter by node_type="doc"): (tag, node_type, project, limit)
        self.backend
            .query_nodes(None, Some("doc"), None, usize::MAX)
            .unwrap_or_default()
            .into_iter()
            .map(|n| n.id)
            .collect()
    }
}

/// Resolve an absolute file path to a docs-root-relative doc id.
fn path_to_doc_id(abs: &Path, docs_root: &Path) -> Option<String> {
    abs.strip_prefix(docs_root)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

/// Current UTC time, ISO 8601 with second precision — the comparison key for
/// `valid_until`.
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Whether `valid_until` (ISO 8601) is set and earlier than `now`.
///
/// Parsed as datetimes so mixed UTC-offsets compare correctly (a plain
/// lexicographic compare mis-orders e.g. `23:00+09:00` vs `15:00Z`);
/// falls back to string compare if either side fails to parse.
fn expired_before(valid_until: &str, now: &str) -> bool {
    if valid_until.is_empty() {
        return false;
    }
    match (
        chrono::DateTime::parse_from_rfc3339(valid_until),
        chrono::DateTime::parse_from_rfc3339(now),
    ) {
        (Ok(v), Ok(n)) => v < n,
        _ => valid_until < now,
    }
}

/// Build (or incrementally update) the doc graph for `docs_root`.
///
/// `files`: full `(project, rel_to_project, absolute_path)` set — the same
/// shape `builder::scan_all_files` produces. Used for the filename map
/// (Obsidian-style bare-name resolution) and the prune diff.
///
/// `changed`: subset of `files` whose contents may have changed (the
/// incremental-index delta). When the on-disk file set differs from the last
/// run (stamp mismatch, missing graph DB), a full rebuild happens regardless
/// and `changed` is ignored — this also repairs links in unchanged files that
/// start/stopped resolving when another doc was added or removed.
///
/// Returns a short status string for the index result JSON.
pub(crate) fn rebuild_doc_graph(
    docs_root: &Path,
    files: &[(String, String, PathBuf)],
    changed: Option<&[(String, String, PathBuf)]>,
) -> Result<String> {
    let graph = DocGraph::open(docs_root)?;

    // filename map over all indexed files (Obsidian-style bare-name links).
    let paths: Vec<PathBuf> = files.iter().map(|(_, _, p)| p.clone()).collect();
    let filename_map = build_filename_map(&paths);

    let current_ids: std::collections::HashSet<String> = files
        .iter()
        .filter_map(|(_, _, abs)| path_to_doc_id(abs, docs_root))
        .collect();

    // Full rebuild when the file set changed since the last run or the graph
    // DB is gone (stamp can outlive a manually deleted docgraph.db).
    let stamp_path = docs_root.join(".alcove").join("docgraph.stamp");
    let stamp = stamp_text(&current_ids);
    let db_exists = docs_root.join(".alcove").join("docgraph.db").exists();
    let stamp_matches = std::fs::read_to_string(&stamp_path).is_ok_and(|s| s == stamp);
    let file_set_changed = !db_exists || !stamp_matches;

    let to_process: Vec<&(String, String, PathBuf)> = if file_set_changed {
        files.iter().collect()
    } else {
        match changed {
            Some(delta) => delta.iter().collect(),
            None => Vec::new(),
        }
    };

    let processed = to_process.len();
    for (_proj, _rel, abs) in to_process {
        let Some(doc_id) = path_to_doc_id(abs, docs_root) else {
            continue;
        };
        // Read + extract links. Non-utf8 / read errors → skip the file.
        let Ok(content) = std::fs::read_to_string(abs) else {
            continue;
        };
        let title = abs
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&doc_id)
            .to_string();
        graph.upsert_doc(&doc_id, &title)?;

        let edges = extract_edges(&content, abs, docs_root, &filename_map);
        graph.refresh_out_edges(&doc_id, &edges)?;
    }

    // Prune nodes no longer present (only meaningful on a full rebuild).
    if file_set_changed {
        for id in graph.all_doc_ids() {
            if !current_ids.contains(&id) {
                let _ = graph.backend.delete_node(&id);
            }
        }
        let _ = std::fs::write(&stamp_path, &stamp);
    }

    let mode = if file_set_changed {
        "rebuilt"
    } else {
        "updated"
    };
    Ok(format!("{mode} {}/{} docs", processed, current_ids.len()))
}

/// Stamp content: deterministic hash of the sorted doc-id set, so adding or
/// removing any file triggers a full graph rebuild.
///
/// `DefaultHasher` is not stable across platforms/releases — fine here: the
/// stamp is only compared against one written by the same binary on the same
/// machine, and a mismatch just triggers a (correct) full rebuild.
fn stamp_text(ids: &std::collections::HashSet<String>) -> String {
    use std::hash::{Hash, Hasher};
    let mut sorted: Vec<&String> = ids.iter().collect();
    sorted.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for id in sorted {
        id.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// Extract directed edges from `content`: `[[wikilink]]` and `[text](path)`
/// targets, each resolved to an absolute path → doc id.
fn extract_edges(
    content: &str,
    containing_file: &Path,
    docs_root: &Path,
    filename_map: &HashMap<String, PathBuf>,
) -> Vec<GraphEdge> {
    let prose = strip_code_blocks(content);
    let source_id = match path_to_doc_id(containing_file, docs_root) {
        Some(s) => s,
        None => return Vec::new(),
    };
    let mut edges: Vec<GraphEdge> = Vec::new();

    let mut push = |target: String, relation: &str| {
        // Deterministic edge id so re-inserts are idempotent (INSERT OR IGNORE).
        // ponytail: a `->` inside a filename could alias two distinct edges —
        // pathological, and the cost is one dropped duplicate edge; switch to a
        // hash id if it ever bites.
        let id = format!("{relation}:{source_id}->{target}");
        edges.push(GraphEdge {
            id,
            source: source_id.clone(),
            target,
            relation: relation.to_string(),
            weight: 1.0,
            ts: TS.to_string(),
        });
    };

    // Wikilinks: [[target]] or [[target|alias]]
    for cap in wiki_link_re().captures_iter(&prose) {
        let raw = cap[1].trim();
        let tid = resolve_link_path(raw, containing_file, docs_root, filename_map)
            .and_then(|abs| path_to_doc_id(&abs, docs_root));
        if let Some(tid) = tid {
            push(tid, REL_WIKILINK);
        }
    }

    // Markdown links: [text](path) — skip http/https/mailto/anchor
    for cap in md_link_re().captures_iter(&prose) {
        let target = cap[1].trim();
        if target.starts_with("http://")
            || target.starts_with("https://")
            || target.starts_with("mailto:")
            || target.starts_with('#')
        {
            continue;
        }
        let path_part = target.split('#').next().unwrap_or(target);
        if path_part.is_empty() {
            continue;
        }
        let tid = resolve_link_path(path_part, containing_file, docs_root, filename_map)
            .and_then(|abs| path_to_doc_id(&abs, docs_root));
        if let Some(tid) = tid {
            push(tid, REL_LINK);
        }
    }

    edges
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) -> PathBuf {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, body).unwrap();
        p
    }

    fn make_files(root: &Path, rels: &[&str]) -> Vec<(String, String, PathBuf)> {
        rels.iter()
            .map(|rel| {
                let abs = root.join(rel);
                let (proj, rest) = rel
                    .split_once('/')
                    .map(|(p, r)| (p.to_string(), r.to_string()))
                    .unwrap_or((String::new(), rel.to_string()));
                (proj, rest, abs)
            })
            .collect()
    }

    #[test]
    fn wikilink_creates_backlink_and_related() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "proj/a.md", "# A\n\nSee [[b]] for details.\n");
        write(root, "proj/b.md", "# B\n\nReferenced by a.\n");
        let files = make_files(root, &["proj/a.md", "proj/b.md"]);
        rebuild_doc_graph(root, &files, None).unwrap();

        let g = DocGraph::open(root).unwrap();
        // a links to b
        let related = g.related_docs("proj/a.md").unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].doc_id, "proj/b.md");
        assert_eq!(related[0].relation, "wikilink");
        // b has a backlink from a
        let back = g.backlinks("proj/b.md").unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].doc_id, "proj/a.md");
    }

    #[test]
    fn markdown_link_resolved() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "proj/a.md", "# A\n\nLink to [b](b.md).\n");
        write(root, "proj/b.md", "# B\n");
        let files = make_files(root, &["proj/a.md", "proj/b.md"]);
        rebuild_doc_graph(root, &files, None).unwrap();

        let g = DocGraph::open(root).unwrap();
        let related = g.related_docs("proj/a.md").unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].relation, "link");
    }

    #[test]
    fn codeblock_toml_table_not_wikilink() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(
            root,
            "proj/a.md",
            "# A\n\n```toml\n[[dependencies]]\nfoo = 1\n```\nNo real link here.\n",
        );
        write(root, "proj/dependencies.md", "# Deps\n");
        let files = make_files(root, &["proj/a.md", "proj/dependencies.md"]);
        rebuild_doc_graph(root, &files, None).unwrap();

        let g = DocGraph::open(root).unwrap();
        let related = g.related_docs("proj/a.md").unwrap();
        // [[dependencies]] inside a code fence must NOT produce an edge.
        assert!(related.is_empty(), "code-fence [[table]] must be ignored");
    }

    #[test]
    fn reindex_refreshes_out_edges() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "proj/a.md", "# A\n\n[[b]] [[c]]\n");
        write(root, "proj/b.md", "# B\n");
        write(root, "proj/c.md", "# C\n");
        let files = make_files(root, &["proj/a.md", "proj/b.md", "proj/c.md"]);
        rebuild_doc_graph(root, &files, None).unwrap();
        let g = DocGraph::open(root).unwrap();
        assert_eq!(g.related_docs("proj/a.md").unwrap().len(), 2);

        // Edit a.md to drop the link to c. Same file set, so the incremental
        // path runs: only a.md is reprocessed (stamp matches).
        write(root, "proj/a.md", "# A\n\n[[b]] only.\n");
        let files_a = make_files(root, &["proj/a.md"]);
        rebuild_doc_graph(root, &files, Some(&files_a)).unwrap();
        let g = DocGraph::open(root).unwrap();
        let related = g.related_docs("proj/a.md").unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].doc_id, "proj/b.md");
    }

    #[test]
    fn deleted_doc_is_pruned() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "proj/a.md", "# A\n\n[[b]]\n");
        write(root, "proj/b.md", "# B\n");
        let files = make_files(root, &["proj/a.md", "proj/b.md"]);
        rebuild_doc_graph(root, &files, None).unwrap();

        // Remove b from the file set → should be pruned.
        std::fs::remove_file(root.join("proj/b.md")).unwrap();
        let files = make_files(root, &["proj/a.md"]);
        rebuild_doc_graph(root, &files, None).unwrap();

        let g = DocGraph::open(root).unwrap();
        assert!(g.read_doc_title("proj/b.md").unwrap().is_none());
        // The dangling edge a→b must be gone too.
        let related = g.related_docs("proj/a.md").unwrap();
        assert!(related.is_empty(), "edge to pruned node must be removed");
    }

    #[test]
    fn expired_target_excluded_from_recall() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, "proj/a.md", "# A\n\n[[b]] [[c]]\n");
        write(root, "proj/b.md", "# B\n");
        write(root, "proj/c.md", "# C\n");
        let files = make_files(root, &["proj/a.md", "proj/b.md", "proj/c.md"]);
        rebuild_doc_graph(root, &files, None).unwrap();

        // Expire c: set valid_until in the past, then upsert back.
        let g = DocGraph::open(root).unwrap();
        let mut node = g.backend.read_node("proj/c.md").unwrap().unwrap();
        node.valid_until = "2020-01-01T00:00:00+00:00".to_string();
        node.last_verified = "2019-12-01T00:00:00+00:00".to_string();
        g.backend.upsert_node(&node).unwrap();

        // Related docs: b stays, expired c drops out at query time.
        let related = g.related_docs("proj/a.md").unwrap();
        assert_eq!(related.len(), 1, "expired target must be excluded");
        assert_eq!(related[0].doc_id, "proj/b.md");
        assert!(related[0].valid_until.is_empty());
        // Backlinks on c also vanish once every incoming edge target is expired.
        assert!(g.backlinks("proj/c.md").unwrap().is_empty());
        // The expired doc itself recalls nothing in either direction.
        assert!(g.related_docs("proj/c.md").unwrap().is_empty());
        // read_doc_title still sees the node — expiry is recall-only, not deletion.
        assert!(g.read_doc_title("proj/c.md").unwrap().is_some());
    }

    #[test]
    fn expired_before_handles_mixed_utc_offsets() {
        // 2026-08-18T23:00+09:00 == 14:00Z, earlier than 15:00Z — but a plain
        // string compare would say "23..." > "15..." and keep it unexpired.
        assert!(expired_before(
            "2026-08-18T23:00:00+09:00",
            "2026-08-18T15:00:00Z"
        ));
        assert!(!expired_before(
            "2026-08-19T10:00:00+09:00", // 19th 01:00Z — future
            "2026-08-18T15:00:00Z"
        ));
        assert!(!expired_before("", "2026-08-18T15:00:00Z"));
        // Unparseable value falls back to string compare rather than panicking.
        assert!(expired_before("garbage", "zzz") == ("garbage" < "zzz"));
    }
}
