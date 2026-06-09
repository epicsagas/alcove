# eval-project Benchmark Strategy

## Overview

The benchmark system measures search quality against manually curated query sets. Each benchmark run evaluates how well the search engine retrieves relevant documents for a set of test queries, producing quality metrics that can be tracked over time.

## Benchmark Suite Structure

A benchmark suite is a TOML file containing:

- A list of test queries with expected relevant document paths.
- Optional relevance grades (0-3) for graded metrics.
- Configuration overrides for search parameters during the run.

```toml
name = "architecture-queries"
description = "Queries about system architecture and design"

[[queries]]
query = "how does the search engine work"
relevant = [
  "ARCHITECTURE.md",
  "DECISIONS.md",
]

[[queries]]
query = "검색 엔진 내부 동작 원리"
relevant = [
  "ARCHITECTURE.md",
]
```

## Quality Metrics

### Precision@k

Precision@k measures the fraction of retrieved results that are relevant. For k=5, if 3 of the top 5 results are relevant, Precision@5 = 0.6.

This is the primary metric for evaluating result quality at the top of the ranking, where users are most likely to engage.

### NDCG (Normalized Discounted Cumulative Gain)

NDCG accounts for the position of relevant results in the ranked list. Higher relevance grades at top positions contribute more to the score. The metric is normalized against the ideal ranking, producing values between 0 and 1.

NDCG is computed as:

```
DCG@k = Σ (2^rel_i - 1) / log2(i + 1)
NDCG@k = DCG@k / IDCG@k
```

Where `rel_i` is the relevance grade of the result at position `i`.

### MRR (Mean Reciprocal Rank)

MRR evaluates how quickly the first relevant result appears. For each query, the reciprocal rank of the first relevant result is recorded. The mean across all queries gives the MRR.

```
MRR = (1/|Q|) * Σ 1/rank_i
```

Where `rank_i` is the position of the first relevant result for query `i`.

## 벤치마크 품질

벤치마크 품질 관리는 지속적인 과정입니다. 각 릴리스마다 벤치마크 스위트를 실행하여 검색 품질의 회귀를 감지합니다.

### 기준선 설정

v0.11을 기준선으로 설정했습니다. 이 버전에서의 측정값은 다음과 같습니다:

- Precision@5: 0.72
- NDCG@10: 0.68
- MRR: 0.81

이 값들은 향후 변경 사항이 검색 품질을 저하시키지 않는지 확인하는 기준점 역할을 합니다.

### 품질 게이트

CI 파이프라인은 다음 조건을 품질 게이트로 사용합니다:

- Precision@5 >= 0.70 (기준선 대비 2% 이하 하락 허용)
- NDCG@10 >= 0.65
- MRR >= 0.75

게이트를 통과하지 못하면 PR이 병합되지 않습니다.

## Running Benchmarks

### CLI Commands

```bash
# Run all benchmark suites
eval-project bench run

# Run a specific suite
eval-project bench run --suite architecture-queries

# Compare two runs
eval-project bench compare --baseline run-001 --candidate run-002

# Generate quality report
eval-project bench report --format markdown --output report.md
```

### Continuous Integration

Benchmarks run automatically on every pull request that modifies search-related code (index, search, or ingest modules). Results are posted as PR comments showing metric changes relative to the main branch.

## Corpus Design Principles

- Benchmark corpora must use synthetic content, never real project documentation.
- Query sets should cover keyword queries, natural language queries, and CJK queries.
- Each query must have at least one relevant document and at most five.
- Relevance judgments should be made by at least two reviewers.
