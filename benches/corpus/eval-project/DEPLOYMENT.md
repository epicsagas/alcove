# eval-project Deployment Runbook

## Prerequisites

Before deploying eval-project, verify the following system requirements:

- **Operating System**: Linux (glibc 2.31+) or macOS 12+. Windows is not supported.
- **Runtime**: No external runtime required. The server is a statically linked Rust binary.
- **Memory**: Minimum 512 MB RAM. Recommended 1 GB for corpora exceeding 10,000 documents.
- **Disk space**: Allow 3x the corpus size for index storage. A 100 MB corpus requires approximately 300 MB for the full index including vectors.
- **Network**: Outbound HTTPS access required for OpenAI embedding API. Not required when using Ollama locally.

Download the latest release binary for your platform from the releases page. Verify the checksum matches the published SHA-256 value.

## Docker Deployment

eval-project publishes a container image for simplified deployment. The image is based on Debian slim and includes the server binary.

### Building the Image

```bash
docker build -t eval-project:latest .
```

The Dockerfile copies the pre-built binary and sets the default command to run the server with HTTP transport.

### Running the Container

```bash
docker run -d \
  --name eval-project \
  -p 8080:8080 \
  -v /path/to/vaults:/vaults:ro \
  -v /path/to/index:/index \
  -e EVAL_PROJECT_OPENAI_API_KEY=<REDACTED> \
  -e EVAL_PROJECT_BIND=0.0.0.0:8080 \
  eval-project:latest
```

Mount vault directories as read-only volumes. The index directory must be writable for segment persistence.

### Docker Compose

For multi-container deployments, use the provided `docker-compose.yml`:

```yaml
services:
  eval-project:
    image: eval-project:latest
    ports:
      - "8080:8080"
    volumes:
      - ./vaults:/vaults:ro
      - ./index:/index
    environment:
      - EVAL_PROJECT_OPENAI_API_KEY=${OPENAI_KEY}
      - EVAL_PROJECT_BIND=0.0.0.0:8080
    restart: unless-stopped
```

### Resource Limits

Apply memory and CPU limits to prevent resource exhaustion during large index builds:

```bash
docker update --memory=1g --cpus=2.0 eval-project
```

## Health Checks

The server exposes health and readiness endpoints when running with HTTP transport.

### Liveness Check

```
GET /health
```

Returns HTTP 200 when the server process is running. Does not check index status.

Example response:
```json
{ "status": "alive", "uptime_secs": 3600 }
```

### Readiness Check

```
GET /ready
```

Returns HTTP 200 when the server is ready to accept search queries. This endpoint verifies that at least one project index is loaded and the embedding backend is reachable.

Example response:
```json
{
  "status": "ready",
  "projects_indexed": 3,
  "embedding_backend": "openai",
  "last_index_time": "2025-05-28T14:30:00Z"
}
```

### Metrics Endpoint

```
GET /metrics
```

Exposes Prometheus-format metrics including query count, latency histograms, and index size gauges.

### Configuring Health Check Intervals

For container orchestrators, configure health checks with a 10-second interval and 3-second timeout. Allow 30 seconds of startup grace period before the first readiness check.

## Rollback Procedure

When a deployment introduces regressions, follow this procedure to roll back to the previous version.

### Step 1: Stop the Current Deployment

```bash
docker stop eval-project
```

Do not remove the container or index volume. The index format is backward-compatible within the same major version.

### Step 2: Pull the Previous Version

```bash
docker pull eval-project:v0.10
```

Check the release notes to confirm index compatibility between versions.

### Step 3: Start the Previous Version

```bash
docker run -d \
  --name eval-project-rollback \
  -p 8080:8080 \
  -v /path/to/vaults:/vaults:ro \
  -v /path/to/index:/index \
  -e EVAL_PROJECT_OPENAI_API_KEY=<REDACTED> \
  eval-project:v0.10
```

### Step 4: Verify Rollback

1. Check the readiness endpoint: `curl http://localhost:8080/ready`
2. Run a test search query and verify results.
3. Check server logs for index loading errors: `docker logs eval-project-rollback`

### Step 5: Investigate and Fix

Once the previous version is running, investigate the regression in a separate environment. Create a test index from the production corpus and reproduce the issue before shipping a fix.

### Index Incompatibility

If the index format changed between versions (major version upgrade), a full re-index is required after rollback. Run:

```bash
docker exec eval-project-rollback eval-project reindex --all
```

This rebuilds the index from the vault files. Re-indexing a 10,000 document corpus takes approximately 30 seconds.
