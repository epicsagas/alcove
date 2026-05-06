pub mod cache;
pub mod lock;
pub mod schema;
pub mod chunker;
pub mod reader;
pub mod builder;
pub mod searcher;

// ---------------------------------------------------------------------------
// Public re-exports — preserves the original crate-level API so callers
// (main.rs, tools.rs, server.rs, etc.) require no changes.
// ---------------------------------------------------------------------------

pub use builder::{
    build_index,
    rebuild_index,
    build_index_bm25_only,
    build_vault_index,
    rebuild_vault_index,
    build_all_vault_indexes,
    check_doc_changes,
    ensure_index_fresh,
    index_exists,
    is_index_stale,
};

pub use searcher::{
    search_indexed,
    search_vault,
};

#[cfg(feature = "alcove-full")]
pub use searcher::search_hybrid;

#[allow(unused_imports)]
pub use schema::IndexSchema;

#[allow(unused_imports)]
pub use cache::CacheCategory;

// Internal symbols used only by the tests module below.
#[cfg(test)]
pub(crate) use builder::build_index_inner;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use builder::build_index_unlocked;
#[cfg(test)]
pub(crate) use builder::IndexMeta;
#[cfg(test)]
pub(crate) use lock::{index_dir, lock_file, try_acquire_lock, is_locked, release_lock};
#[cfg(test)]
pub(crate) use schema::{SCHEMA_VERSION, register_ngram_tokenizer};
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use chunker::{chunk_content, extract_title, Chunk};
#[cfg(test)]
pub(crate) use reader::read_file_content;
#[cfg(test)]
pub(crate) use cache::{reader_cache_for, get_cached_reader};
#[cfg(test)]
pub(crate) use searcher::{sanitize_query, build_search_query, apply_project_diversity};

