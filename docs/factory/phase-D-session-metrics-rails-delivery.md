# Phase D Delivery — Session Metrics Rails

## Phase Name

Deterministic cost, agent mix, and volume metrics from session transcripts
without an LLM.

## Input

- Session `.md` metadata (tokens, cost, model, project, tool)
- Optional `*.training.json` under `training_output`
- Phase B `metrics_ref` field design

## Problem

Case studies need graphs: spend by agent, turns over time, model mix. LLM
summaries must not invent these numbers. Session files already carry much of it.

## Scope

- Define a stable metrics document or JSON shape for a day (and later week).
- Implement a cassio subcommand or library path, e.g.:
  - `cassio metrics day YYYY-MM-DD`
  - `cassio metrics week YYYY-Www`
  - or `cassio summary --metrics` extension — pick one clean CLI surface.
- Emit for a day:
  - session count
  - user/assistant message counts if available
  - token in/out aggregates
  - cost aggregate when present
  - breakdown by model and by tool (claude/codex/grok/…)
  - project histogram
- Write output beside experiment/archive under a clear name, e.g.
  `YYYY-MM-DD.metrics.json` (gitignore policy: decide; default allow under
  transcripts if user enables, else experiment dir).
- Document how dailies set `metrics_ref` to this file.

## Out Of Scope

- Inferring “feature units” automatically
- Pricing table expansion beyond existing `pricing.rs` behavior
- Weekly LLM synthesis (E)

## Acceptance Criteria

- Metrics for `2026-06-11` match hand-checked samples from session headers.
- Command is deterministic (same inputs → same JSON).
- Docs describe rails vs CaseStudyEvidence boundary.
- Tests cover parsing aggregates on fixture sessions.

## Checkpoint

```sh
cargo test
cassio metrics day 2026-06-11 -o /tmp/cassio-metrics-test   # final flag names per impl
python3 -c "import json; json.load(open('/tmp/cassio-metrics-test/...'))"
```

## Gate

**PASS** if deterministic day metrics work and tests cover aggregates.  
**FAIL** if implementation only prints human text without stable machine form.
