# Phase F Delivery — Monthly via Weeklies

## Phase Name

Make monthly synthesis prefer weeklies as input while keeping dailies→monthly
available for audit.

## Input

- Phase E weeklies
- `src/prompts/monthly.md` / `monthly_merge.md`
- Current `run_monthly` behavior

## Problem

Monthlies currently ingest dailies. That forces chunking and weakens
longitudinal structure. Weeklies should be the default monthly diet.

## Scope

- Update monthly input discovery: prefer `*.weekly.md` for the month when
  present and complete enough; fall back to dailies with a clear stderr note.
- Define “complete enough” (e.g. all ISO weeks with dailies covered, or
  explicit `--from dailies|weeklies` flag).
- Adjust `monthly.md` to:
  - aggregate patterns **and** CaseStudyEvidence trends;
  - forbid inventing cost/agent numbers not in rails/evidence;
  - keep quote requirements.
- CLI flags for force path: `--source weeklies|dailies|auto`.
- Tests for input selection logic.
- Produce one experiment monthly from weeklies for a sample month (may be
  partial if only one week of evidence exists — document).

## Out Of Scope

- Formal loss scoring (Phase G)
- Quarterly rollup
- Archive-wide backfill

## Acceptance Criteria

- Auto mode uses weeklies when available.
- Explicit `--source dailies` still works.
- Monthly prompt forbids fabricated metrics.
- Selection logic covered by tests.

## Checkpoint

```sh
cargo test
cassio compact monthly --help
# experiment monthly out of weeklies when ready
```

## Gate

**PASS** if dual-path selection works and prompt/contract updates land.  
**NEEDS REVIEW** if “complete enough” rule is still ambiguous — resolve in-phase.
