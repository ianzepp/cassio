# cassio

Every conversation you have with an AI coding assistant is buried in opaque JSONL logs scattered across your filesystem. Cassio turns them into plain text you can actually read, search, and keep forever.

```
grep "authentication" ~/transcripts/**/*.md
grep -l "refactor" ~/transcripts/2025-06/*.md
grep "❌" ~/transcripts/2025-07/*.md   # find all failures
```

Your AI conversations are a form of long-term memory: decisions made, bugs debugged, architectures explored, dead ends abandoned. Cassio makes that memory greppable. Run it nightly, commit the output to a repo, and you have a searchable history of every session across every tool you use.

## Quick install

```sh
curl -fsSL https://raw.githubusercontent.com/ianzepp/cassio/master/install.sh | bash
```

Or via Homebrew:

```sh
brew tap ianzepp/tap && brew install cassio
```

Or build from source:

```sh
git clone https://github.com/ianzepp/cassio.git && cd cassio && cargo build --release
```

## What the output looks like

### emoji-text (default)

```
📋 Session: abc-123
📋 Project: /Users/you/my-app
📋 Started: 2025-11-12T21:52:16.079+00:00
📋 Version: 2.0.37
📋 Branch: main

📋 Model: sonnet-4.5
👤 Hello, can you help me refactor this module?
🤖 Sure! Let me take a look at the code first.
✅ Read: file="src/lib.rs"
✅ Bash: cargo test
❌ Bash: cargo build
🤖 I found the issue. Here's the fix...
✅ Edit: file="src/lib.rs"

📋 --- Summary ---
📋 Duration: 5m
📋 Messages: 2 user, 3 assistant
📋 Tool calls: 4 total, 1 failed
📋 Files: 1 read, 1 edited
📋 Tokens: 12.5K in, 3.2K out
📋 Cache: 280.9K read, 13.3K created
```

Each line starts with an emoji that tells you what it is at a glance:

| Emoji | Meaning |
|-------|---------|
| 📋 | Metadata / summary |
| 👤 | User message |
| 🤖 | Assistant message |
| ✅ | Successful tool call |
| ❌ | Failed tool call |
| ⏳ | Queue operation |

### jsonl

Structured JSONL for programmatic consumption. Metadata on the first line, one message per line, stats on the last:

```jsonl
{"session_id":"abc-123","tool":"claude","project_path":"/project","started_at":"2025-11-12T21:52:16.079Z","version":"2.0.37","model":"sonnet-4.5"}
{"role":"user","timestamp":"2025-11-12T21:54:35Z","content":[{"type":"text","text":"Hello"}],"usage":null}
{"role":"assistant","timestamp":"2025-11-12T21:54:42Z","content":[{"type":"text","text":"Hi!"}],"usage":{"input_tokens":100,"output_tokens":50}}
{"user_messages":1,"assistant_messages":1,"tool_calls":0,"tool_errors":0,"duration_seconds":7}
```

### training-json

Canonical machine-readable session export for downstream dataset work. In batch mode, `emoji-text` now writes a sibling `*.training.json` file beside each transcript. You can also emit it directly:

```sh
cassio --format training-json session.jsonl
```

## Supported tools

Cassio reads the native log format of each tool and normalizes everything into the same AST before formatting.

| Tool | Log format | Default path |
|------|-----------|-------------|
| Claude Code | JSONL (one record per line) | `~/.claude/projects` |
| OpenAI Codex | JSONL (`rollout-*.jsonl` files) | `~/.codex/sessions` |
| OpenCode | Fragmented JSON (session/message/part dirs) | `~/.local/share/opencode/storage` |
| pi | JSONL (one record per line) | `~/.pi/agent/sessions` |

Format detection is automatic based on file paths and content.

## Usage

### Process a single file

```sh
cassio session.jsonl
```

### Pipe from stdin

```sh
cat session.jsonl | cassio
```

### Batch mode: directory in, directory out

```sh
cassio ~/.claude/projects -o ~/transcripts
```

Output is organized into `YYYY-MM/` subdirectories:

```
~/transcripts/
  2025-11/
    2025-11-12T21-52-16-claude.md
    2025-11-12T21-52-16-claude.training.json
    2025-11-11T14-12-49-codex.md
    2025-11-11T14-12-49-codex.training.json
  2025-12/
    2025-12-01T09-30-00-opencode.md
    2025-12-01T09-30-00-opencode.training.json
```

### Process everything at once

```sh
cassio --all -o ~/transcripts
```

Discovers all installed tools and processes their logs in one pass.

### Nightly cron

Add to your crontab for automatic transcript generation:

```sh
0 3 * * * cassio --all -o ~/transcripts && cd ~/transcripts && git add -A && git commit -m "$(date +%F)"
```

### Other options

