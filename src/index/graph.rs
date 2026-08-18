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
#[allow(dead_code)]
pub(crate) struct DocGraph {
    backend: SqliteGraph,
}

/// A resolved document reference — doc id + title + relation type.
#[derive(Debug, Clone)]
pub(crate) struct DocLink {
    pub(crate) doc_id: String,
    pub(crate) title: String,
    pub(crate) relation: String,
}

impl DocGraph {
    /// Open (creating if needed) the graph DB under `.alcove/docgraph.db`.
    /// `SqliteGraph::open` applies the schema + migrations automatically.
    #[allow(dead_code)]
    pub(crate) fn open(docs_root: &Path) -> Result<Self> {
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
    /// wipe incoming backlinks from other docs).
    fn refresh_out_edges(&self, doc_id: &str, edges: &[GraphEdge]) -> Result<()> {
        for existing in self
            .backend
            .edges_for_node_dir(doc_id, EdgeDirection::Out, None)?
        {
            let _ = self.backend.delete_edge(&existing.id);
        }
        if !edges.is_empty() {
            self.backend.append_edges(edges)?;
        }
        Ok(())
    }

    /// Documents that link *to* `doc_id` (backlinks).
    pub(crate) fn backlinks(&self, doc_id: &str) -> Result<Vec<DocLink>> {
        let edges = self
            .backend
            .edges_for_node_dir(doc_id, EdgeDirection::In, None)?;
        self.edges_to_doc_links(edges, |e| e.source.clone())
    }

    /// Documents that `doc_id` links *to* (outgoing references).
    pub(crate) fn related_docs(&self, doc_id: &str) -> Result<Vec<DocLink>> {
        let edges = self
            .backend
            .edges_for_node_dir(doc_id, EdgeDirection::Out, None)?;
        self.edges_to_doc_links(edges, |e| e.target.clone())
    }

    /// Read a document node's title, if present.
    #[allow(dead_code)] // used by tests and future tool surface
    pub(crate) fn read_doc_title(&self, doc_id: &str) -> Result<Option<String>> {
        Ok(self.backend.read_node(doc_id)?.map(|n| n.title))
    }

    fn edges_to_doc_links(
        &self,
        edges: Vec<GraphEdge>,
        key: impl Fn(&GraphEdge) -> String,
    ) -> Result<Vec<DocLink>> {
        let mut out = Vec::with_capacity(edges.len());
        for edge in edges {
            let doc_id = key(&edge);
            let title = self
                .backend
                .read_node(&doc_id)?
                .map(|n| n.title)
                .unwrap_or_default();
            out.push(DocLink {
                doc_id,
                title,
                relation: edge.relation,
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

/// Build the doc graph for `docs_root` from `files`.
///
/// `files`: `(project, rel_to_project, absolute_path)` tuples — the same shape
/// `builder::scan_all_files` produces. Markdown links/wikilinks are extracted
/// per file, resolved against a filename map built from the full file set, and
/// upserted as directed edges.
///
/// Idempotent: nodes are upserted, out-edges are replaced per file, and docs
/// absent from `files` are pruned (node + edges) via `delete_node`.
pub(crate) fn rebuild_doc_graph(
    docs_root: &Path,
    files: &[(String, String, PathBuf)],
) -> Result<()> {
    let graph = DocGraph::open(docs_root)?;

    // filename map over all indexed files (Obsidian-style bare-name links).
    let paths: Vec<PathBuf> = files.iter().map(|(_, _, p)| p.clone()).collect();
    let filename_map = build_filename_map(&paths);

    let current_ids: std::collections::HashSet<String> = files
        .iter()
        .filter_map(|(_, _, abs)| path_to_doc_id(abs, docs_root))
        .collect();

    for (_proj, _rel, abs) in files {
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

    // Prune nodes no longer present.
    for id in graph.all_doc_ids() {
        if !current_ids.contains(&id) {
            let _ = graph.backend.delete_node(&id);
        }
    }

    Ok(())
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
        if let Some(abs) = resolve_link_path(raw, containing_file, docs_root, filename_map) {
            if let Some(tid) = path_to_doc_id(&abs, docs_root) {
                push(tid, REL_WIKILINK);
            }
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
        if let Some(abs) = resolve_link_path(path_part, containing_file, docs_root, filename_map)
        {
            if let Some(tid) = path_to_doc_id(&abs, docs_root) {
                push(tid, REL_LINK);
            }
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
        rebuild_doc_graph(root, &files).unwrap();

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
        rebuild_doc_graph(root, &files).unwrap();

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
        rebuild_doc_graph(root, &files).unwrap();

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
        rebuild_doc_graph(root, &files).unwrap();
        let g = DocGraph::open(root).unwrap();
        assert_eq!(g.related_docs("proj/a.md").unwrap().len(), 2);

        // Edit a.md to drop the link to c.
        write(root, "proj/a.md", "# A\n\n[[b]] only.\n");
        rebuild_doc_graph(root, &files).unwrap();
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
        rebuild_doc_graph(root, &files).unwrap();

        // Remove b from the file set → should be pruned.
        std::fs::remove_file(root.join("proj/b.md")).unwrap();
        let files = make_files(root, &["proj/a.md"]);
        rebuild_doc_graph(root, &files).unwrap();

        let g = DocGraph::open(root).unwrap();
        assert!(g.read_doc_title("proj/b.md").unwrap().is_none());
        // The dangling edge a→b must be gone too.
        let related = g.related_docs("proj/a.md").unwrap();
        assert!(related.is_empty(), "edge to pruned node must be removed");
    }
}
