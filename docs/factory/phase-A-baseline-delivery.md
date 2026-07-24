# Phase A Delivery — Experimental Baseline & Findings Lock

## Phase Name

Document the multi-model daily experiment as durable factory ground truth.

## Input

- Session work (2026-07-24) on cassio compact routing
- `~/tmp/cassio-daily-experiment-2026-06-11/`
- `docs/factory/GOAL.md`

## Problem

Experiment results lived only in chat. Factory needs a locked baseline so later
phases do not rediscover routing, proxy constraints, or quality findings.

## Scope

- Capture day under test, models, paths, timings, sizes, chunk counts.
- Record proxy chat-completions necessity (Codex OAuth / Responses-only).
- Record quality verdict: narrative/corrections strong; metrics weak.
- Mark this phase **done** in the ledger (docs-only; no code required).

## Out Of Scope

- Re-running models
- Changing cassio code
- Changing production archive

## Acceptance Criteria

- [x] GOAL references experiment paths and findings.
- [x] Ledger Phase A status is done with concrete artifacts listed.
- [x] Later phases can cite A without chat history.

## Checkpoint

```sh
ls ~/tmp/cassio-daily-experiment-2026-06-11/out-deepseek-v4-pro/2026-06/2026-06-11.daily.md
ls ~/tmp/cassio-daily-experiment-2026-06-11/out-gpt-5.6-luna-medium/2026-06/2026-06-11.daily.md
test -f docs/factory/GOAL.md && test -f docs/factory/factory-ledger.md
```

## Gate

**PASS** — artifacts exist and are referenced from factory docs (this package).
