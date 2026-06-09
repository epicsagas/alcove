# eval-project Environment Variables and Configuration Reference

## Configuration File

The primary configuration file is `eval-project.toml` in the working directory. It defines vault paths, embedding model settings, and server options.

## Environment Variables

### Server Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `EVAL_PROJECT_CONFIG` | `./eval-project.toml` | Path to the TOML configuration file. |
| `EVAL_PROJECT_BIND` | `127.0.0.1:8080` | HTTP bind address when using HTTP transport. |
| `EVAL_PROJECT_LOG_LEVEL` | `info` | Minimum log level (trace, debug, info, warn, error). |
| `EVAL_PROJECT_TELEMETRY` | `on` | Enable or disable telemetry collection (on/off). |

### Embedding Model Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `EVAL_PROJECT_EMBEDDING_PROVIDER` | `openai` | Embedding backend: `openai` or `ollama`. |
| `EVAL_PROJECT_EMBEDDING_MODEL` | `text-embedding-3-small` | Model name for the selected provider. |
| `EVAL_PROJECT_EMBEDDING_DIMENSIONS` | `1536` | Output dimension size for embeddings. |
| `EVAL_PROJECT_EMBEDDING_BASE_URL` | (provider default) | Custom API endpoint URL. |

### API Keys

| Variable | Required | Description |
|----------|----------|-------------|
| `EVAL_PROJECT_OPENAI_API_KEY` | When provider is `openai` | API key for OpenAI embedding requests. |
| `EVAL_PROJECT_OLLAMA_HOST` | When provider is `ollama` | Ollama server host (default: `http://localhost:11434`). |

### Index Storage

| Variable | Default | Description |
|----------|---------|-------------|
| `EVAL_PROJECT_INDEX_DIR` | `./index` | Directory for Tantivy index segments and metadata. |
| `EVAL_PROJECT_COMMIT_INTERVAL_SECS` | `30` | Seconds between automatic index commits. |
| `EVAL_PROJECT_COMMIT_BUFFER_BYTES` | `67108864` | Maximum buffer size before forced commit (64 MB). |

### Telemetry Endpoints

| Variable | Default | Description |
|----------|---------|-------------|
| `EVAL_PROJECT_POSTHOG_KEY` | (none) | PostHog project API key for event tracking. |
| `EVAL_PROJECT_POSTHOG_HOST` | `https://us.i.posthog.com` | PostHog ingestion endpoint. |
| `EVAL_PROJECT_SENTRY_DSN` | (none) | Sentry DSN for error and crash reporting. |

### Search Tuning

| Variable | Default | Description |
|----------|---------|-------------|
| `EVAL_PROJECT_RRF_K` | `60` | Reciprocal Rank Fusion smoothing constant. |
| `EVAL_PROJECT_BM25_K1` | `1.2` | BM25 term frequency saturation parameter. |
| `EVAL_PROJECT_BM25_B` | `0.75` | BM25 document length normalization parameter. |
| `EVAL_PROJECT_DEFAULT_LIMIT` | `10` | Default number of results returned by search. |
| `EVAL_PROJECT_MAX_LIMIT` | `50` | Maximum number of results per query. |

## Configuration File Structure

```toml
[server]
transport = "stdio"          # "stdio" or "http"
bind = "127.0.0.1:8080"

[embedding]
provider = "openai"
model = "text-embedding-3-small"
dimensions = 1536

[[projects]]
name = "my-project"
path = "/path/to/docs"
include = ["*.md"]
exclude = ["drafts/**"]

[index]
dir = "./index"
commit_interval_secs = 30
```

## Security Notes

- API keys must be set via environment variables, never committed to configuration files.
- The configuration file may be committed to version control only if it contains no secrets.
- When using the Ollama provider locally, no API key is required.
