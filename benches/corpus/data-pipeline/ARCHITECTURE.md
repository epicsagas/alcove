# Data Pipeline Architecture

The data pipeline is a distributed stream processing system that ingests event data from multiple sources, transforms and enriches it in real time, and delivers aggregated outputs to a data lake and downstream consumers.

## System Overview

The architecture has five logical stages:

1. **Ingestion** -- Apache Kafka consumers read raw events from upstream producers (application logs, IoT sensors, clickstream trackers).
2. **Validation** -- A schema registry (Confluent Schema Registry) enforces Avro schemas on every incoming message. Invalid messages route to a dead-letter topic.
3. **Transformation** -- Apache Spark Structured Streaming jobs clean, normalize, and enrich events. Lookups against reference data (user profiles, geo-IP mappings) add context.
4. **Aggregation** -- Pre-computed tumbling window aggregations (1-minute, 1-hour, 1-day) write results to both Kafka output topics and Apache Parquet files on S3.
5. **Serving** -- Downstream consumers (BI tools, ML feature store, alerting service) read from Kafka topics or query Parquet files through Apache Presto.

## Data Ingestion

Kafka clusters run with 3-node broker ensembles in each availability zone. Topics are partitioned by `source_id` to guarantee ordering within a data source. Key ingestion parameters:

- **Retention**: 7 days for raw topics, 30 days for processed topics.
- **Replication factor**: 3 for production topics.
- **Compression**: LZ4 for raw events, Zstandard for aggregated outputs.
- **Throughput target**: 200,000 events per second sustained, 500,000 peak.

A Kafka Connect cluster manages source connectors for PostgreSQL (Debezium CDC), S3 (batch file drops), and HTTP webhook endpoints.

## Stream Processing

Spark Structured Streaming jobs run on a Kubernetes-based Spark operator. Each job is stateful, with checkpointing to S3 every 30 seconds.

### Transformation Pipeline

The core transformation DAG performs the following steps:

1. Deserialize Avro payload using the schema registry.
2. Normalize timestamps to UTC and validate required fields.
3. Enrich with geo-IP data using a MaxMind lookup broadcast join.
4. Join with user profile reference data from a Delta Lake table.
5. Compute derived fields (session duration, event sequence number).
6. Serialize to Avro and produce to the enriched events topic.

### Aggregation Windows

Tumbling window aggregations compute the following metrics per `source_id`:

- Event count, unique user count, error rate, p50/p95/p99 latency.
- Top-N event types by frequency.
- Geographical distribution breakdown.

Window results are written both to Kafka (for real-time consumers) and to partitioned Parquet files in the data lake (for batch analytics).

## Data Lake

The data lake uses a Hive-compatible directory structure on S3:

- `/raw/{source}/{date}/` -- Original events in Avro format.
- `/enriched/{source}/{date}/` -- Enriched events in Parquet format.
- `/aggregates/{source}/{window}/{date}/` -- Pre-aggregated metrics in Parquet format.

Parquet files use Snappy compression and are partitioned by date and source. Compaction jobs run hourly to merge small files into 256 MB target sizes.

## Schema Registry

Confluent Schema Registry stores Avro schemas with backward compatibility mode enforced. Schema evolution rules:

- New fields must have default values.
- Field removals require a deprecation period of 30 days.
- Schema versions are immutable once registered.
- A compatibility check runs as a CI gate before any pipeline deployment.

## Monitoring and Observability

- Prometheus scrapes Kafka broker metrics, Spark job metrics, and custom application metrics.
- Grafana dashboards display consumer lag, processing latency, error rates, and throughput.
- PagerDuty alerts fire when consumer lag exceeds 10,000 messages or processing latency exceeds the 5-second SLA.