```sh
cassio --format jsonl session.jsonl     # JSONL output instead of text
cassio --format training-json session.jsonl
cassio --all -o ~/transcripts --force   # regenerate even if output is newer
```

Batch mode skips files whose output is already newer than the input unless `--force` is set.

## Configuration

Generate a default config file with all options:

```sh
cassio init
```

```
Created config file: ~/.config/cassio/config.toml

Edit it directly, or use:
  cassio set output ~/transcripts
  cassio set git.commit true
  cassio get
```

Manage individual values:

```sh
cassio set output "~/transcripts"   # default output directory
cassio set format jsonl             # default output format
cassio set format training-json
cassio set git.commit true          # auto-commit after processing
cassio set git.push true            # auto-push after committing

cassio get                          # list all config values
cassio get output                   # get a single value
cassio unset git.push               # remove a value
```

The resulting config file:

```toml
output = "~/transcripts"
format = "jsonl"

[git]
commit = true
push = true

# Override default source paths (optional)
# [sources]
# claude = "~/.claude/projects"
# codex = "~/.codex/sessions"
# opencode = "~/.local/share/opencode/storage"
# pi = "~/.pi/agent/sessions"
```

CLI flags always override config values. With the config above, `cassio --all` just works without `-o`.

### Config keys

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `output` | string | *(none)* | Default output directory |
| `format` | string | `emoji-text` | Default output format (`emoji-text`, `jsonl`, or `training-json`) |
| `model` | string | `llama3.1` | Default model name (passed to the selected provider) |
| `provider` | string | `ollama` | LLM provider for compaction (`ollama`, `claude`, `codex`, `openrouter`, or `openai`) |
| `base_url` | string | *(none)* | Base URL for `provider = "openai"`, such as a local llama.cpp `/v1` endpoint |
| `git.commit` | bool | `false` | Auto-commit output files after processing |
| `git.push` | bool | `false` | Auto-push after committing |
| `sources.claude` | string | `~/.claude/projects` | Override Claude Code log path |
| `sources.codex` | string | `~/.codex/sessions` | Override Codex log path |
| `sources.opencode` | string | `~/.local/share/opencode/storage` | Override OpenCode log path |
| `sources.pi` | string | `~/.pi/agent/sessions` | Override pi log path |

## Summary statistics

Get a quick overview of your transcript history:

```sh
cassio summary -o ~/transcripts
```

```
| Month | claude | codex | opencode | pi | Total | Tokens | Duration |
|-------|--------|-------|----------|-------|--------|----------|
| 2025-11 | 554 | 36 | 98 | 688 | 399.8M | 198h 30m |
| 2025-12 | 1028 | 35 | 374 | 1437 | 32.1M | 331h 16m |
| **Total** | **1582** | **71** | **472** | **2125** | **431.9M** | **529h 46m** |
```

For per-project detail:

```sh
cassio summary --detailed -o ~/transcripts
```

```
| Project | Sessions | User | Asst | Tools (ok/fail) | Tokens (in/out) | Duration |
|---------|----------|------|------|-----------------|-----------------|----------|
| github/ianzepp/faber | 790 | 5069 | 13598 | 33864/2230 | 46.8M/6.1M | 163h 31m |
| github/ianzepp/abbot | 923 | 3869 | 13920 | 34543/917 | 25.5M/4.5M | 173h 36m |
| **Total** | ... | ... | ... | ... | ... | ... |
```

## Daily compaction

Cassio can compact a day's worth of session transcripts into a structured daily summary using a local or cloud LLM. Supported providers are **ollama** (default), **claude**, **codex**, **openrouter**, and **openai** for local or self-hosted OpenAI-compatible endpoints such as llama.cpp. The compaction preserves every user utterance, compresses LLM behavior to one-liners, marks decision points and corrections, and extracts lessons learned.

```sh
# Compact all pending days (input and output in same directory)
cassio compact dailies -i ~/transcripts

# Separate input and output directories
cassio compact dailies -i ~/transcripts -o ~/dailies

# Limit to 3 days, use a specific model
cassio compact dailies -i ~/transcripts --limit 3 --model llama3.1

# Use Claude instead of Ollama
cassio compact dailies -i ~/transcripts --provider claude --model sonnet

# Use a local llama.cpp server with its OpenAI-compatible endpoint
cassio compact dailies -i ~/transcripts --provider openai --base-url http://127.0.0.1:18173/v1 --model local
```

Progress is reported per day:

```
dailies: [1 of 23] 2025-12/2025-12-05... ok
dailies: [2 of 23] 2025-12/2025-12-08... ok
dailies: [3 of 23] 2025-12/2025-12-10... [FAIL]
finished: 2m25s, 2 compacted, 1 failed
```

Output files are written as `YYYY-MM/YYYY-MM-DD.daily.md` in the output directory. Days that already have a `.daily.md` are skipped automatically.

