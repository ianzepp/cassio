# Phase B Delivery — CaseStudyEvidence Schema

## Phase Name

Define the fixed evidence contract that every daily (and later weekly) must emit.

## Input

- `docs/factory/GOAL.md` case-study questions
- Sample dailies from Phase A experiment
- Existing `src/prompts/compact.md` / `daily_merge.md` (read-only this phase)

## Problem

Narrative dailies do not systematically carry counts, outcomes, instruction
deltas, or agent/cost hooks. Without a schema, weeklies and monthlies cannot
support defensible case studies.

## Scope

- Specify `CaseStudyEvidence` field set (required vs optional).
- Map each case-study question in GOAL.md to one or more fields.
- Choose serialization for inside `.daily.md` (default: fenced YAML block under
  a fixed heading, plus human-readable tables if useful).
- Document merge rules: which fields are union, sum, max, or “keep all quotes”.
- Add schema doc under `docs/factory/` or `docs/` (e.g.
  `docs/case-study-evidence.md`).
- Add at least one **filled example** derived from 2026-06-11 (manual is fine).

### Minimal required fields (starting point — refine in-phase)

| Field | Purpose |
| --- | --- |
| `period` | date or week id |
| `projects` | project list |
| `volume.sessions` / `volume.user_turns` / `volume.corrections` / `volume.decisions` | scale |
| `agents_used[]` | model/tool/session counts (may reference rails) |
| `instruction_deltas[]` | rules codified that period |
| `corrections[]` | type + verbatim USER quote |
| `outcomes[]` | unit name + result enum |
| `process_invocations[]` | factory/delivery/skill usage |
| `open_threads[]` | unfinished work |
| `case_study_quotes[]` | ≤10 verbatim USER quotes |
| `metrics_ref` | pointer/hash to rails file if present |

### Outcome enum (starting point)

`autonomous_success | first_pass | rework | hardening | deferred | abandoned | unknown`

### Correction type enum (starting point)

`over_abstraction | ignored_clean_break | wrong_boundary | tool_misuse | ignored_instruction | over_hedging | other`

## Out Of Scope

- Implementing emit in compact code (Phase C)
- Weekly/monthly prompts (E/F)
- Automatic extraction of all fields from sessions without LLM

## Acceptance Criteria

- Schema doc is complete and maps every GOAL case-study question.
- Example block for 2026-06-11 is filled enough to read as a real day.
- Merge semantics are explicit (no “use judgment” for required fields).
- Ledger marks B done and C unblocked.

## Checkpoint

```sh
test -f docs/case-study-evidence.md   # or agreed path
rg -n "CaseStudyEvidence|instruction_deltas|case_study_quotes" docs/
```

Human review: schema answers “can monthly claim X without inventing?” for each
GOAL question.

## Gate

**PASS** if schema + example + merge rules land and question map is complete.  
**FAIL** if fields are only narrative prose with no stable names.
