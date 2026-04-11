# Training Dataset Export Spec

## Problem

Cassio currently produces human-readable transcripts and a lightweight AST-shaped `jsonl` output. Those are useful for browsing and grep, but they are not sufficient as the canonical basis for a real model-training dataset.

The current pipeline is transcript-oriented and already too lossy for serious training use:

- `emoji-text` intentionally drops thinking blocks and tool-use payloads.
- `jsonl` serializes the normalized AST, not the raw-enough source needed for training.
- parser stages already discard or flatten source material that matters for later SFT, preference, tool-use, and eval dataset derivation.
- redaction is destructive and non-auditable.

The goal of this spec is to define a second artifact, emitted alongside the human transcript, that preserves enough provenance and structure to become the canonical source for real training data work.

## Non-Goals

- Replacing the human transcript format.
- Turning Cassio into a trainer or eval runner.
- Generating SFT, preference, or RL datasets directly during session extraction.
- Preserving every raw byte of every source log with no sanitization.

This spec defines the canonical per-session export only. Derived training corpora come later.

## Design Goals

1. Preserve enough structure for later derivation of:
   - supervised fine-tuning datasets
   - preference datasets
   - eval datasets
   - tool-use datasets
   - summarization datasets
2. Preserve provenance and ordering so the export is reproducible.
3. Preserve both raw-enough content and sanitized content where feasible.
4. Make contamination visible:
   - embedded transcript payloads
   - pasted generated summaries
   - prompt templates
   - other nested historical material
5. Keep the schema provider-agnostic across Claude, Codex, and OpenCode.

## Current State

### Existing AST

Cassio already normalizes sessions into:

- `Session { metadata, messages, stats }`
- per-message `role`, `timestamp`, `model`, `usage`
- content blocks:
  - `Text`
  - `Thinking`
  - `ToolUse`
  - `ToolResult`
  - `ModelChange`
  - `QueueOperation`
- aggregate session stats

This is a solid transcript IR, but it is not a sufficient training IR.

### Known Lossiness

Current losses that matter for training:

- parser-level discarded source records
- collapsed tool outputs
- removed context/ref blocks
- dropped synthetic or meta content without audit trail
- redaction without structured provenance
- no explicit embedded-content markers
- no source-record references or stable event IDs

## Required Output

Cassio should emit a second structured artifact per session:

- human transcript:
  - `YYYY-MM/YYYY-MM-DDTHH-MM-SS-<tool>.md`
- training export:
  - `YYYY-MM/YYYY-MM-DDTHH-MM-SS-<tool>.training.json`

The `.training.json` artifact is the canonical machine-readable session export.

## Architecture

Recommended pipeline:

```text
Raw source logs
  -> parser
  -> normalized training session IR
  -> sanitization pass
  -> .training.json
  -> human transcript formatter
  -> .md
```

Important: the training export must not be reconstructed from markdown or from the current AST dump after information has already been discarded.

## Schema Overview

Top-level shape:

```json
{
  "schema_version": "training_session.v1",
  "cassio_version": "0.x.y",
  "parser_version": "claude.v1",
  "source": {},
  "metadata": {},
  "events": [],
  "stats": {},
  "sanitization": {},
  "quality_flags": {}
}
```

## Top-Level Fields

### `schema_version`

String. Mandatory.

Used for downstream compatibility and migration.

### `cassio_version`

String. Mandatory.

The Cassio build version that produced the export.

### `parser_version`

String. Mandatory.

Identifies the parser/schema interpretation version used for this source type.

Examples:

- `claude.v1`
- `codex.v1`
- `opencode.v1`

## `source`

Provider and provenance information.

```json
{
  "tool": "claude",
  "source_path": "/abs/path/to/source",
  "session_id": "abc",
  "source_hash": "sha256:...",
  "source_record_count": 123
}
```

Required fields:

- `tool`
- `source_path`
- `session_id`
- `source_hash`

Recommended fields:

- `source_record_count`
- `source_format`
- `source_root`

### Notes

- `source_path` is operationally important. Do not drop it from the canonical export.
- `source_hash` must hash the original source material used to build the export, not just the sanitized output.

