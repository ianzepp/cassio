# Factory Playbook — Process Case-Study Pipeline

## Entry Point

For a new session implementing this goal:

1. Read `docs/factory/GOAL.md` (vision boundary, non-goals, invariants).
2. Read `docs/factory/factory-ledger.md` (current phase, completed work).
3. Open the **current phase** delivery spec under `docs/factory/`.
4. Implement only that phase. Do not skip ahead.

## Vocabulary

| Term | Means |
| --- | --- |
| **CaseStudyEvidence** | Fixed structured block every daily/weekly must carry |
| **Metric rails** | Deterministic numbers from session metadata (no LLM) |
| **Weekly** | `YYYY-Www.weekly.md` (or agreed name) rollup of dailies |
| **Loss audit** | Scored compare of monthly-from-dailies vs monthly-from-weeklies |
| **Experiment dir** | Isolated output (e.g. `~/tmp/cassio-…`); not production archive |

## Factory Loop (this project)

Same as global factory: vision → production → delivery → loop.

Project-specific rules:

| Rule | Detail |
| --- | --- |
| **Evidence over prose** | Prefer schema fields + metric rails over longer narrative |
| **No archive pollution** | Default compact outputs for experiments to temp dirs |
| **One phase** | Do not implement weekly before daily schema lands |
| **Validate with fixed day** | Prefer `2026-06-11` and/or one known dense week for gates |
| **Quotes verbatim** | Carry-forward quotes must remain user-verbatim |

## Slice / Phase / Epic

| Term | This project |
| --- | --- |
| **Slice** | The whole process case-study pipeline program |
| **Phase** | One delivery-sized unit (A–H in the ledger) |
| **Epic** | Optional grouping (schema, pipeline, audit, production) |

## Artifact Layout

```text
docs/factory/
  GOAL.md
  factory-playbook.md
  factory-ledger.md
  phase-A-baseline-delivery.md
  phase-B-case-study-evidence-schema-delivery.md
  phase-C-daily-contract-delivery.md
  phase-D-session-metrics-rails-delivery.md
  phase-E-weekly-compaction-delivery.md
  phase-F-monthly-via-weeklies-delivery.md
  phase-G-loss-audit-delivery.md
  phase-H-model-routing-harness-delivery.md
```

## Checkpoint Style

Each phase delivery names:

- Acceptance criteria
- Commands / artifacts to inspect
- PASS / NEEDS REVIEW / FAIL gate

## Deferred Findings

End of phase: record out-of-scope discoveries in the ledger under
`Deferred Findings / Future Work`. Do not expand phase scope mid-flight.