Chunked days resume by default from on-disk state. Cassio caches successful chunk summaries under `YYYY-MM/.cassio-checkpoints/YYYY-MM-DD/`, writes machine-readable day state to `status.json`, appends structured chunk progress events to `progress.jsonl`, and marks chunks as finalized only after the downstream daily summary is written successfully.

Failures are classified as `timeout`, `provider_http_error`, `parse_error`, `empty_response`, `process`, `transport`, or `io`. On parse failures, cassio also writes the raw failing provider response into the same checkpoint directory for debugging.

Each chunk or merge request uses a per-call timeout and bounded retries. The defaults are 5 minutes and 3 retries, configurable with `--chunk-timeout` and `--max-retries`. If a run finishes with one or more failed days, `cassio compact dailies` exits with code `2` and names the exact failed day and chunk or merge phase.

The compaction prompt extracts:
- **Session clusters** grouped by topic, with verbatim user quotes
- **Decision points** and **corrections/pushbacks**
- **The arc** of the day's work (intent, pivots, outcome)
- **Lessons learned**: tool failures, correction patterns, confidence signals, and suggested rules for CLAUDE.md/AGENTS.md

## Monthly compaction

Synthesize a month of daily compactions into a monthly personality and pattern summary:

```sh
cassio compact monthly -i 2025-12
cassio compact monthly -i 2025-12 --model llama3.1
```

The monthly prompt aggregates patterns across all daily compactions for the month — what recurs, what evolves, and what's distinctive. It preserves direct quotes as evidence, counts when possible, and separates stable traits from evolving ones.

If the compactions exceed the LLM context limit (~150KB), cassio automatically chunks them and runs a multi-pass summarization:

```
monthly: 2026-01 (31 compaction files)
  chunked: 5 chunks (648884 bytes total)
  chunk [1 of 5] 2026-01-01 to 2026-01-06... ok
  chunk [2 of 5] 2026-01-07 to 2026-01-12... ok
  chunk [3 of 5] 2026-01-13 to 2026-01-19... ok
  chunk [4 of 5] 2026-01-20 to 2026-01-25... ok
  chunk [5 of 5] 2026-01-26 to 2026-01-31... ok
  merging 5 chunk summaries... ok
finished: 4m12s, wrote 2026-01.monthly.md
```

Small months that fit in a single pass skip the chunking step entirely. Output is written as `YYYY-MM/YYYY-MM.monthly.md`. Skips if the monthly already exists.

The monthly summary includes:
- **Stable interaction patterns** with supporting quotes and frequency counts
- **Evolving patterns** with timeline and evidence
- **Decision-making profile**, **correction/pushback profile**
- **Workflow structure** and **tool/process usage**
- **Notable quotes** (10-20) capturing voice and working style

## Full pipeline

Run the entire pipeline in one command — sessions, dailies, and monthlies:

```sh
cassio compact all -o ~/transcripts
cassio compact all --model llama3.1
```

This runs three steps in sequence:

1. **Sessions**: discovers all tool sources (Claude, Codex, OpenCode, pi) and converts new JSONL logs to `.md` transcripts
2. **Dailies**: compacts pending days into `.daily.md` summaries
3. **Monthlies**: synthesizes months that have compactions but no `.monthly.md` yet

Each step skips work that's already done, so it's safe to run repeatedly (e.g. via cron).

```
=== Step 1: Processing sessions ===

Found 4 source(s): claude, codex, opencode, pi
...
  Done: 14 processed, 99 skipped, 5005 up-to-date

=== Step 2: Compacting dailies ===

dailies: [1 of 14] 2026-02/2026-02-01... ok
...
finished: 8m30s, 14 compacted, 0 failed

=== Step 3: Compacting monthlies ===

Found 1 pending month(s): 2026-02
monthly: 2026-02 (14 compaction files)
  single pass (42381 bytes)
  processing... ok
finished: 45s, wrote 2026-02.monthly.md
```

## CLI reference

```
cassio [OPTIONS] [PATH] [COMMAND]

Commands:
  init     Create a default config file
  summary  Show summary statistics for transcripts
  search   Search transcript outputs with summary-first ranking
  compact  Compact transcripts into daily/monthly analysis
  get      Get a config value (e.g. cassio get output)
  set      Set a config value (e.g. cassio set git.commit true)
  unset    Remove a config value (e.g. cassio unset git.push)

Arguments:
  [PATH]  Input file or directory (omit for stdin)

Options:
  -o, --output <DIR>     Output directory for batch mode
  -f, --format <FORMAT>  Output format: emoji-text, jsonl, training-json [default: emoji-text]
      --all              Discover and process all tools' default paths
      --force            Regenerate even if output is newer than input
  -h, --help             Print help
```

### cassio summary

