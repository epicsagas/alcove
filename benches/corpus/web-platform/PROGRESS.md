# MetricFlow Development Progress

## Current Version: v0.8.2 (Beta)

## Milestones

### v0.1.0 -- Project Bootstrap (Completed 2025-09-15)

- Repository setup with Go modules, Docker Compose, and CI pipeline.
- PostgreSQL schema design and initial migrations.
- Basic auth service with email/password registration.

### v0.2.0 -- Event Ingestion (Completed 2025-10-20)

- Kafka-based event pipeline with schema validation.
- JavaScript SDK with automatic pageview tracking.
- Batch ingestion endpoint with rate limiting.
- Event storage in TimescaleDB hypertables.

### v0.3.0 -- Query Engine (Completed 2025-11-18)

- SQL query builder for metric aggregation and Redis caching layer.
- Time range filtering with comparison support. Benchmarked at p95 under 800ms.

### v0.4.0 -- Dashboard Builder (Completed 2025-12-22)

- Drag-and-drop canvas with line, bar, pie charts and single-value cards.
- Dashboard sharing via public links. CSV and PNG export functionality.

### v0.5.0 -- User Segmentation (Completed 2026-01-30)

- Dynamic segment builder with AND/OR logic for event property filters.
- Segment comparison view and scheduled recalculation (daily and weekly).

### v0.6.0 -- Automated Insights (Completed 2026-03-10)

- Anomaly detection using statistical thresholds with email and Slack notifications.
- Weekly summary report generation for metric changes.

### v0.7.0 -- Multi-Tenancy (Completed 2026-04-25)

- Schema-per-tenant PostgreSQL isolation, RBAC (admin, analyst, viewer).
- Usage-based billing metering and tenant provisioning API.

### v0.8.0 -- Beta Release (Completed 2026-05-28)

- Public beta signup, iOS/Android SDK release, PDF export, SOC 2 audit initiated.

## In Progress

- iOS SDK custom event tracking refinement (target: v0.8.3).
- Query engine optimization for datasets exceeding 500M events.
- Funnel drop-off analysis visualization.

## Upcoming

- **v0.9.0**: Mobile dashboard app, funnel builder, Slack dashboard embeds.
- **v1.0.0**: General availability, SLA enforcement, advanced A/B testing module.

## Known Issues

- Dashboard canvas rendering lag on Safari when widgets exceed 20.
- Segment recalculation timeout for tenants with over 1 billion events.
- PDF export occasionally truncates wide tables.
