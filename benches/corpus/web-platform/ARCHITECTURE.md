# Web Platform Architecture

The web platform is a SaaS analytics service built on a REST API architecture. It provides multi-tenant data aggregation, real-time metric visualization, and user segmentation capabilities for product teams.

## System Overview

The platform follows a four-layer architecture:

1. **API Gateway Layer** -- An NGINX reverse proxy handles TLS termination, rate limiting, and request routing to backend services.
2. **Application Layer** -- A set of stateless microservices written in Go, each responsible for a bounded context (ingestion, query, auth, billing).
3. **Cache Layer** -- Redis clusters provide sub-millisecond response times for frequently accessed dashboard configurations and aggregated metrics.
4. **Persistence Layer** -- PostgreSQL serves as the primary data store with TimescaleDB extensions for time-series optimization.

All services communicate over HTTP/2 with Protocol Buffers for internal RPC. External clients interact exclusively through the REST API gateway.

## API Gateway

The NGINX gateway enforces the following policies:

- **Rate limiting**: 1000 requests per minute per API key, with burst capacity of 50.
- **TLS termination**: All external traffic uses TLS 1.3 with HSTS headers.
- **CORS**: Configured per-tenant with whitelisted origins.
- **Request validation**: JSON schema validation before forwarding to application services.

The gateway routes requests based on URL prefix. The `/api/v2/events` endpoint forwards to the ingestion service, while `/api/v2/query` routes to the query service.

## Authentication Middleware

Authentication uses JWT tokens issued by the auth service. The flow is:

1. Client sends credentials to `POST /api/v2/auth/token`.
2. Auth service validates against the user database and issues a signed JWT (RS256 algorithm).
3. JWT contains claims: `sub` (user ID), `tenant_id`, `role`, and `exp` (15-minute expiry).
4. A refresh token with 7-day expiry is stored in an HttpOnly cookie.
5. Each API request includes the JWT in the `Authorization: Bearer` header.
6. The gateway validates the signature against the public key fetched from the auth service's JWKS endpoint.

Role-based access control maps `tenant_id` and `role` claims to resource permissions at the query level.

## Database Layer

### PostgreSQL with TimescaleDB

The primary database uses a logical schema per tenant. TimescaleDB hypertables partition event data by time (1-day chunks) and automatically compress chunks older than 30 days.

Key tables:

- `events` -- Raw event stream with JSONB payload columns.
- `aggregates` -- Pre-computed hourly and daily rollups for dashboard queries.
- `segments` -- User-defined cohort definitions stored as JSON predicates.
- `dashboards` -- Dashboard layout and widget configurations.

Read replicas handle query traffic with streaming replication. Write traffic goes exclusively to the primary instance.

### Redis Cache

Redis stores three categories of cached data:

- **Session cache**: JWT validation results and user session metadata (TTL: 5 minutes).
- **Metric cache**: Pre-aggregated query results keyed by tenant, metric, and time range (TTL: 60 seconds).
- **Rate limit counters**: Sliding window counters for API rate limiting.

Cache invalidation uses a publish-subscribe pattern. When the ingestion service writes new aggregates, it publishes an invalidation event that the query service subscribes to.

## Data Flow

1. Client SDK sends event batch to the ingestion endpoint.
2. Gateway validates JWT and forwards to the ingestion service.
3. Ingestion service validates the event schema and writes to a Kafka topic.
4. A stream processor consumes from Kafka, enriches events, and writes to PostgreSQL.
5. The query service reads from PostgreSQL and Redis to serve dashboard API requests.

## Deployment

All services run in Kubernetes with horizontal pod autoscaling. The ingestion service scales on CPU utilization (target: 60%), while the query service scales on request latency (p99 target: 200ms). Database backups run every 6 hours with point-in-time recovery enabled.
