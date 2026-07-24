# Phase G Delivery — Loss Audit Protocol

## Phase Name

Prove weeklies preserve case-study fidelity versus direct dailies→monthly.

## Input

- Same month (or synthetic multi-week fixture) with:
  - dailies containing CaseStudyEvidence
  - weeklies from those dailies
  - monthly-from-weeklies
  - monthly-from-dailies (chunked if needed)
- Phase B schema question map

## Problem

Weeklies only help if they do not destroy the evidence needed for case studies.
We need a scored audit, not vibes.

## Scope

- Write `docs/factory/loss-audit-protocol.md` with pass/fail tolerances:
  - instruction_deltas: 100% present in weekly union (or listed as dropped)
  - correction counts: sum dailies == weekly (exact)
  - case_study_quotes: ≥80% of daily top-quotes appear verbatim in weekly or
    monthly-from-weeklies
  - metrics: monthly must not invent numbers absent from rails
  - no new projects/agents invented in monthly
- Implement a checker script or `cassio audit loss …` that scores machine-checkable
  items (quotes/counts). Human rubric for narrative invention.
- Run audit once on experiment data; store report under
  `docs/factory/reports/` or `~/tmp/…` with path recorded in ledger.
- Go/no-go recommendation for enabling weeklies in production archive.

## Out Of Scope

- Perfect semantic equivalence of arcs
- Full-year audit

## Acceptance Criteria

- Protocol doc with numeric tolerances exists.
- At least one audit report generated.
- Ledger records go/no-go for production weeklies.

## Checkpoint

```sh
# after tool exists:
cassio audit loss --month YYYY-MM --dailies-monthly PATH --weeklies-monthly PATH
# or scripted equivalent
```

## Gate

**PASS** if protocol + one report land and go/no-go is explicit.  
**FAIL** if recommendation is “looks fine” without scores.
