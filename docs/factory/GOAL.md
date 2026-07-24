# Goal: Process Case-Study Pipeline (Multi-Scale Transcript Evidence)

## Summary

Turn the cassio transcript archive into a **longitudinal engineering-process
dataset** that supports defensible case studies: how guidance, corrections,
agent mix, cost, process adoption, and first-pass vs rework outcomes changed
over time. Summaries exist to serve that analysis — not as nostalgia artifacts.

The pipeline is multi-scale because raw months of large dailies do not fit a
single monthly model call cleanly. Intermediate **weeklies** absorb context
budget pressure while a fixed **CaseStudyEvidence** schema prevents
quote/metrics loss across compressions.

## Problem

1. **Daily size** — modern dailies are often 35–50KB. Thirty days overwhelm a
   single monthly compaction; chunked monthlies lose cross-day structure.
2. **Wrong product shape** — current dailies/monthlies optimize for interaction
   narrative and “personality” patterns. They do not systematically emit the
   metrics needed for process case studies.
3. **Loss without contract** — each LLM stage paraphrases. Without a
   non-negotiable evidence block, weekly and monthly invent trends or drop the
   exact quotes that make claims client-safe.
4. **Split truth sources** — cost, model mix, and turn volume often exist in
   session metadata already, but narrative dailies do not roll them up
   deterministically. Case studies need **numeric rails + quoted incidents**.

## Business Purpose

Answer, with evidence suitable for fractional/full CTO positioning:

| Question | Evidence class |
| --- | --- |
| How did my instructions / standing rules change over time? | instruction deltas + quotes |
| Did correction load drop? What kinds of corrections? | correction counts + taxonomy + quotes |
| How much back-and-forth to ship a feature unit? | user turns / sessions per unit |
| Am I giving less guidance now? Agents better vs process better? | guidance density + process invocation + model controls |
| Did process replace repeated steering? | repeated correction → codified rule timeline |
| Which agents/tools were used, and was that structural or ad hoc? | agent mix + lane labels over time |
| What did that cost? | token/cost rollups by agent/project/week |
| First-pass success vs bugfix/hardening — agents or process? | outcome labels + process/model covariates |

## Goal Boundary

**In scope (cassio + prompts + experiment harness):**

- Define and enforce `CaseStudyEvidence` as a durable daily/weekly field set.
- Harden daily compact + daily merge to emit and preserve that set.
- Deterministic session-level metric rollups (no LLM required for numbers).
- Weekly compaction stage (`*.weekly.md`) with budget-aware rollup.
- Monthly synthesis path that prefers weeklies; keep dailies→monthly as audit.
- Loss-audit protocol comparing dailies→monthly vs dailies→weeklies→monthly.
- Documented multi-model routing for daily generation quality experiments.
- Experiment artifacts and golden samples under a documented path (not the
  production archive by default).

**Primary repo:** `~/work/ianzepp/cassio`  
**Data archive:** `~/personal/transcripts` (read; write only via controlled
experiment dirs or explicit production runs)  
**Related:** `openai-codex-proxy` chat-completions shim (host tooling; already
landed for Luna path)

## Non-Goals

- Full automatic CTO marketing site or customer portal.
- Replacing session search; sessions remain forensic ground truth.
- Embedding/semantic index work (orthogonal; do not couple).
- Reprocessing the entire multi-year archive in phase 1.
- Perfect reconstruction of every tool call from summaries.
- Local small-model production dailies (explicitly excluded for quality).

## Central Invariant

> **Numbers are rails. Quotes are evidence. Narrative is secondary.**  
> No weekly or monthly claim about process improvement is allowed without a
> carry-forward field or a deterministic metric behind it.

## Acceptance Signals (program-level)

- A stranger can open the factory ledger, pick the current phase, and execute
  without chat history.
- One calendar month can be monthly-compacted from weeklies in a **single**
  model call under a documented size budget (or fail the gate with metrics).
- Case-study questions above map 1:1 to fields in the schema + metric rails.
- Loss audit shows weeklies preserve instruction deltas, correction counts,
  and top quotes within defined tolerances.
- At least one sample month produces a monthly that cites week-level evidence
  without inventing agent/cost figures.

## Stop Conditions

- Schema changes that break already-written dailies without a migration note.
- Weekly that rewrites days into a new story without carry-forward tables.
- Monthly that invents cost/agent trends not present in rails or weeklies.
- Writing experimental multi-model outputs into the production archive by
  default (must be opt-in).
- Scope creep into unrelated cassio features (search, embeddings, new parsers).

## Ground Truth (session evidence)

Already established in-session:

- Day under test: **2026-06-11** (15 sessions, multi-chunk daily).
- DeepSeek V4 Pro direct daily → `~/tmp/cassio-daily-experiment-2026-06-11/out-deepseek-v4-pro/`
- GPT-5.6 Luna medium (via codex proxy chat-completions) → `.../out-gpt-5.6-luna-medium/`
- Proxy gained `/v1/chat/completions` because Codex OAuth upstream is
  Responses-only (no chat-completions route; OAuth lacks Platform scopes).
- Daily quality is strong on decisions/corrections/rules; weak on systematic
  counts, outcome labels, and deterministic cost/agent rollups.

## Initial Production Inputs

See `factory-ledger.md` phases A–H.
