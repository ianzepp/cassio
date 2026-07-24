# Factory Ledger — Process Case-Study Pipeline

## Vision Source

- `docs/factory/GOAL.md`
- Session experiment (2026-07-24): multi-model daily for `2026-06-11`;
  DeepSeek V4 Pro + GPT-5.6 Luna medium; codex proxy chat-completions shim
- Cassio compact pipeline today: sessions → dailies → monthlies (no weeklies)

## Factory Status

| Field | Value |
| --- | --- |
| **Current phase** | **B — CaseStudyEvidence schema** |
| **Status** | Ready to execute (A closed as docs-only baseline) |
| **Primary repo** | `~/work/ianzepp/cassio` |
| **Data archive** | `~/personal/transcripts` |
| **Experiment root** | `~/tmp/cassio-daily-experiment-2026-06-11/` |

## Roadmap Units (Epics)

| Epic | Phases | Intent |
| --- | --- | --- |
| **E0 Baseline** | A | Lock experiment findings as durable ground truth |
| **E1 Evidence contract** | B, C | Schema + daily/merge emit/preserve |
| **E2 Numeric rails** | D | Deterministic cost/agent/volume metrics |
| **E3 Multi-scale pipeline** | E, F | Weeklies + monthly-via-weeklies |
| **E4 Validation** | G | Loss audit for case-study fidelity |
| **E5 Production quality** | H | Model routing / experiment harness |

## Delivery-Sized Units (Phases)

| Phase | Name | Spec | Status |
| --- | --- | --- | --- |
| **A** | Experimental baseline & findings lock | `phase-A-baseline-delivery.md` | **done** (docs) |
| **B** | CaseStudyEvidence schema | `phase-B-case-study-evidence-schema-delivery.md` | **next** |
| **C** | Daily compact + merge contract | `phase-C-daily-contract-delivery.md` | pending |
| **D** | Session metrics rails | `phase-D-session-metrics-rails-delivery.md` | pending |
| **E** | Weekly compaction | `phase-E-weekly-compaction-delivery.md` | pending |
| **F** | Monthly via weeklies | `phase-F-monthly-via-weeklies-delivery.md` | pending |
| **G** | Loss audit protocol | `phase-G-loss-audit-delivery.md` | pending |
| **H** | Model routing harness | `phase-H-model-routing-harness-delivery.md` | pending |

## Dependency Notes

```text
A ──► B ──► C ──► E ──► F ──► G
            │
            └──► D ──► E (D can parallelize after B; E needs C+D)
H can start after C (uses daily path); full multi-model bakeoff after E optional
```

- **B before C** — schema before prompt/code emit it.
- **C before E** — weeklies roll up daily evidence blocks.
- **D before E preferred** — weeklies should attach metric rails, not guess.
- **E before F** — monthly path needs weekly artifacts.
- **F before G** — loss audit needs both monthly paths (or scripted stubs).
- **H independent-ish** after C — quality routing for dailies.

## Generated Delivery Specs

All under `docs/factory/phase-*-delivery.md` (this commit).

## Completed Units

### A — Experimental baseline & findings lock (2026-07-24)

- Confirmed cassio daily multi-chunk path for `2026-06-11` (15 sessions, 6 chunks).
- DeepSeek V4 Pro direct daily completed (~11m, ~52KB).
- GPT-5.6 Luna medium via local proxy `/v1/chat/completions` completed (~11m, ~35KB).
- Documented: Codex OAuth has no chat-completions upstream; proxy translates.
- Documented: dailies strong on decisions/corrections; weak on systematic metrics.
- Artifacts: `~/tmp/cassio-daily-experiment-2026-06-11/`.

## Pending Units

B → H as table above.

## Blocked Units

None.

## Repo Boundaries

| Path | Role |
| --- | --- |
| `cassio` | Product code, prompts, CLI, tests, factory docs |
| `~/personal/transcripts` | Production archive (read; careful write) |
| `~/tmp/cassio-*` | Experiment outputs |
| `~/.local/bin/openai-codex-proxy` | Host proxy (OAuth ChatGPT models) — change only with explicit phase note |

## Deferred Findings / Future Work

| ID | Finding | Suggested home |
| --- | --- | --- |
| DF-1 | Quarterly synthesizer for customer-facing case chapters | Post-H epic |
| DF-2 | Reprocess historical months with new schema (migration) | Post-G |
| DF-3 | Auto-label feature/goal IDs across sessions | Research / later phase |
| DF-4 | Embed CaseStudyEvidence in training.json exports | After B stabilizes |
| DF-5 | Persist proxy chat-completions shim in a versioned repo | Host tooling hygiene |

## User Decisions Needed

| Decision | Default if deferred | Blocks |
| --- | --- | --- |
| Primary production daily model | Luna-class for lean weeklies; DeepSeek for audit samples | H |
| Weekly filename convention | `YYYY-Www.weekly.md` ISO week under month dir | E |
| Whether production archive gets weeklies committed | Yes once G passes | F/G |
| Schema format (YAML fence vs JSON vs markdown tables) | Markdown tables + optional YAML block in B | B |

## Checkpoint Log

| Date | Event |
| --- | --- |
| 2026-07-24 | Factory package created; Phase A closed; Phase B is current |
