# eval-project Roadmap and Valuation

## Project Vision

eval-project aims to become the standard documentation search layer for AI-assisted development environments. By providing fast, accurate context retrieval through the MCP protocol, the server enables any AI agent to understand project documentation without manual context management.

## Roadmap

### Phase 1: Foundation (Complete)

- Core search pipeline with BM25 and vector retrieval.
- MCP server protocol with stdio and HTTP transports.
- Heading-based chunking with metadata extraction.
- Interactive setup and CLI tooling.

### Phase 2: Quality (Current)

- Benchmark framework with precision@k, NDCG, and MRR metrics.
- CJK tokenizer support for Korean and Chinese content.
- Hybrid search with Reciprocal Rank Fusion.
- Quality gates in CI pipeline.

### Phase 3: Scale (Planned)

- Incremental embedding updates to avoid full re-index on model changes.
- Federated search across multiple eval-project instances.
- Plugin system for custom tokenizers and scoring functions.
- PDF and plain text file support.

### Phase 4: Platform (Future)

- Multi-user authentication and access control.
- Web dashboard for search analytics and index management.
- GraphQL API alongside the existing MCP endpoints.
- Managed cloud offering with automatic scaling.

## Milestones

| Milestone | Target Date | Key Deliverables |
|-----------|-------------|------------------|
| v1.0 Stable | 2025-Q3 | Stable API, full documentation, production readiness review |
| v1.1 CJK Excellence | 2025-Q4 | Optimized Korean/Chinese tokenizers, CJK benchmark suite |
| v2.0 Platform | 2026-Q1 | Multi-user, web dashboard, plugin system |
| v2.1 Cloud | 2026-Q2 | Managed hosting, federated search, auto-scaling |

## Monetization Strategy

### Pricing Tiers

**Community (Free)**
- Single user, local deployment.
- Up to 3 project vaults.
- Standard embedding models.
- Community support via GitHub.

**Team ($15/user/month)**
- Up to 10 users per instance.
- Unlimited project vaults.
- Priority embedding model access.
- Shared index with access control.
- Email support with 48-hour response time.

**Enterprise (Custom Pricing)**
- Unlimited users and vaults.
- Federated search across instances.
- Custom embedding model integration.
- SSO and audit logging.
- Dedicated support with 4-hour response SLA.
- On-premise deployment assistance.

### Revenue Projections

The project targets a developer tools market with an estimated 50,000 potential team users. Assuming a 2% conversion rate on the Team tier after the first year:

- Year 1: $18,000 MRR at launch (100 team subscribers).
- Year 2: $90,000 MRR (500 team subscribers, 5 enterprise contracts).
- Year 3: $250,000 MRR (1,500 team subscribers, 15 enterprise contracts).

### Launch Plan

1. **Soft Launch (v1.0)**: Release as open source under Apache-2.0. Build community through documentation quality and developer experience.
2. **Growth Phase (v1.1-1.5)**: Introduce Team tier with hosted option. Focus on CJK market with localized documentation.
3. **Platform Phase (v2.0)**: Launch Enterprise tier with full platform features. Establish partnership integrations with IDE vendors.

## Success Metrics

- Search query latency p99 under 50 ms.
- Precision@5 above 0.75 on benchmark suites.
- Community GitHub stars target: 1,000 within 6 months of v1.0.
- Team tier conversion target: 2% of active users within 12 months.