// ---------------------------------------------------------------------------
// Tests (verbatim from original index.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::OnceLock;
    use tempfile::TempDir;

    // ---------------------------------------------------------------------------
    // Shared read-only fixture — built once, reused by all read-only tests.
    // Tests that mutate the index must call `setup_indexed_root()` for their own
    // private TempDir so they don't corrupt the shared state.
    // ---------------------------------------------------------------------------
    static SHARED_ROOT: OnceLock<TempDir> = OnceLock::new();

    fn shared_indexed_root() -> &'static std::path::Path {
        SHARED_ROOT
            .get_or_init(|| {
                let tmp = setup_indexed_root();
                build_index_inner(tmp.path(), true).unwrap();
                tmp
            })
            .path()
    }

    fn setup_indexed_root() -> TempDir {
        let tmp = TempDir::new().unwrap();
        // backend
        let backend = tmp.path().join("backend");
        fs::create_dir_all(&backend).unwrap();
        fs::write(
            backend.join("PRD.md"),
            "# Backend PRD\n\nAuthentication flow using OAuth 2.0.\nThe API gateway handles token validation.\nRefresh tokens are stored in Redis.",
        ).unwrap();
        fs::write(
            backend.join("ARCHITECTURE.md"),
            "# Backend Architecture\n\nMicroservices design with gRPC.\nService mesh using Istio.\nDatabase: PostgreSQL with read replicas.",
        ).unwrap();
        // frontend
        let frontend = tmp.path().join("frontend");
        fs::create_dir_all(&frontend).unwrap();
        fs::write(
            frontend.join("PRD.md"),
            "# Frontend PRD\n\nLogin page with OAuth integration.\nSocial login support for Google and GitHub.",
        ).unwrap();
        // notes (knowledge base)
        let notes = tmp.path().join("notes");
        fs::create_dir_all(&notes).unwrap();
        fs::write(
            notes.join("k8s-tips.md"),
            "# K8s Tips\n\nTroubleshooting CrashLoopBackOff errors.\nCheck resource limits and liveness probes.\nUse kubectl describe pod for diagnostics.",
        ).unwrap();
        fs::write(
            notes.join("oauth-memo.md"),
            "# OAuth Memo\n\nOAuth 2.0 authorization code flow.\nPKCE extension for public clients.\nToken refresh best practices.",
        ).unwrap();
        // hidden (should be skipped)
        fs::create_dir_all(tmp.path().join("_template")).unwrap();
        fs::write(tmp.path().join("_template/TPL.md"), "# Template").unwrap();
        tmp
    }

    #[test]
    fn build_index_succeeds() {
        let tmp = setup_indexed_root();
        let result = build_index_inner(tmp.path(), true).unwrap();
        assert_eq!(result["status"], "ok");
        assert!(result["indexed"].as_u64().unwrap() >= 5);
        assert!(result["projects"].as_u64().unwrap() >= 3);
        // Index directory should exist
        assert!(tmp.path().join(".alcove/index").exists());
    }

    #[test]
    fn build_index_incremental_skips_unchanged() {
        let tmp = setup_indexed_root();
        // First build
        build_index_inner(tmp.path(), true).unwrap();
        // Second build with no changes
        let result = build_index_inner(tmp.path(), true).unwrap();
        assert_eq!(result["status"], "ok");
        // All files should be skipped on second run
        assert!(result["skipped"].as_u64().unwrap() >= 5);
    }

    #[test]
    fn search_indexed_finds_oauth() {
        let root = shared_indexed_root();
        let result = search_indexed(root, "OAuth", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "should find OAuth matches");
        assert_eq!(result["mode"], "ranked");

        // Should have scores
        for m in matches {
            assert!(m["score"].as_f64().unwrap() > 0.0);
            assert!(m["project"].is_string());
        }
    }

    #[test]
    fn search_indexed_with_project_filter() {
        let root = shared_indexed_root();
        let result = search_indexed(root, "OAuth", 10, Some("backend")).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty());
        for m in matches {
            assert_eq!(m["project"], "backend");
        }
    }

    #[test]
    fn search_indexed_respects_limit() {
        let root = shared_indexed_root();
        let result = search_indexed(root, "OAuth", 1, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn search_indexed_no_results() {
        let root = shared_indexed_root();
        let result = search_indexed(root, "zzz_nonexistent_query_zzz", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn search_indexed_skips_hidden_projects() {
        let root = shared_indexed_root();
        let result = search_indexed(root, "Template", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        let projects: Vec<&str> = matches
            .iter()
            .filter_map(|m| m["project"].as_str())
            .collect();
        assert!(!projects.contains(&"_template"));
    }

    #[test]
    fn chunk_content_basic() {
        let content = "line1\nline2\nline3";
        let chunks = chunk_content(content, "md");
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].line_start, 1);
    }

    #[test]
    fn chunk_content_long_splits() {
        // Create content longer than CHUNK_SIZE
        let lines: Vec<String> = (0..100)
            .map(|i| {
                format!(
                    "This is line number {} with some padding text to make it longer.",
                    i
                )
            })
            .collect();
        let content = lines.join("\n");
        let chunks = chunk_content(&content, "md");
        assert!(
            chunks.len() > 1,
            "long content should produce multiple chunks"
        );
        // First chunk starts at line 1
        assert_eq!(chunks[0].line_start, 1);
    }

    #[test]
    fn chunk_content_empty() {
        let chunks = chunk_content("", "md");
        assert!(chunks.is_empty());
    }

    #[test]
    fn is_index_stale_when_no_index() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("proj")).unwrap();
        fs::write(tmp.path().join("proj/DOC.md"), "# Doc").unwrap();
        assert!(is_index_stale(tmp.path()));
    }

    #[test]
    fn is_index_fresh_after_build() {
        let root = shared_indexed_root();
        assert!(!is_index_stale(root), "just-built index should not be stale");
    }

    #[test]
    fn is_index_stale_after_file_change() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();
        assert!(!is_index_stale(tmp.path()));

        // Modify a file (need to change mtime)
        std::thread::sleep(std::time::Duration::from_secs(1));
        fs::write(
            tmp.path().join("backend/PRD.md"),
            "# Updated PRD\n\nNew content added.",
        )
        .unwrap();
        assert!(
            is_index_stale(tmp.path()),
            "modified file should make index stale"
        );
    }

    #[test]
    fn is_index_stale_after_new_file() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();

        // Add a new file
        fs::write(tmp.path().join("backend/NEW.md"), "# New doc").unwrap();
        assert!(
            is_index_stale(tmp.path()),
            "new file should make index stale"
        );
    }

    #[test]
    fn ensure_index_fresh_rebuilds_when_stale() {
        let tmp = setup_indexed_root();
        // No index yet
        assert!(is_index_stale(tmp.path()));

        let rebuilt = ensure_index_fresh(tmp.path());
        assert!(rebuilt, "should have rebuilt");
        assert!(!is_index_stale(tmp.path()), "should be fresh after rebuild");
    }

    #[test]
    fn ensure_index_fresh_skips_when_fresh() {
        let root = shared_indexed_root();
        let rebuilt = ensure_index_fresh(root);
        assert!(!rebuilt, "should not rebuild when fresh");
    }

    #[test]
    fn is_index_stale_after_file_deletion() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();
        assert!(!is_index_stale(tmp.path()));

        // Delete a file
        fs::remove_file(tmp.path().join("backend/PRD.md")).unwrap();
        assert!(
            is_index_stale(tmp.path()),
            "deleted file should make index stale"
        );
    }

    #[test]
    fn sanitize_query_escapes_special_chars() {
        assert_eq!(sanitize_query("hello world"), "hello world");
        assert_eq!(sanitize_query("C++"), "C\\+\\+");
        assert_eq!(sanitize_query("test:query"), "test\\:query");
        assert_eq!(sanitize_query("(foo)"), "\\(foo\\)");
        assert_eq!(sanitize_query("a/b"), "a\\/b");
        assert_eq!(sanitize_query(""), "");
        assert_eq!(sanitize_query("   "), "");
    }

    #[test]
    fn search_indexed_special_chars_no_panic() {
        let root = shared_indexed_root();
        let result = search_indexed(root, "C++", 10, None).unwrap();
        assert!(result["matches"].is_array());
        let result = search_indexed(root, "test:query", 10, None).unwrap();
        assert!(result["matches"].is_array());
        let result = search_indexed(root, "(foo AND bar)", 10, None).unwrap();
        assert!(result["matches"].is_array());
    }

    #[test]
    fn search_indexed_xss_special_chars_no_panic() {
        let root = shared_indexed_root();
        let result = search_indexed(root, "<script>alert()</script>", 10, None).unwrap();
        assert!(result["matches"].is_array());
        let result = search_indexed(root, "foo<bar>baz", 10, None).unwrap();
        assert!(result["matches"].is_array());
        let result = search_indexed(root, r#""quoted phrase""#, 10, None).unwrap();
        assert!(result["matches"].is_array());
    }

    #[test]
    fn sanitize_query_escapes_angle_brackets_and_quotes() {
        assert_eq!(sanitize_query("<script>"), "\\<script\\>");
        assert_eq!(sanitize_query(r#""hello""#), "\\\"hello\\\"");
        assert_eq!(
            sanitize_query("<script>alert()</script>"),
            "\\<script\\>alert\\(\\)\\<\\/script\\>"
        );
    }

    #[test]
    fn search_indexed_empty_query() {
        let root = shared_indexed_root();
        let result = search_indexed(root, "", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(matches.is_empty(), "empty query should return no matches");
    }

    #[test]
    fn search_indexed_deduplicates_by_file() {
        let tmp = TempDir::new().unwrap();
        let proj = tmp.path().join("proj");
        fs::create_dir_all(&proj).unwrap();

        // Create a large file that will produce multiple chunks mentioning the same term
        let mut content = String::new();
        for i in 0..50 {
            content.push_str(&format!(
                "Line {}: The authentication system uses OAuth tokens.\n",
                i
            ));
        }
        fs::write(proj.join("BIG.md"), &content).unwrap();

        build_index_inner(tmp.path(), true).unwrap();
        let result = search_indexed(tmp.path(), "OAuth", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();

        // Should have at most 1 result per file (BIG.md), not multiple chunks
        let files: Vec<&str> = matches.iter().filter_map(|m| m["file"].as_str()).collect();
        let unique_files: std::collections::HashSet<&&str> = files.iter().collect();
        assert_eq!(
            files.len(),
            unique_files.len(),
            "results should be deduplicated by file"
        );
    }

    #[test]
    fn sanitize_query_preserves_unicode() {
        assert_eq!(sanitize_query("인증 흐름"), "인증 흐름");
        assert_eq!(sanitize_query("認証フロー"), "認証フロー");
    }

    #[test]
    fn sanitize_query_mixed_special_and_text() {
        assert_eq!(sanitize_query("user@name"), "user@name");
        assert_eq!(sanitize_query("[RFC-001]"), "\\[RFC\\-001\\]");
        assert_eq!(sanitize_query("feat!: breaking"), "feat\\!\\: breaking");
    }

    #[test]
    fn search_indexed_no_index_returns_error() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("proj")).unwrap();
        fs::write(tmp.path().join("proj/DOC.md"), "# Doc").unwrap();
        // No index built — should error
        let result = search_indexed(tmp.path(), "doc", 10, None);
        assert!(result.is_err());
    }

    #[test]
    fn search_indexed_global_scope_label() {
        let root = shared_indexed_root();
        let result = search_indexed(root, "OAuth", 10, None).unwrap();
        assert_eq!(result["scope"], "global");
        assert_eq!(result["mode"], "ranked");
    }

    #[test]
    fn search_indexed_returns_chunk_id() {
        let root = shared_indexed_root();
        let result = search_indexed(root, "OAuth", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty());
        for m in matches {
            assert!(m.get("chunk_id").is_some(), "each match must have a chunk_id field");
            assert!(m["chunk_id"].is_number(), "chunk_id must be numeric");
        }
    }

    #[test]
    fn search_indexed_project_scope_label() {
        let root = shared_indexed_root();
        let result = search_indexed(root, "OAuth", 10, Some("backend")).unwrap();
        assert_eq!(result["scope"], "project");
    }

    #[test]
    fn build_index_incremental_rebuilds_after_change() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();

        // Add new file
        fs::write(
            tmp.path().join("backend/NEW.md"),
            "# New Document\n\nFresh content here.",
        )
        .unwrap();
        let r2 = build_index_inner(tmp.path(), true).unwrap();
        assert_eq!(r2["status"], "ok");
        // The new file should be picked up (indexed >= 1)
        assert!(
            r2["indexed"].as_u64().unwrap_or(0) >= 1,
            "incremental rebuild should index new file"
        );
    }

    #[test]
    fn chunk_content_single_line() {
        let chunks = chunk_content("Single line document.", "md");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].line_start, 1);
        assert_eq!(chunks[0].text, "Single line document.");
    }

    #[test]
    fn build_index_lock_prevents_concurrent() {
        let tmp = TempDir::new().unwrap();
        let _lock_path = lock_file(tmp.path());

        assert!(!is_locked(tmp.path()));

        assert!(try_acquire_lock(tmp.path()));
        assert!(is_locked(tmp.path()));

        assert!(!try_acquire_lock(tmp.path()));

        release_lock(tmp.path());
        assert!(!is_locked(tmp.path()));

        assert!(try_acquire_lock(tmp.path()));
        release_lock(tmp.path());
    }

    #[test]
    fn index_exists_false_when_no_index() {
        let tmp = TempDir::new().unwrap();
        assert!(!index_exists(tmp.path()));
    }

    #[test]
    fn index_exists_true_after_build() {
        let root = shared_indexed_root();
        assert!(index_exists(root));
    }

    #[test]
    fn check_doc_changes_no_index() {
        let tmp = setup_indexed_root();
        let result = check_doc_changes(tmp.path());
        assert!(!result["index_exists"].as_bool().unwrap());
        assert!(result["is_stale"].as_bool().unwrap());
        // All files should be "added" since no index exists
        assert!(!result["added"].as_array().unwrap().is_empty());
        assert!(result["modified"].as_array().unwrap().is_empty());
        assert!(result["deleted"].as_array().unwrap().is_empty());
    }

    #[test]
    fn check_doc_changes_fresh_index() {
        let root = shared_indexed_root();
        let result = check_doc_changes(root);
        assert!(result["index_exists"].as_bool().unwrap());
        assert!(!result["is_stale"].as_bool().unwrap());
        assert!(result["added"].as_array().unwrap().is_empty());
        assert!(result["modified"].as_array().unwrap().is_empty());
        assert!(result["deleted"].as_array().unwrap().is_empty());
        assert!(result["unchanged_count"].as_u64().unwrap() >= 5);
    }

    #[test]
    fn check_doc_changes_after_add() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();
        fs::write(tmp.path().join("backend/NEW.md"), "# New").unwrap();
        let result = check_doc_changes(tmp.path());
        assert!(result["is_stale"].as_bool().unwrap());
        let added: Vec<&str> = result["added"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(added.iter().any(|a| a.contains("NEW.md")));
    }

    #[test]
    fn check_doc_changes_after_delete() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();
        fs::remove_file(tmp.path().join("backend/PRD.md")).unwrap();
        let result = check_doc_changes(tmp.path());
        assert!(result["is_stale"].as_bool().unwrap());
        let deleted: Vec<&str> = result["deleted"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(deleted.iter().any(|d| d.contains("PRD.md")));
    }

    #[test]
    fn check_doc_changes_after_modify() {
        let tmp = setup_indexed_root();
        build_index_inner(tmp.path(), true).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        fs::write(tmp.path().join("backend/PRD.md"), "# Updated PRD").unwrap();
        let result = check_doc_changes(tmp.path());
        assert!(result["is_stale"].as_bool().unwrap());
        let modified: Vec<&str> = result["modified"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(modified.iter().any(|m| m.contains("PRD.md")));
    }

    #[test]
    fn search_indexed_korean() {
        let tmp = TempDir::new().unwrap();
        let proj = tmp.path().join("korean");
        fs::create_dir_all(&proj).unwrap();
        fs::write(
            proj.join("PRD.md"),
            "# 제품 요구사항\n\n사용자 인증 기능이 필요합니다.\nOAuth 2.0을 사용하여 로그인을 구현합니다.",
        )
        .unwrap();

        build_index_inner(tmp.path(), true).unwrap();
        let result = search_indexed(tmp.path(), "인증", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "should find Korean text '인증'");
    }

    #[test]
    fn search_indexed_japanese() {
        let tmp = TempDir::new().unwrap();
        let proj = tmp.path().join("japanese");
        fs::create_dir_all(&proj).unwrap();
        fs::write(
            proj.join("PRD.md"),
            "# 製品要件\n\nユーザー認証機能が必要です。\nOAuth 2.0を使用してログインを実装します。",
        )
        .unwrap();

        build_index_inner(tmp.path(), true).unwrap();
        let result = search_indexed(tmp.path(), "認証", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "should find Japanese text '認証'");
    }

    #[test]
    fn search_indexed_chinese() {
        let tmp = TempDir::new().unwrap();
        let proj = tmp.path().join("chinese");
        fs::create_dir_all(&proj).unwrap();
        fs::write(
            proj.join("PRD.md"),
            "# 产品需求\n\n用户认证功能是必需的。\n使用OAuth 2.0实现登录。",
        )
        .unwrap();

        build_index_inner(tmp.path(), true).unwrap();
        let result = search_indexed(tmp.path(), "认证", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "should find Chinese text '认证'");
    }

    #[cfg(feature = "alcove-full")]
    #[test]
    fn search_hybrid_returns_bm25_only_when_embedding_not_ready() {
        use crate::embedding::EmbeddingService;
        use crate::config::EmbeddingConfig;

        let root = shared_indexed_root();
        let svc = EmbeddingService::new(EmbeddingConfig {
            model: "snowflake-arctic-embed-xs".to_string(),
            auto_download: false,
            cache_dir: root.join(".alcove/models").to_string_lossy().to_string(),
            enabled: false,
            query_cache_size: 64,
        });

        let result = search_hybrid(root, "OAuth", &svc, 5, None).unwrap();
        assert_eq!(result["mode"], "bm25-only", "expected bm25-only when embedding is not ready");
    }

    // B4: vector store open/search errors must be surfaced in embedding_status,
    // not silently swallowed while still reporting mode = "hybrid-bm25-vector".
    #[cfg(feature = "alcove-full")]
    #[test]
    fn search_hybrid_vector_store_error_reflected_in_embedding_status() {
        use crate::embedding::EmbeddingService;
        use crate::config::EmbeddingConfig;

        let root = shared_indexed_root();
        let svc = EmbeddingService::new(EmbeddingConfig {
            model: "snowflake-arctic-embed-xs".to_string(),
            auto_download: false,
            cache_dir: root.join(".alcove/models").to_string_lossy().to_string(),
            enabled: true,
            query_cache_size: 64,
        });

        let result = search_hybrid(root, "OAuth", &svc, 5, None).unwrap();
        let mode = result["mode"].as_str().unwrap_or("");
        let status = result["embedding_status"].as_str().unwrap_or("");
        if mode == "hybrid-bm25-vector" {
            assert_eq!(status, "ready");
        } else {
            assert_ne!(status, "ready",
                "embedding_status should reflect the failure reason, not 'ready'");
        }
    }

    #[test]
    fn search_indexed_skips_underscore_prefixed_files() {
        let tmp = TempDir::new().unwrap();
        let proj = tmp.path().join("myproject");
        fs::create_dir_all(&proj).unwrap();
        fs::write(
            proj.join("PRD.md"),
            "# PRD\n\nThis project uses OAuth 2.0 authentication flow.",
        )
        .unwrap();
        fs::write(
            proj.join("_template.md"),
            "# Template\n\nOAuth placeholder for template files.",
        )
        .unwrap();

        build_index_inner(tmp.path(), true).unwrap();

        let result = search_indexed(tmp.path(), "OAuth", 10, None).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "should find OAuth in PRD.md");

        for m in matches {
            let file = m["file"].as_str().unwrap_or("");
            assert!(
                !file.contains("_template.md"),
                "_template.md should be excluded from search results, but found: {file}"
            );
        }
    }

    /// Ignored in normal test runs because IndexReader cache is bypassed in test
    /// builds. Run with `cargo test -- --ignored` to verify cache behaviour.
    #[test]
    #[ignore = "IndexReader cache is bypassed in test builds; run with --ignored to verify"]
    fn test_project_vault_cache_isolation() {
        // Build two separate indexes in different temp dirs.
        let tmp_project = TempDir::new().unwrap();
        let tmp_vault = TempDir::new().unwrap();

        // Create minimal index content for project.
        let proj_docs = tmp_project.path().join("docs");
        fs::create_dir_all(&proj_docs).unwrap();
        fs::write(proj_docs.join("proj.md"), "# Project doc\nHello project world.\n").unwrap();

        // Create minimal index content for vault.
        let vault_docs = tmp_vault.path().join("docs");
        fs::create_dir_all(&vault_docs).unwrap();
        fs::write(vault_docs.join("vault.md"), "# Vault doc\nHello vault world.\n").unwrap();

        // Build indexes.
        let proj_index_dir = index_dir(tmp_project.path());
        let vault_index_dir = index_dir(tmp_vault.path());

        build_index(tmp_project.path()).expect("project index build");
        build_index(tmp_vault.path()).expect("vault index build");

        // Open indexes and get cached readers with different categories.
        use tantivy::Index;
        let proj_index = Index::open_in_dir(&proj_index_dir).expect("open project index");
        let vault_index = Index::open_in_dir(&vault_index_dir).expect("open vault index");

        let _proj_reader = get_cached_reader(&proj_index_dir, &proj_index, CacheCategory::Project)
            .expect("project reader");
        let _vault_reader = get_cached_reader(&vault_index_dir, &vault_index, CacheCategory::Vault)
            .expect("vault reader");

        // Verify isolation: project cache has the project entry but not the vault entry.
        {
            let proj_cache = reader_cache_for(CacheCategory::Project).lock().unwrap();
            assert!(proj_cache.contains_key(&proj_index_dir), "project cache must contain project index");
            assert!(!proj_cache.contains_key(&vault_index_dir), "project cache must not contain vault index");
        }

        // Verify isolation: vault cache has the vault entry but not the project entry.
        {
            let vault_cache = reader_cache_for(CacheCategory::Vault).lock().unwrap();
            assert!(vault_cache.contains_key(&vault_index_dir), "vault cache must contain vault index");
            assert!(!vault_cache.contains_key(&proj_index_dir), "vault cache must not contain project index");
        }
    }

    #[cfg(feature = "alcove-full")]
    #[test]
    fn read_file_content_pdf_does_not_hang() {
        use std::time::{Duration, Instant};
        let tmp = tempfile::TempDir::new().unwrap();
        // Write a minimal valid-looking but actually broken PDF so pdftotext exits fast.
        let pdf_path = tmp.path().join("broken.pdf");
        std::fs::write(&pdf_path, b"%PDF-1.4 broken content").unwrap();
        let start = Instant::now();
        let _ = read_file_content(&pdf_path);
        assert!(
            start.elapsed() < Duration::from_secs(35),
            "read_file_content must not block longer than the 30s timeout"
        );
    }

    // -- Vault indexing tests --

    fn setup_vault_dir(tmp: &TempDir) -> std::path::PathBuf {
        let vault = tmp.path().join("my-vault");
        fs::create_dir_all(&vault).unwrap();
        fs::write(
            vault.join("note1.md"),
            "# Note One\n\nThis is about OAuth 2.0 authentication.\nPKCE flow for public clients.",
        )
        .unwrap();
        fs::write(
            vault.join("note2.md"),
            "# Note Two\n\nKubernetes deployment strategies.\nRolling updates and blue-green deployments.",
        )
        .unwrap();
        fs::write(
            vault.join("readme.txt"),
            "This is a text file, not markdown.",
        )
        .unwrap();
        vault
    }

    #[test]
    fn test_build_vault_index() {
        let tmp = TempDir::new().unwrap();
        let vault = setup_vault_dir(&tmp);

        let result = build_vault_index(&vault).unwrap();
        assert_eq!(result["status"], "ok");
        assert_eq!(result["vault"], "my-vault");
        assert_eq!(result["files"].as_u64().unwrap(), 2);
        assert!(vault.join(".alcove/index").exists());
    }

    #[test]
    fn test_search_vault() {
        let tmp = TempDir::new().unwrap();
        let vault = setup_vault_dir(&tmp);

        build_vault_index(&vault).unwrap();

        let result = search_vault(&vault, "OAuth", 10).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(!matches.is_empty(), "should find OAuth matches in vault");

        for m in matches {
            assert_eq!(m["project"], "my-vault");
            assert!(m["score"].as_f64().unwrap() > 0.0);
        }
    }

    #[test]
    fn test_vault_index_excludes_underscore_files() {
        let tmp = TempDir::new().unwrap();
        let vault = setup_vault_dir(&tmp);

        fs::write(
            vault.join("_template.md"),
            "# Template\n\nOAuth placeholder template content.",
        )
        .unwrap();

        let result = build_vault_index(&vault).unwrap();
        assert_eq!(result["files"].as_u64().unwrap(), 2);

        let search_result = search_vault(&vault, "OAuth", 10).unwrap();
        let matches = search_result["matches"].as_array().unwrap();
        for m in matches {
            let file = m["file"].as_str().unwrap_or("");
            assert!(
                !file.contains("_template.md"),
                "_template.md should be excluded from vault search results"
            );
        }
    }

    #[test]
    fn test_rebuild_vault_index() {
        let tmp = TempDir::new().unwrap();
        let vault = setup_vault_dir(&tmp);

        build_vault_index(&vault).unwrap();

        fs::write(
            vault.join("note3.md"),
            "# Note Three\n\nNew content about gRPC services.",
        )
        .unwrap();

        let result = rebuild_vault_index(&vault).unwrap();
        assert_eq!(result["status"], "ok");
        assert_eq!(result["files"].as_u64().unwrap(), 3);
    }

    #[test]
    fn test_search_vault_empty_query() {
        let tmp = TempDir::new().unwrap();
        let vault = setup_vault_dir(&tmp);
        build_vault_index(&vault).unwrap();

        let result = search_vault(&vault, "", 10).unwrap();
        let matches = result["matches"].as_array().unwrap();
        assert!(matches.is_empty(), "empty query should return no matches");
    }

    #[test]
    fn test_search_vault_no_index_returns_error() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("empty-vault");
        fs::create_dir_all(&vault).unwrap();
        fs::write(vault.join("note.md"), "# Note").unwrap();

        let result = search_vault(&vault, "note", 10);
        assert!(result.is_err(), "search on unindexed vault should error");
    }

    #[test]
    fn test_vault_index_excludes_alcove_dir() {
        let tmp = TempDir::new().unwrap();
        let vault = setup_vault_dir(&tmp);

        build_vault_index(&vault).unwrap();

        fs::write(
            vault.join(".alcove/internal.md"),
            "# Internal\n\nThis should not be indexed.",
        )
        .unwrap();

        let result = rebuild_vault_index(&vault).unwrap();
        assert_eq!(result["files"].as_u64().unwrap(), 2);
    }

    // -----------------------------------------------------------------------
    // Tests for extract_title
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_title_h1() {
        assert_eq!(extract_title("# My Title\nSome text", "doc.md", 0), "My Title");
    }

    #[test]
    fn test_extract_title_h2() {
        assert_eq!(extract_title("## Sub Title\nSome text", "doc.md", 0), "Sub Title");
    }

    #[test]
    fn test_extract_title_h3() {
        assert_eq!(extract_title("### Deep Title\nSome text", "doc.md", 0), "Deep Title");
    }

    #[test]
    fn test_extract_title_no_heading_falls_back_to_filename() {
        assert_eq!(extract_title("Just some text\nNo heading here", "README.md", 0), "README");
    }

    #[test]
    fn test_extract_title_non_first_chunk_uses_filename() {
        assert_eq!(
            extract_title("# Heading Ignored\nSome text", "GUIDE.md", 1),
            "GUIDE"
        );
    }

    #[test]
    fn test_extract_title_filename_with_multiple_dots() {
        assert_eq!(extract_title("text", "ARCHITECTURE.md", 0), "ARCHITECTURE");
    }

    #[test]
    fn test_extract_title_heading_after_blank_lines() {
        let content = "\n\n## Late Heading\nBody text";
        assert_eq!(extract_title(content, "doc.md", 0), "Late Heading");
    }

    #[test]
    fn test_extract_title_heading_with_extra_whitespace() {
        assert_eq!(extract_title("  #   Spaced Title  \nBody", "doc.md", 0), "Spaced Title");
    }

    // -----------------------------------------------------------------------
    // Tests for build_search_query
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_search_query_returns_non_null() {
        use tantivy::schema::{Schema, TEXT};
        use tantivy::{Index, TantivyDocument};
        use tantivy::collector::TopDocs;
        use tantivy::{DocAddress, Score};

        let mut schema_builder = Schema::builder();
        let body_field = schema_builder.add_text_field("body", TEXT);
        let title_field = schema_builder.add_text_field("title", TEXT);
        let filename_field = schema_builder.add_text_field("filename", TEXT);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema);
        register_ngram_tokenizer(&index).unwrap();

        let mut index_writer = index.writer(15_000_000).unwrap();
        let mut doc = TantivyDocument::new();
        doc.add_text(body_field, "authentication OAuth token");
        doc.add_text(title_field, "Auth Guide");
        doc.add_text(filename_field, "auth-guide.md");
        index_writer.add_document(doc).unwrap();
        index_writer.commit().unwrap();

        let query = build_search_query("OAuth", &index, body_field, title_field, filename_field);

        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let top_docs: Vec<(Score, DocAddress)> = searcher
            .search(&query, &TopDocs::with_limit(10).order_by_score())
            .unwrap();
        assert!(!top_docs.is_empty(), "query should match the indexed document");
    }

    #[test]
    fn test_build_search_query_empty_sanitized() {
        use tantivy::schema::{Schema, TEXT};
        use tantivy::Index;
        use tantivy::collector::TopDocs;
        use tantivy::{DocAddress, Score};

        let mut schema_builder = Schema::builder();
        let body_field = schema_builder.add_text_field("body", TEXT);
        let title_field = schema_builder.add_text_field("title", TEXT);
        let filename_field = schema_builder.add_text_field("filename", TEXT);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema);
        register_ngram_tokenizer(&index).unwrap();

        let query = build_search_query("", &index, body_field, title_field, filename_field);
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let top_docs: Vec<(Score, DocAddress)> = searcher
            .search(&query, &TopDocs::with_limit(10).order_by_score())
            .unwrap();
        assert!(top_docs.is_empty(), "empty query should match nothing");
    }

    // -----------------------------------------------------------------------
    // Tests for apply_project_diversity
    // -----------------------------------------------------------------------

    #[test]
    fn test_apply_project_diversity_limits_per_project() {
        use serde_json::json;
        let results = vec![
            json!({"project": "backend", "text": "a"}),
            json!({"project": "backend", "text": "b"}),
            json!({"project": "backend", "text": "c"}),
            json!({"project": "frontend", "text": "d"}),
            json!({"project": "frontend", "text": "e"}),
        ];
        let mut results = results;
        apply_project_diversity(&mut results, 2);

        let backend_count = results.iter().filter(|r| r["project"] == "backend").count();
        let frontend_count = results.iter().filter(|r| r["project"] == "frontend").count();
        assert_eq!(backend_count, 2, "backend should be capped at 2");
        assert_eq!(frontend_count, 2, "frontend should be capped at 2");
        assert_eq!(results.len(), 4);
    }

    #[test]
    fn test_apply_project_diversity_no_filter_needed() {
        use serde_json::json;
        let results = vec![
            json!({"project": "backend", "text": "a"}),
            json!({"project": "frontend", "text": "b"}),
        ];
        let mut results = results;
        apply_project_diversity(&mut results, 2);
        assert_eq!(results.len(), 2, "under-limit results should be unchanged");
    }

    #[test]
    fn test_apply_project_diversity_empty_input() {
        let mut results: Vec<serde_json::Value> = vec![];
        apply_project_diversity(&mut results, 2);
        assert!(results.is_empty());
    }

    #[test]
    fn test_apply_project_diversity_single_project() {
        use serde_json::json;
        let results = vec![
            json!({"project": "mono", "text": "a"}),
            json!({"project": "mono", "text": "b"}),
            json!({"project": "mono", "text": "c"}),
            json!({"project": "mono", "text": "d"}),
        ];
        let mut results = results;
        apply_project_diversity(&mut results, 1);
        assert_eq!(results.len(), 1, "max_per_project=1 should keep only 1");
    }

    #[test]
    fn test_apply_project_diversity_preserves_order() {
        use serde_json::json;
        let results = vec![
            json!({"project": "a", "val": 1}),
            json!({"project": "b", "val": 2}),
            json!({"project": "a", "val": 3}),
            json!({"project": "a", "val": 4}),
            json!({"project": "b", "val": 5}),
            json!({"project": "b", "val": 6}),
        ];
        let mut results = results;
        apply_project_diversity(&mut results, 2);

        let vals: Vec<i64> = results.iter().filter_map(|r| r["val"].as_i64()).collect();
        assert_eq!(vals, vec![1, 2, 3, 5]);
    }

    #[test]
    fn test_apply_project_diversity_missing_project_field() {
        use serde_json::json;
        let results = vec![
            json!({"project": "", "text": "a"}),
            json!({"project": "", "text": "b"}),
            json!({"project": "", "text": "c"}),
        ];
        let mut results = results;
        apply_project_diversity(&mut results, 2);
        assert_eq!(results.len(), 2, "empty-string project should also be capped");
    }

    // -----------------------------------------------------------------------
    // Tests for schema version check
    // -----------------------------------------------------------------------

    #[test]
    fn test_schema_version_stale_when_older() {
        use std::collections::HashMap;
        let old_meta = IndexMeta {
            files: HashMap::new(),
            schema_version: 1,
        };
        assert!(
            old_meta.schema_version < SCHEMA_VERSION,
            "version 1 should be considered stale compared to SCHEMA_VERSION={}",
            SCHEMA_VERSION
        );
    }

    #[test]
    fn test_schema_version_current_is_not_stale() {
        use std::collections::HashMap;
        let current_meta = IndexMeta {
            files: HashMap::new(),
            schema_version: SCHEMA_VERSION,
        };
        assert!(
            current_meta.schema_version >= SCHEMA_VERSION,
            "current version should not be considered stale"
        );
    }

    #[test]
    fn test_schema_version_default_is_stale() {
        let default_meta = IndexMeta::default();
        assert!(
            default_meta.schema_version < SCHEMA_VERSION,
            "default IndexMeta (version 0) should be stale"
        );
    }

    // -----------------------------------------------------------------------
    // Tests for IndexSchema struct (T4 refactoring)
    // -----------------------------------------------------------------------

    #[test]
    fn test_index_schema_build_returns_all_fields() {
        let s = IndexSchema::build();
        let schema = &s.schema;
        assert!(schema.get_field("project").is_ok(), "schema must have 'project' field");
        assert!(schema.get_field("file").is_ok(), "schema must have 'file' field");
        assert!(schema.get_field("filename").is_ok(), "schema must have 'filename' field");
        assert!(schema.get_field("title").is_ok(), "schema must have 'title' field");
        assert!(schema.get_field("chunk_id").is_ok(), "schema must have 'chunk_id' field");
        assert!(schema.get_field("body").is_ok(), "schema must have 'body' field");
        assert!(schema.get_field("line_start").is_ok(), "schema must have 'line_start' field");
    }

    #[test]
    fn test_index_schema_fields_match_schema() {
        let s = IndexSchema::build();
        assert_eq!(s.project, s.schema.get_field("project").unwrap());
        assert_eq!(s.file,    s.schema.get_field("file").unwrap());
        assert_eq!(s.filename, s.schema.get_field("filename").unwrap());
        assert_eq!(s.title,   s.schema.get_field("title").unwrap());
        assert_eq!(s.chunk_id, s.schema.get_field("chunk_id").unwrap());
        assert_eq!(s.body,    s.schema.get_field("body").unwrap());
        assert_eq!(s.line_start, s.schema.get_field("line_start").unwrap());
    }
}