```
cassio summary [OPTIONS]

Options:
      --detailed         Show per-project detailed stats instead of month×tool overview
  -o, --output <DIR>     Directory containing transcript files
```

Regular mode shows a month × tool session count table with token and duration totals. `--detailed` shows a per-project breakdown with message counts, tool usage, and token spend.

## Search

Use Cassio search when plain `grep` or `rg` is too unstructured. It searches
Cassio's generated artifacts in retrieval order: monthly summaries, daily
compactions, session transcripts, and optionally training JSON metadata.

```sh
cassio search "launchd rsync rollback" -o ~/transcripts
cassio search "skill-author" --month 2026-04
cassio search "Permissions could not be resolved" --summaries-only
cassio search "session_id|source_path" --regex --include-training --json
```

Literal queries are split on whitespace and ANDed on each line, so
`cassio search "launchd rsync rollback"` finds lines containing all three terms
without requiring that exact phrase. Use `--regex` for regular-expression
matching. The command uses config `output` when `-o` is omitted.

```
cassio search [OPTIONS] <QUERY>

Options:
  -m, --month <YYYY-MM>       Restrict search to one month directory
  -l, --limit <N>             Maximum matches to print [default: 50]
      --summaries-only        Search only monthly and daily summary files
      --include-training      Include *.training.json after markdown hits
      --regex                 Treat query as a regular expression
      --case-sensitive        Use case-sensitive matching
      --json                  Emit JSON instead of text
  -o, --output <DIR>          Directory containing transcript files
```

### cassio compact dailies

```
cassio compact dailies [OPTIONS]

Options:
  -i, --input <DIR>    Input directory containing session transcripts
  -l, --limit <N>      Maximum number of days to process
  -m, --model <MODEL>     Model name passed to the selected provider
  -p, --provider <NAME>  LLM provider: ollama, claude, codex, openrouter, or openai
      --base-url <URL>   Base URL for provider=openai, such as http://127.0.0.1:18173/v1
      --chunk-timeout <SECONDS>  Per-call timeout for each chunk or merge request [default: 300]
      --max-retries <N>          Maximum retries for each chunk or merge request [default: 3]
  -o, --output <DIR>      Output directory for compaction files
```

If `-i` is omitted, falls back to `-o` or config `output`. If `-o` is omitted, falls back to config `output` or `-i`. CLI providers require the selected provider CLI to be installed. The `openai` provider requires `base_url` and calls `/chat/completions` directly, appending that path to the base URL when needed. Resume is the default behavior whenever checkpoint state already exists on disk.

Exit status:
- `0` = all requested days compacted cleanly
- `2` = run completed with partial failures
- `1` = fatal setup or execution error

### cassio compact monthly

```
cassio compact monthly [OPTIONS] --input <YYYY-MM>

Options:
  -i, --input <YYYY-MM>  Month to process (e.g. 2025-12)
  -m, --model <MODEL>     Model name passed to the selected provider
  -p, --provider <NAME>  LLM provider: ollama, claude, codex, openrouter, or openai
      --base-url <URL>   Base URL for provider=openai, such as http://127.0.0.1:18173/v1
  -o, --output <DIR>      Directory containing month subdirectories
```

Reads `.daily.md` files from `<output>/<YYYY-MM>/`, writes `<YYYY-MM>.monthly.md` in the same directory. Automatically chunks large months across multiple LLM calls (150KB input budget per call, ~37.5K tokens at 4 bytes/token).

### cassio compact all

```
cassio compact all [OPTIONS]

Options:
  -m, --model <MODEL>     Model name passed to the selected provider
  -p, --provider <NAME>  LLM provider: ollama, claude, codex, openrouter, or openai
      --base-url <URL>   Base URL for provider=openai, such as http://127.0.0.1:18173/v1
      --chunk-timeout <SECONDS>  Per-call timeout for each chunk or merge request [default: 300]
      --max-retries <N>          Maximum retries for each chunk or merge request [default: 3]
  -o, --output <DIR>      Directory for transcripts, dailies, and monthlies
```

Runs the full pipeline: sessions → dailies → monthlies. CLI providers require the selected provider CLI to be installed. The `openai` provider calls an OpenAI-compatible `/chat/completions` endpoint configured by `base_url`. Each step skips already-processed items.

## Install

```sh
git clone https://github.com/ianzepp/cassio.git
cd cassio
cargo build --release
# Binary at target/release/cassio
```

## Architecture

```
Input (JSONL/JSON) → Parser → AST (Session) → Formatter → Output (txt/jsonl)
                                                              ↓
                                               Extract → LLM provider → Daily compaction (md)
```

The AST layer cleanly separates parsing from formatting, making it straightforward to add new input parsers or output formatters. The compaction pipeline operates on formatted output, extracting key signals and sending them through the selected LLM provider for structured summarization.
