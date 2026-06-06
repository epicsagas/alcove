# Embedding Models

Alcove uses [fastembed-rs](https://github.com/Anush008/fastembed-rs) (ONNX Runtime) for local vector embeddings. Models are downloaded on first use and cached under `~/.alcove/models/`.

Set a model via CLI:

```bash
alcove model set <ModelName>
```

Or in `~/.alcove/config.toml`:

```toml
embedding_model = "BGEM3"
```

## Curated Models

These are the models shown by `alcove model list` — curated for size, quality, and language coverage.

| Variable | Dim | Context | Size | Languages | Best for |
|----------|-----|---------|------|-----------|----------|
| `ArcticEmbedXS` (default) | 384 | 512 | 90 MB | Multilingual (partial) | Best size/quality ratio |
| `ArcticEmbedXSQ` | 384 | 512 | 90 MB | Multilingual (partial) | Quantized, smaller download |
| `MultilingualE5Small` | 384 | 512 | 470 MB | 100+ languages | Korean/CJK best quality |
| `BGEM3` | 1024 | 8192 | 600 MB | 100+ languages | Premium — Dense+Sparse+ColBERT |
| `ArcticEmbedMLong` | 768 | 8192 | 430 MB | Multilingual (partial) | Long documents |
| `JinaEmbeddingsV2BaseCode` | 768 | 8192 | 550 MB | Code + English | Code-optimized |

## All Supported Models

Any model below can be set in config or via `alcove model set <Variable>`.

### Sentence Transformers

| Variable | Dim | Context | Size | Languages | Prefix |
|----------|-----|---------|------|-----------|--------|
| `AllMiniLML6V2` | 384 | 256 | 80 MB | English | — |
| `AllMiniLML6V2Q` | 384 | 256 | 80 MB | English | — |
| `AllMiniLML12V2` | 384 | 256 | 120 MB | English | — |
| `AllMiniLML12V2Q` | 384 | 256 | 120 MB | English | — |
| `AllMpnetBaseV2` | 768 | 384 | 420 MB | English | — |

### E5 — Multilingual

| Variable | Dim | Context | Size | Languages | Prefix |
|----------|-----|---------|------|-----------|--------|
| `MultilingualE5Small` | 384 | 512 | 470 MB | 100+ | `query:` / `passage:` |
| `MultilingualE5Base` | 768 | 512 | 1.1 GB | 100+ | `query:` / `passage:` |
| `MultilingualE5Large` | 1024 | 512 | 2.2 GB | 100+ | `query:` / `passage:` |

> **Prefix**: E5 models prepend `query: ` to search queries and `passage: ` to indexed documents for best results. Alcove handles this automatically.

### BGE — English

| Variable | Dim | Context | Size | Languages | Prefix |
|----------|-----|---------|------|-----------|--------|
| `BGESmallENV15` | 384 | 512 | 130 MB | English | — |
| `BGESmallENV15Q` | 384 | 512 | 40 MB | English | — |
| `BGEBaseENV15` | 768 | 512 | 430 MB | English | — |
| `BGEBaseENV15Q` | 768 | 512 | 130 MB | English | — |
| `BGELargeENV15` | 1024 | 512 | 1.3 GB | English | — |
| `BGELargeENV15Q` | 1024 | 512 | 400 MB | English | — |

### BGE — Chinese

| Variable | Dim | Context | Size | Languages | Prefix |
|----------|-----|---------|------|-----------|--------|
| `BGESmallZHV15` | 512 | 512 | 100 MB | Chinese | — |
| `BGELargeZHV15` | 1024 | 512 | 1.3 GB | Chinese | — |

### BGE-M3 — Multilingual Flagship

| Variable | Dim | Context | Size | Languages | Prefix |
|----------|-----|---------|------|-----------|--------|
| `BGEM3` | 1024 | 8192 | 600 MB | 100+ | — |

> **BGEM3** produces Dense + Sparse + ColBERT vectors. Best quality among multilingual models.

### Arctic Embed — Snowflake

| Variable | Dim | Context | Size | Languages | Prefix |
|----------|-----|---------|------|-----------|--------|
| `ArcticEmbedXS` | 384 | 512 | 90 MB | Multilingual (partial) | query prefix † |
| `ArcticEmbedXSQ` | 384 | 512 | 90 MB | Multilingual (partial) | query prefix † |
| `ArcticEmbedS` | 384 | 512 | 130 MB | Multilingual (partial) | query prefix † |
| `ArcticEmbedSQ` | 384 | 512 | 130 MB | Multilingual (partial) | query prefix † |
| `ArcticEmbedM` | 768 | 512 | 430 MB | Multilingual (partial) | query prefix † |
| `ArcticEmbedMQ` | 768 | 512 | 430 MB | Multilingual (partial) | query prefix † |
| `ArcticEmbedMLong` | 768 | 8192 | 430 MB | Multilingual (partial) | query prefix † |
| `ArcticEmbedMLongQ` | 768 | 8192 | 430 MB | Multilingual (partial) | query prefix † |
| `ArcticEmbedL` | 1024 | 512 | 1.3 GB | Multilingual (partial) | query prefix † |
| `ArcticEmbedLQ` | 1024 | 512 | 1.3 GB | Multilingual (partial) | query prefix † |

> † Arctic models prepend `"Represent this sentence for searching relevant passages: "` to queries. Alcove handles this automatically.

### Nomic

| Variable | Dim | Context | Size | Languages | Prefix |
|----------|-----|---------|------|-----------|--------|
| `NomicEmbedTextV1` | 768 | 8192 | 550 MB | English | — |
| `NomicEmbedTextV15` | 768 | 8192 | 550 MB | English | `search_query:` / `search_document:` |
| `NomicEmbedTextV15Q` | 768 | 8192 | 550 MB | English | `search_query:` / `search_document:` |

### GTE — Alibaba

| Variable | Dim | Context | Size | Languages | Prefix |
|----------|-----|---------|------|-----------|--------|
| `GTEBaseENV15` | 768 | 512 | 430 MB | English | — |
| `GTEBaseENV15Q` | 768 | 512 | 130 MB | English | — |
| `GTELargeENV15` | 1024 | 512 | 1.3 GB | English | — |
| `GTELargeENV15Q` | 1024 | 512 | 400 MB | English | — |

### Other

| Variable | Dim | Context | Size | Languages | Prefix |
|----------|-----|---------|------|-----------|--------|
| `ModernBertEmbedLarge` | 1024 | 512 | 600 MB | English | — |
| `MxbaiEmbedLargeV1` | 1024 | 512 | 670 MB | English | — |
| `MxbaiEmbedLargeV1Q` | 1024 | 512 | 200 MB | English | — |
| `ParaphraseMLMiniLML12V2` | 384 | 512 | 420 MB | Multilingual | — |
| `ParaphraseMLMiniLML12V2Q` | 384 | 512 | 130 MB | Multilingual | — |
| `ParaphraseMLMpnetBaseV2` | 768 | 512 | 1.1 GB | Multilingual | — |
| `JinaEmbeddingsV2BaseCode` | 768 | 8192 | 550 MB | Code + English | — |
| `JinaEmbeddingsV2BaseEN` | 768 | 8192 | 550 MB | English | — |
| `EmbeddingGemma300M` | 768 | 8192 | 600 MB | English | — |

## Columns

| Column | Description |
|--------|-------------|
| **Variable** | Name used in `config.toml` and `alcove model set` |
| **Dim** | Embedding vector dimension |
| **Context** | Max token length per input |
| **Size** | Approximate download size (ONNX format) |
| **Languages** | Supported language scope |
| **Prefix** | Query/document prefix applied automatically by Alcove |

## Changing Models

After changing the model, rebuild the index:

```bash
alcove model set BGEM3
alcove index /path/to/docs
```

> **Note**: Switching to a model with a different dimension requires re-indexing. Alcove will prompt you automatically.
