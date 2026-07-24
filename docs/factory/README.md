# Factory: Process Case-Study Pipeline

Meta-engineering program: multi-scale transcript evidence for longitudinal
process case studies (CTO / fractional CTO materials).

## Start Here

1. [`GOAL.md`](GOAL.md) — vision, boundary, case-study questions  
2. [`factory-playbook.md`](factory-playbook.md) — how to enter a session  
3. [`factory-ledger.md`](factory-ledger.md) — **current phase** and status  

## Phases

| Phase | Delivery spec | Status |
| --- | --- | --- |
| A | [phase-A-baseline-delivery.md](phase-A-baseline-delivery.md) | done |
| B | [phase-B-case-study-evidence-schema-delivery.md](phase-B-case-study-evidence-schema-delivery.md) | done |
| C | [phase-C-daily-contract-delivery.md](phase-C-daily-contract-delivery.md) | done |
| D | [phase-D-session-metrics-rails-delivery.md](phase-D-session-metrics-rails-delivery.md) | done |
| E | [phase-E-weekly-compaction-delivery.md](phase-E-weekly-compaction-delivery.md) | done |
| F | [phase-F-monthly-via-weeklies-delivery.md](phase-F-monthly-via-weeklies-delivery.md) | done |
| G | [phase-G-loss-audit-delivery.md](phase-G-loss-audit-delivery.md) | done |
| H | [phase-H-model-routing-harness-delivery.md](phase-H-model-routing-harness-delivery.md) | done |

See also: [case-study-evidence.md](../case-study-evidence.md), [loss-audit-protocol.md](loss-audit-protocol.md).

## Dependency Graph

```text
A → B → C → E → F → G
         ↘ D ↗
C → H (routing harness; full bakeoff after evidence exists)
```