## `metadata`

Session-level metadata.

```json
{
  "project_path_raw": "/Users/ianzepp/github/ianzepp/abbot",
  "project_path_sanitized": "/Users/ianzepp/github/ianzepp/abbot",
  "started_at": "2026-02-05T19:40:37.835Z",
  "ended_at": "2026-02-05T23:03:33.000Z",
  "git_branch": "main",
  "title": null,
  "session_kind": "human",
  "models_seen": ["gpt-5.2", "opus-4.6"]
}
```

Required fields:

- `project_path_raw`
- `project_path_sanitized`
- `started_at`
- `session_kind`
- `models_seen`

Optional fields:

- `ended_at`
- `git_branch`
- `title`
- `version`

## `events`

This is the core of the export.

Each event must preserve enough structure to reconstruct sequence, role, tool-use linkage, and source provenance.

### Event shape

```json
{
  "event_id": "evt-000123",
  "sequence": 123,
  "timestamp": "2026-02-05T19:58:31.000Z",
  "role": "assistant",
  "event_kind": "message",
  "model": "opus-4.6",
  "raw_text": "...",
  "sanitized_text": "...",
  "embedded_content_flags": {
    "contains_embedded_transcript": false,
    "contains_generated_summary": false,
    "contains_large_pasted_block": false,
    "contains_prompt_template": false
  },
  "tool_name": null,
  "tool_call_id": null,
  "tool_input_raw": null,
  "tool_input_sanitized": null,
  "tool_output_raw": null,
  "tool_output_sanitized": null,
  "usage": null,
  "source_record_refs": ["raw:42"]
}
```

### Mandatory event fields

- `event_id`
- `sequence`
- `event_kind`
- `source_record_refs`

### Strongly recommended event fields

- `timestamp`
- `role`
- `model`
- `raw_text`
- `sanitized_text`
- `embedded_content_flags`

### `event_kind`

Allowed initial values:

- `message`
- `tool_call`
- `tool_result`
- `model_change`
- `queue_operation`
- `system_context`
- `meta_record`

The exact list can expand later, but downstream consumers must not need provider-specific interpretation to understand core event behavior.

### `source_record_refs`

This field is mandatory.

It links each normalized event back to the underlying raw source records or fragments that produced it.

Examples:

- Claude:
  - `["jsonl:42"]`
- Codex:
  - `["jsonl:108"]`
- OpenCode:
  - `["message:msg_user", "part:part1", "part:part2"]`

Without this field, reproducibility and debugging degrade immediately.

## `usage`

Per-event usage where available:

```json
{
  "input_tokens": 100,
  "output_tokens": 50,
  "cache_read_tokens": 400,
  "cache_creation_tokens": 0
}
```

This should remain nullable when the source does not expose event-level usage.

## `stats`

Session aggregates.

```json
{
  "user_messages": 12,
  "assistant_messages": 10,
  "tool_calls": 4,
  "tool_errors": 1,
  "duration_seconds": 2040,
  "files_read": ["a.rs", "b.rs"],
  "files_written": ["c.rs"],
  "files_edited": ["d.rs"],
  "total_tokens": {
    "input_tokens": 1000,
    "output_tokens": 600,
    "cache_read_tokens": 4000,
    "cache_creation_tokens": 0
  },
  "cost_usd": 0.42
}
```

Keep the file lists, not just counts. Counts alone are too lossy for downstream analysis.

## `sanitization`

The sanitization block records what Cassio changed.

```json
{
  "policy_version": "sanitization.v1",
  "redaction_count": 4,
  "redaction_kinds": [
    "api_key",
    "bearer_token",
    "private_key",
    "path_normalization"
  ],
  "dropped_block_count": 2,
  "dropped_block_kinds": [
    "synthetic_context",
    "unsupported_part_type"
  ]
}
```

This block is required. Redaction without auditability is a dataset integrity problem.

## `quality_flags`

Session-level warnings and derivable labels.

