# Phase C Delivery — Daily Compact + Merge Contract

## Phase Name

Make daily compaction emit CaseStudyEvidence and preserve it through chunk merge.

## Input

- Phase B schema doc
- `src/prompts/compact.md`
- `src/prompts/daily_merge.md`
- `src/compact.rs` (merge/chunk behavior)
- Phase A sample day for re-run validation

## Problem

Even a perfect schema is useless if the LLM daily omits it or the merge pass
drops it while synthesizing chunk partials.

## Scope

- Update `compact.md` to require CaseStudyEvidence block (exact heading/fence).
- Update `daily_merge.md` with field-level merge rules from Phase B.
- Add tests that:
  - merge input containing evidence blocks produces a single valid block;
  - required keys survive merge fixtures.
- Optionally validate structure post-LLM with a lightweight checker (warn or
  soft-fail — decide in implementation; prefer non-fatal warn for v1).
- Re-run one daily on `2026-06-11` into experiment dir; confirm block present.

## Out Of Scope

- Weeklies (E)
- Changing max_input_bytes policy globally
- Full historical recompact of archive

## Acceptance Criteria

- Prompts require CaseStudyEvidence.
- Merge rules tested with fixtures (no live LLM required for unit tests).
- One live daily re-run (any approved model) contains a parseable evidence block.
- Existing daily sections (Summary/Arc/Clusters/Lessons) still present.

## Checkpoint

```sh
cargo test compact
# live (example):
# cassio compact dailies -i … -o … -l 1 -p openai -m … --base-url …
rg -n "CaseStudyEvidence|instruction_deltas" ~/tmp/cassio-*/**/*.daily.md
```

## Gate

**PASS** if prompts + tests land and one live daily shows the block.  
**NEEDS REVIEW** if live model omits fields often — then tighten prompt or add
repair pass, still in this phase if small.
