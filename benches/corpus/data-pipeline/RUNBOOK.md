# StreamForge Operational Runbook

This runbook covers operational procedures for the StreamForge data pipeline platform. Use it for incident response, routine maintenance, and capacity planning.

## Monitoring

### Key Metrics

Monitor the following metrics on the Grafana operations dashboard:

- **Consumer Lag**: Kafka consumer group lag per pipeline. Alert threshold: 10,000 messages.
- **Processing Latency**: Time from Kafka produce to Kafka commit per event. p99 target: 5 seconds.
- **Error Rate**: Percentage of events routed to dead-letter topics. Target: below 0.5%.
- **Throughput**: Events per second ingested and emitted per pipeline instance.
- **Checkpoint Duration**: Spark Structured Streaming checkpoint write time. Alert if exceeding 60 seconds.

### Dashboards

- `pipeline-overview`: Aggregate health across all pipelines with traffic light indicators.
- `pipeline-detail`: Per-pipeline drill-down with source, transform, and sink stage metrics.
- `infrastructure`: Kafka broker disk usage, Spark executor memory, S3 write latency.
- `cost`: Daily cost breakdown by pipeline, compute hours, and storage consumption.

### Alert Channels

- Critical alerts route to PagerDuty on-call rotation (24/7).
- Warning alerts post to the `#data-pipeline-alerts` Slack channel.
- Informational alerts (deployments, scaling events) go to the `#data-pipeline-ops` channel.

## Incident Response

### Severity Levels

| Level | Criteria | Response Time |
|-------|----------|---------------|
| P1 | Data loss or complete pipeline outage | 15 minutes |
| P2 | Single pipeline failure or SLA breach | 30 minutes |
| P3 | Degraded performance or elevated error rate | 2 hours |

### P1: Pipeline Outage

1. Verify Kafka broker health: `kafka-broker-api-versions --bootstrap-list <broker-list>`.
2. Check Spark job status on Kubernetes dashboard for executor OOM kills.
3. If Kafka is healthy but Spark failed, restart the operator: `kubectl rollout restart deployment spark-operator -n streamforge`.
4. After recovery, replay dead-letter messages: `sf-cli replay --topic <dead-letter-topic> --from <outage-start-timestamp>`.
5. Validate end-to-end integrity by comparing source and sink event counts.

### P2: Consumer Lag Spike

1. Identify the lagging consumer group: `kafka-consumer-groups --describe --group <group-id>`.
2. Check downstream sink health (database connections, S3 write permissions).
3. Scale Spark executors: `kubectl scale statefulset <pipeline> --replicas=<current+2>`.
4. Monitor lag reduction. Escalate to P1 if no improvement within 10 minutes.

### Schema Validation Failures

1. Check schema registry for recent changes: `curl http://schema-registry:8081/subjects/<topic>-value/versions/latest`.
2. Compare failing event payload against the schema and roll back if needed.
3. Replay dead-letter messages after fixing the incompatibility.

## Scaling

### Horizontal Scaling

- Add Kafka partitions to increase parallelism: `kafka-topics --alter --topic <topic> --partitions <new-count>`.
- Scale Spark executors proportionally. Target: 1 executor per 2 partitions.

### Vertical Scaling

- Increase Spark executor memory if checkpoint durations exceed 60 seconds.
- Upgrade Kafka broker instance types when disk IOPS or network throughput saturates.

### Capacity Planning

- Review throughput trends weekly. Provision for 2x current peak.
- S3 storage grows ~1 TB per billion events. Transition cold data to Glacier after 90 days.
- Kafka disk usage should stay below 70%. Set retention policies to auto-delete after the configured window.

### Maintenance Windows

- Schedule Spark upgrades during lowest-traffic window (02:00-04:00 UTC).
- Kafka broker rolling restarts: `kafka-preferred-replica-election`. Announce 48 hours in advance.
