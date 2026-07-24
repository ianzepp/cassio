# CaseStudyEvidence Schema

Machine-stable evidence block embedded in daily and weekly compaction markdown.
Narrative sections remain for humans; **this block is the case-study contract**.

## Placement

Every `.daily.md` and `.weekly.md` MUST end with (or include exactly once):

```markdown
## CaseStudyEvidence

```yaml
period: "2026-06-11"
# ...fields...
```
```

- Heading is exact: `## CaseStudyEvidence`
- Body is a single fenced `yaml` block
- One block per file (merge must produce exactly one)

## Design rules

| Rule | Meaning |
| --- | --- |
| Numbers are rails | Prefer `metrics_ref` / deterministic counts over guessed volumes |
| Quotes are verbatim | `quote` fields are USER text, not paraphrases |
| Unknown > invented | Use empty lists / `unknown` enums; never fabricate cost or agents |
| Stable names | Field names below are the API; do not rename casually |

## Field reference

### Root

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `period` | string | yes | Day `YYYY-MM-DD` or week `YYYY-Www` (ISO week) |
| `period_kind` | `day` \| `week` | yes | Discriminator |
| `projects` | string[] | yes | Distinct project paths or short names |
| `volume` | object | yes | Scale counters |
| `agents_used` | object[] | yes | Agent/tool mix (may be partial if rails missing) |
| `instruction_deltas` | object[] | yes | Standing rules codified that period |
| `corrections` | object[] | yes | User corrections with taxonomy |
| `outcomes` | object[] | yes | Work-unit results |
| `process_invocations` | string[] | yes | Named process/skills used |
| `open_threads` | string[] | yes | Unfinished work |
| `case_study_quotes` | string[] | yes | ≤10 best verbatim USER quotes |
| `metrics_ref` | string \| null | yes | Relative path to `*.metrics.json` or null |
| `notes` | string \| null | no | Freeform caveats (not a substitute for fields) |

### `volume`

| Field | Type | Required | Merge |
| --- | --- | --- | --- |
| `sessions` | int | yes | **sum** |
| `user_turns` | int \| null | no | **sum** (null + n = n) |
| `corrections` | int | yes | **sum** (should match `len(corrections)` when complete) |
| `decisions` | int | yes | **sum** |

### `agents_used[]`

| Field | Type | Required | Merge |
| --- | --- | --- | --- |
| `tool` | string | yes | e.g. `codex`, `claude`, `grok` |
| `model` | string \| null | no | e.g. `gpt-5.5` |
| `sessions` | int | yes | **sum** by (tool, model) key |
| `tokens_in` | int \| null | no | **sum** |
| `tokens_out` | int \| null | no | **sum** |
| `cost_usd` | number \| null | no | **sum** — only if known from rails/headers |

If only narrative is available, set `sessions` from transcript count and leave token/cost null. **Do not invent cost.**

### `instruction_deltas[]`

| Field | Type | Required | Merge |
| --- | --- | --- | --- |
| `summary` | string | yes | What rule/process changed |
| `target` | string \| null | no | e.g. `skills/factory`, `AGENTS.md` |
| `quote` | string \| null | no | USER quote if any |
| `codified` | bool | yes | true if written into durable process artifact |

Merge: **union** by normalized `summary` (case-fold); keep first quote/target.

### `corrections[]`

| Field | Type | Required | Merge |
| --- | --- | --- | --- |
| `type` | enum | yes | see below |
| `quote` | string | yes | Verbatim USER correction |
| `context` | string \| null | no | One-line what was wrong |
| `codified_into` | string \| null | no | Artifact path if later codified same day |

**`type` enum:**  
`over_abstraction` | `ignored_clean_break` | `wrong_boundary` | `tool_misuse` | `ignored_instruction` | `over_hedging` | `terminology` | `other`

Merge: **union** by exact `quote` string.

### `outcomes[]`

| Field | Type | Required | Merge |
| --- | --- | --- | --- |
| `unit` | string | yes | Feature/goal/phase label |
| `result` | enum | yes | see below |
| `sessions` | int \| null | no | **sum** if same unit |
| `notes` | string \| null | no | short |

**`result` enum:**  
`autonomous_success` | `first_pass` | `rework` | `hardening` | `deferred` | `abandoned` | `unknown`

Merge: **union** by `unit`; if conflicting `result`, prefer worse outcome order:  
`abandoned` > `deferred` > `hardening` > `rework` > `first_pass` > `autonomous_success` > `unknown`

### `process_invocations`

String tags, e.g. `factory`, `delivery`, `goal-forge`, `poker-face`, `bonsai`, `housekeeping`, `warmup`.

Merge: **set union**.

### `open_threads` / `case_study_quotes`

Merge: **union**; `case_study_quotes` capped at **10** after merge (prefer quotes that appear in `corrections` or `instruction_deltas` first).

### `metrics_ref`

Merge: if any chunk has non-null, keep the day-level rails path (same for all chunks of a day). Weekly sets week-level rails path or null.

## Question → field map (GOAL case studies)

| Case-study question | Fields |
| --- | --- |
| How did instructions / standing rules change? | `instruction_deltas` |
| Did correction load change? What kinds? | `volume.corrections`, `corrections[].type` |
| Back-and-forth per feature unit? | `outcomes[].sessions`, `volume.user_turns`, `outcomes[].unit` |
| Less guidance now? Agents vs process? | `process_invocations`, `volume.user_turns`, `agents_used`, time series across periods |
| Process replacing repetition? | `instruction_deltas` where `codified=true` + prior `corrections` |
| Agents used how? Structural vs ad hoc? | `agents_used`, multi-week variance |
| Cost structure? | `agents_used[].cost_usd`, `metrics_ref` |
| First-pass vs bugfix/hardening? | `outcomes[].result` |

## Daily merge rules (chunk partials → one day)

1. Parse each partial's CaseStudyEvidence YAML (if missing, treat as empty required lists / zero volume — do not invent).
2. Apply field merge table above.
3. Set `period` to the day; `period_kind: day`.
4. Recompute `volume.corrections` as `corrections.len()` when corrections list is authoritative.
5. Emit **one** CaseStudyEvidence section on the final daily.
6. Narrative merge remains per `daily_merge.md`; evidence is never dropped to save space.

## Weekly rollup rules (dailies → one week)

1. `period` = ISO week id; `period_kind: week`.
2. Same merge operators across all dailies in the week.
3. `metrics_ref` → week metrics file if present, else null.
4. Cap `case_study_quotes` at 10.
5. Weekly narrative must not rewrite evidence; tables/YAML win.

## Monthly rules (consumers)

- May aggregate trends and cite quotes from evidence blocks.
- **MUST NOT** invent `cost_usd`, agent lists, or correction counts absent from evidence or metrics rails.
- Prefer summing rails JSON over reading prose Summary bullets for numbers.

## Validation (soft v1)

A checker SHOULD warn if:

- heading/fence missing
- required keys missing
- `volume.corrections` disagrees with `corrections.len()` by >0 when both present
- `case_study_quotes` > 10
- unknown enum values

Hard-fail is optional for CI later; compact pipeline v1 warns only.

## Example: 2026-06-11

See `docs/factory/examples/2026-06-11.case-study-evidence.yaml` (filled from Phase A Luna/DeepSeek dailies + session inventory).
