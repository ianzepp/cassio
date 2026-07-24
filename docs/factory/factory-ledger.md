# Factory Ledger — Process Case-Study Pipeline

## Vision Source

- `docs/factory/GOAL.md`
- Session experiment (2026-07-24): multi-model daily for `2026-06-11`
- Schema: `docs/case-study-evidence.md`

## Factory Status

| Field | Value |
| --- | --- |
| **Current phase** | Program slice A–H **implemented** (live LLM weeklies/monthlies optional follow-up) |
| **Status** | Code + docs landed; production archive not backfilled |
| **Primary repo** | `~/work/ianzepp/cassio` |
| **Data archive** | `~/personal/transcripts` |
| **Experiment root** | `~/tmp/cassio-daily-experiment-2026-06-11/` |

## Delivery-Sized Units (Phases)

| Phase | Name | Status |
| --- | --- | --- |
| **A** | Experimental baseline | **done** |
| **B** | CaseStudyEvidence schema | **done** |
| **C** | Daily compact + merge contract | **done** (prompts + merge helpers/tests) |
| **D** | Session metrics rails | **done** (`cassio metrics day|week`) |
| **E** | Weekly compaction | **done** (`cassio compact weeklies`) |
| **F** | Monthly via weeklies | **done** (`--source auto|weeklies|dailies`) |
| **G** | Loss audit protocol | **done** (`cassio audit loss` + protocol doc) |
| **H** | Model routing harness | **done** (`scripts/compact-day-models.sh`) |

## Dependency Notes

```text
A → B → C → E → F → G
         ↘ D ↗
C → H
```

## Completed Units

### A — Baseline
- 2026-06-11 multi-model dailies; proxy chat-completions path.

### B — Schema
- `docs/case-study-evidence.md`
- Example: `docs/factory/examples/2026-06-11.case-study-evidence.yaml`

### C — Daily contract
- `src/prompts/compact.md` + `daily_merge.md` require CaseStudyEvidence
- `src/evidence.rs` parse/merge/validate + tests

### D — Metrics rails
- `src/metrics.rs` + `cassio metrics day|week`
- Smoke: 2026-06-11 → 15 sessions, codex/gpt-5.5

### E — Weeklies
- `src/prompts/weekly.md` + `weekly_merge.md`
- `cassio compact weeklies` with mechanical evidence merge fallback

### F — Monthly source
- `MonthlySource::{Auto,Dailies,Weeklies}`
- `cassio compact monthly --source …`
- Monthly prompt forbids invented metrics; adds Process Case-Study Notes

### G — Loss audit
- `cassio audit loss --expected --actual`
- `docs/factory/loss-audit-protocol.md`

### H — Harness
- `scripts/compact-day-models.sh` + `scripts/profiles.example.env`

## Deferred Findings / Future Work

| ID | Finding | Suggested home |
| --- | --- | --- |
| DF-1 | Live re-run dailies with new prompts on 2026-06-11 (LLM cost) | Ops |
| DF-2 | Historical archive backfill of weeklies | Post go/no-go |
| DF-3 | Soft-fail validate after each daily write | polish |
| DF-4 | Quarterly synthesizer for customer case chapters | New epic |
| DF-5 | Token parsing edge cases on unusual session formats | metrics |
| DF-6 | Install release binary after merge (`cargo install`) | release |

## User Decisions Needed

| Decision | Default | Blocks |
| --- | --- | --- |
| Primary production daily model | Luna-class lean; DeepSeek audit | production default config |
| Commit weeklies into transcripts git | After one real week loss audit PASS | ops |
| Push cassio commits to origin | user | remote |

## Checkpoint Log

| Date | Event |
| --- | --- |
| 2026-07-24 | Factory package created |
| 2026-07-24 | Phases B–H implemented in tree; unit tests green |
