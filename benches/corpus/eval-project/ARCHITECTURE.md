# eval-project Architecture

eval-project is a private documentation indexing server designed to provide fast, accurate search across project documentation vaults. It exposes an MCP (Model Context Protocol) server interface so that AI agents and IDE integrations can query documentation context in real time.

## System Overview

The server follows a three-layer architecture:

1. **Ingestion Layer** -- Watches configured vault directories for file changes, parses Markdown into structured chunks, and writes them to the search index.
2. **Index Layer** -- Combines a Tantivy BM25 full-text index with a vector embedding store for hybrid retrieval.
3. **Serving Layer** -- Exposes MCP tool endpoints (`search`, `get_document`, `list_projects`) over stdio and HTTP transports.

All three layers run in a single process to minimize operational overhead. The index is persisted as a directory of segment files on local disk.

## MCP Server Protocol

eval-project implements the MCP specification version 2025-03-26. It registers the following tools:

- `search(query, project?, limit?)` -- Perform a hybrid search across the documentation index and return ranked results with relevance scores.
- `get_document(path)` -- Retrieve the full content of a specific document by its path within a vault.
- `list_projects()` -- Return the list of configured project vaults and their metadata.
- `rebuild_index(project?)` -- Trigger a full re-index of one or all projects.

The server communicates over stdio by default, which is the recommended transport for local agent integrations. An optional HTTP+SSE transport is available for remote or multi-client deployments.

## 검색 엔진 세부

검색 엔진은 Tantivy 기반의 BM25 전문 검색과 벡터 임베딩 유사도 검색을 결합한 하이브리드 방식을 사용합니다. 사용자가 쿼리를 입력하면 두 검색 경로가 병렬로 실행됩니다.

### 청킹 전략

문서는 마크다운 헤딩(##, ###)을 기준으로 청크로 분할됩니다. 각 청크는 헤딩 계층 구조를 메타데이터로 보존하여, 검색 결과가 문서 내의 정확한 위치를 가리킬 수 있도록 합니다. 최소 청크 크기는 100자, 최대 크기는 2000자입니다.

### RRF (Reciprocal Rank Fusion)

BM25 순위와 벡터 유사도 순위는 Reciprocal Rank Fusion 알고리즘으로 병합됩니다. 기본 k 파라미터는 60이며, 이 값은 실험적으로 최적화되었습니다. RRF는 두 순위의 상호 보완성을 활용하여 키워드 매칭과 의미적 유사도를 균형 있게 결합합니다.

### 벡터 임베딩

텍스트 청크는 설정에서 지정한 임베딩 모델을 통해 벡터화됩니다. 기본 모델은 `text-embedding-3-small`이며, Ollama를 통한 로컬 모델 실행도 지원합니다. 임베딩 차원은 1536차원이며, 코사인 유사도를 거리 척도로 사용합니다.

## Search Engine Details

The full-text search component is powered by Tantivy, a high-performance Rust search engine library. Tantivy provides:

- **Inverted index** with term frequency and document length normalization.
- **BM25 scoring** with configurable k1 (default 1.2) and b (default 0.75) parameters.
- **Custom tokenizers** for CJK text, including character bigram fallback for Korean and Chinese content.
- **Field-level boosts** that allow weighting title and heading fields higher than body text.

The index writer uses a batch commit strategy. Documents are buffered in memory and flushed to disk every 30 seconds or when the buffer exceeds 64 MB, whichever comes first. This balances indexing throughput with query freshness.

## Heading-Based Chunking

Documents are split into chunks at every Markdown heading boundary (lines beginning with `#`, `##`, or `###`). Each chunk carries the following metadata:

- `heading_path`: The full heading hierarchy as a slash-separated string (e.g., "Architecture/Search Engine Details").
- `level`: The heading depth (1-6).
- `source_path`: The original file path relative to the vault root.
- `project`: The project identifier this document belongs to.

This approach ensures that search results map back to meaningful sections rather than arbitrary text windows.

## Hybrid Search with RRF

The hybrid search pipeline executes both retrieval paths in parallel using tokio tasks:

```
Query → [BM25 Branch] → Rank List A
      → [Vector Branch] → Rank List B
      → RRF Merge → Final Results
```

The Reciprocal Rank Fusion formula is:

```
score(d) = Σ 1/(k + rank_i(d))
```

Where `k` is a smoothing constant (default 60) and `rank_i(d)` is the rank of document `d` in retrieval path `i`. This method requires no score normalization across paths, which makes it robust to different score distributions.

## Data Flow

1. File watcher detects a change in a vault directory.
2. Parser reads the Markdown file and splits it into heading-based chunks.
3. Each chunk is tokenized for the BM25 index and embedded for the vector store.
4. Both indexes are updated atomically within a single commit.
5. Search queries hit both indexes concurrently and merge results via RRF.

## Performance Characteristics

On a corpus of 5,000 documents (approximately 50 MB of raw text), the system achieves:

- Index build time: under 10 seconds.
- Query latency (p99): under 50 ms for hybrid search.
- Memory footprint: approximately 200 MB including vector index.
- Index size on disk: approximately 120 MB.
