# Phase E Delivery — Weekly Compaction

## Phase Name

Add a weekly compaction stage that rollups dailies under a size budget while
preserving CaseStudyEvidence.

## Input

- Phase B schema + Phase C daily blocks
- Phase D metrics rails (attach/merge; if D not merged yet, accept day metrics
  stubs)
- Existing compact architecture (`run_dailies`, `run_monthly`)

## Problem

Thirty large dailies blow monthly context. Weeklies are the intermediate product
and the primary monthly input.

## Scope

- Add `src/prompts/weekly.md` (and `weekly_merge.md` if chunking week input).
- Weekly responsibilities:
  - Roll up CaseStudyEvidence (sums, unions, quote caps).
  - Short week arc (not a rewrite of each day).
  - Attach or embed week-level metrics rails.
  - Preserve open threads across the week.
- CLI: `cassio compact weeklies` (or `cassio compact weekly --week YYYY-Www`).
- Output: e.g. `YYYY-MM/YYYY-Www.weekly.md` (ISO week; document edge cases for
  weeks spanning months — pick one rule and stick to it).
- Size target: design for **≤12–15KB typical**; document hard fails/warns.
- Tests for week grouping, pending detection, and evidence merge fixtures.
- Dry-run path + experiment generation for one real week containing 2026-06-11
  if neighboring dailies exist; else synthetic fixture week.

## Out Of Scope

- Changing monthly prompts to consume weeklies (Phase F)
- Loss audit (G)
- Rebuilding entire archive

## Acceptance Criteria

- `cassio compact weeklies --help` documents flags (provider/model/base-url same
  pattern as dailies).
- Weekly output contains parseable CaseStudyEvidence.
- Volume fields equal sum of input dailies within tolerance (exact for
  sessions/corrections when present).
- Unit tests do not require live LLM for grouping/merge helpers.

## Checkpoint

```sh
cargo test
cassio compact weeklies --help
# live optional against experiment dailies
```

## Gate

**PASS** if CLI + prompts + tests land and one weekly (fixture or live) shows
intact evidence rollup.  
**FAIL** if weekly is only a shorter narrative without evidence block.
