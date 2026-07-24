# Phase H Delivery — Model Routing Harness

## Phase Name

Make multi-model daily experiments repeatable and production routing explicit.

## Input

- Phase A experiment shell patterns
- Phase C daily contract
- Host tools: DeepSeek direct, openai-codex-proxy chat-completions, OpenRouter

## Problem

Model comparison was ad hoc. Case studies and quality control need a harness:
same day, many models, isolated outputs, recorded provider config.

## Scope

- Document routing matrix (provider, model, base_url, auth env, notes):
  - DeepSeek direct (`openai` + `https://api.deepseek.com` + key bridge)
  - Luna/Sol/Terra via proxy (`openai` + `http://127.0.0.1:18181/v1`)
  - OpenRouter (`openrouter` + `OPENROUTER_API_KEY`)
- Add a small harness script under `scripts/` or `docs/factory/scripts/`:
  - stage one day of sessions
  - run N model configs into `out-<label>/`
  - write `manifest.json` (model, timing, exit, output path, bytes)
- Optional: config file listing model profiles (no secrets in repo).
- Recommend primary production daily model after harness run on 2026-06-11
  **with CaseStudyEvidence present** (post-C), scored for evidence completeness
  not just prose beauty.

## Out Of Scope

- Training a router model
- Changing default cassio config without user approval
- Local llama production dailies

## Acceptance Criteria

- Manifested multi-model run documented and reproducible from README/script.
- Production recommendation recorded in ledger with evidence-completeness notes.
- Secrets never committed; only env var names.

## Checkpoint

```sh
# example
./scripts/compact-day-models.sh 2026-06-11 profiles.example.toml
cat ~/tmp/cassio-daily-experiment-*/manifest.json
```

## Gate

**PASS** if harness + docs + recommendation exist.  
**NEEDS REVIEW** if only one model can complete evidence block reliably.