```json
{
  "contains_embedded_transcript": true,
  "contains_generated_summary": true,
  "contains_large_pasted_block": false,
  "contains_prompt_template": true,
  "likely_meta_session": true,
  "tool_output_truncated": false,
  "ordering_confidence": "high"
}
```

These flags matter because this corpus includes many sessions that quote earlier sessions or include generated summaries inline.

## Raw vs Sanitized Content

Where feasible, preserve both:

- `raw_text`
- `sanitized_text`

Likewise for tool payloads:

- `tool_input_raw`
- `tool_input_sanitized`
- `tool_output_raw`
- `tool_output_sanitized`

This is important because:

- training consumers may need sanitized text
- audit/debug consumers may need to understand what changed
- downstream filters may need to distinguish true content from redaction artifacts

If certain raw values are too sensitive to retain, the export must still include a structured note that something was removed and why.

## Embedded Content Detection

This corpus has a known contamination pattern: sessions often include pasted or generated text from earlier sessions.

The training export must make this visible.

Required detection targets:

- embedded transcript payloads
- generated daily/monthly summaries
- compaction prompt templates
- large pasted historical blocks

These detections do not need to be perfect in v1, but they must exist as explicit flags.

## Provider-Specific Requirements

### Claude

Preserve beyond current transcript needs:

- raw `message.content`
- tool-use IDs
- raw tool input
- raw tool-result payloads when available
- `isMeta` records as structured events or dropped-with-audit, not silent disappearance

### Codex

Preserve beyond current transcript needs:

- raw `event_msg.user_message`
- context/ref blocks before cleanup
- raw function args
- raw function outputs
- reasoning blocks as gated or separately labeled content, not silent loss
- model shifts from `turn_context`

### OpenCode

Preserve beyond current transcript needs:

- stable part ordering
- raw part text
- synthetic part markers
- raw tool input and output metadata
- message/part IDs in source refs

## Output Naming

Per-session file naming:

- transcript:
  - `YYYY-MM/YYYY-MM-DDTHH-MM-SS-<tool>.md`
- training export:
  - `YYYY-MM/YYYY-MM-DDTHH-MM-SS-<tool>.training.json`

Do not overload the current `jsonl` output mode for this.

`jsonl` today means lightweight AST serialization. A real training export should have a distinct format name, such as:

- `training-json`

## Recommended Implementation Strategy

1. Add a new training-oriented intermediate struct separate from the current transcript AST.
2. Populate it during parsing, before source material is flattened or discarded.
3. Add a dedicated sanitization pass that records what changed.
4. Emit one `.training.json` file per session alongside the transcript.
5. Leave current `emoji-text` and `jsonl` behavior intact.

## Keep / Cut / Rename / Make Mandatory

### Keep

- one canonical per-session structured export
- provider-agnostic top-level schema
- raw plus sanitized parallel fields where feasible
- provenance and source refs everywhere

### Cut

- any attempt to derive training data later from markdown
- any attempt to treat current AST `jsonl` as the canonical training export
- any design that silently drops contamination or redaction provenance

### Rename

- new export format should not be called `jsonl`
- use a distinct name like `training-json`

### Make Mandatory

- `schema_version`
- `cassio_version`
- `parser_version`
- `source_hash`
- `sequence`
- `event_kind`
- `source_record_refs`
- `sanitization`
- explicit contamination flags

## Risks

### If we only serialize the current AST differently

That will produce a cleaner JSON file, but not a real training dataset. The key information is already lost earlier in the pipeline.

### If we do destructive sanitization only

Downstream consumers will not be able to audit what changed or distinguish redaction artifacts from source content.

### If embedded-content detection is omitted

The corpus will continue to contaminate itself during later training-data derivation.

## Future Derived Artifacts

This spec does not define them, but the canonical session export is intended to support later generation of:

- `sft.jsonl`
- `prefs.jsonl`
- `eval.jsonl`

Those should be generated as separate downstream steps, not during transcript extraction.

## Final Recommendation

Do not treat this as “add a JSON formatter.” Treat it as “add a canonical machine artifact before information is lost.”

That is the smallest version worth building that can honestly serve as the basis of a real training dataset.
