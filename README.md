# cassio

Every conversation you have with an AI coding assistant is buried in opaque JSONL logs scattered across your filesystem. Cassio turns them into plain text you can actually read, search, and keep forever.

```
grep "authentication" ~/transcripts/**/*.txt
grep -l "refactor" ~/transcripts/2025-06/*.txt
grep "❌" ~/transcripts/2025-07/*.txt   # find all failures
```

Your AI conversations are a form of long-term memory: decisions made, bugs debugged, architectures explored, dead ends abandoned. Cassio makes that memory greppable. Run it nightly, commit the output to a repo, and you have a searchable history of every session across every tool you use.

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

## Supported tools

Cassio reads the native log format of each tool and normalizes everything into the same AST before formatting.

| Tool | Log format | Default path |
|------|-----------|-------------|
| Claude Code | JSONL (one record per line) | `~/.claude/projects` |
| OpenAI Codex | JSONL (`rollout-*.jsonl` files) | `~/.codex/sessions` |
| OpenCode | Fragmented JSON (session/message/part dirs) | `~/.local/share/opencode/storage` |

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
    2025-11-12T21-52-16-claude.txt
    2025-11-11T14-12-49-codex.txt
  2025-12/
    2025-12-01T09-30-00-opencode.txt
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
cassio --all -o ~/transcripts --force   # regenerate even if output is newer
```

Batch mode skips files whose output is already newer than the input unless `--force` is set.

## Daily compaction

Cassio can compact a day's worth of session transcripts into a structured daily summary using Claude. The compaction preserves every user utterance, compresses LLM behavior to one-liners, marks decision points and corrections, and extracts lessons learned.

```sh
# Compact all pending days (input and output in same directory)
cassio compact dailies -i ~/transcripts

# Separate input and output directories
cassio compact dailies -i ~/transcripts -o ~/dailies

# Limit to 3 days, use opus model
cassio compact dailies -i ~/transcripts --limit 3 --model opus
```

Progress is reported per day:

```
dailies: [1 of 23] 2025-12/2025-12-05... ok
dailies: [2 of 23] 2025-12/2025-12-08... ok
dailies: [3 of 23] 2025-12/2025-12-10... [FAIL]
finished: 2m25s, 2 compacted, 1 failed
```

Output files are written as `YYYY-MM/YYYY-MM-DD.compaction.md` in the output directory. Days that already have a `.compaction.md` are skipped automatically.

The compaction prompt extracts:
- **Session clusters** grouped by topic, with verbatim user quotes
- **Decision points** and **corrections/pushbacks**
- **The arc** of the day's work (intent, pivots, outcome)
- **Lessons learned**: tool failures, correction patterns, confidence signals, and suggested rules for CLAUDE.md/AGENTS.md

## Full pipeline

Run the entire pipeline in one command — sessions, dailies, and monthlies:

```sh
cassio compact all -o ~/transcripts
cassio compact all --model opus
```

This runs three steps in sequence:

1. **Sessions**: discovers all tool sources (Claude, Codex, OpenCode) and converts new JSONL logs to `.txt` transcripts
2. **Dailies**: compacts pending days into `.compaction.md` summaries
3. **Monthlies**: synthesizes months that have compactions but no `.monthly.md` yet

Each step skips work that's already done, so it's safe to run repeatedly (e.g. via cron).

```
=== Step 1: Processing sessions ===

Found 3 source(s): claude, codex, opencode
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

## Monthly compaction

Synthesize a month of daily compactions into a monthly personality and pattern summary:

```sh
cassio compact monthly -i 2025-12
cassio compact monthly -i 2025-12 --model opus
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

## Configuration

Cassio reads persistent defaults from `~/.config/cassio/config.toml`. Use the `get` and `set` subcommands to manage it:

```sh
cassio set output "~/transcripts"   # default output directory
cassio set format jsonl             # default output format
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
```

CLI flags always override config values. With the config above, `cassio --all` just works without `-o`.

### Config keys

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `output` | string | *(none)* | Default output directory |
| `format` | string | `emoji-text` | Default output format (`emoji-text` or `jsonl`) |
| `git.commit` | bool | `false` | Auto-commit output files after processing |
| `git.push` | bool | `false` | Auto-push after committing |
| `sources.claude` | string | `~/.claude/projects` | Override Claude Code log path |
| `sources.codex` | string | `~/.codex/sessions` | Override Codex log path |
| `sources.opencode` | string | `~/.local/share/opencode/storage` | Override OpenCode log path |

## CLI reference

```
cassio [OPTIONS] [PATH] [COMMAND]

Commands:
  compact  Compact transcripts into summaries
  get      Get a config value (e.g. cassio get output)
  set      Set a config value (e.g. cassio set git.commit true)
  unset    Remove a config value (e.g. cassio unset git.push)

Arguments:
  [PATH]  Input file or directory (omit for stdin)

Options:
  -o, --output <DIR>     Output directory for batch mode
  -f, --format <FORMAT>  Output format: emoji-text, jsonl [default: emoji-text]
      --all              Discover and process all tools' default paths
      --force            Regenerate even if output is newer than input
  -h, --help             Print help
```

### cassio compact all

```
cassio compact all [OPTIONS]

Options:
  -m, --model <MODEL>  Claude model to use [default: sonnet]
  -o, --output <DIR>   Directory for transcripts, dailies, and monthlies
```

Runs the full pipeline: sessions → dailies → monthlies. Requires `claude` CLI. Each step skips already-processed items.

### cassio compact dailies

```
cassio compact dailies [OPTIONS]

Options:
  -i, --input <DIR>    Input directory containing session transcripts
  -l, --limit <N>      Maximum number of days to process
  -m, --model <MODEL>  Claude model to use for compaction [default: sonnet]
  -o, --output <DIR>   Output directory for compaction files
```

If `-i` is omitted, falls back to `-o` or config `output`. If `-o` is omitted, falls back to config `output` or `-i`. Requires `claude` CLI to be installed and authenticated.

### cassio compact monthly

```
cassio compact monthly [OPTIONS] --input <YYYY-MM>

Options:
  -i, --input <YYYY-MM>  Month to process (e.g. 2025-12)
  -m, --model <MODEL>    Claude model to use [default: sonnet]
  -o, --output <DIR>     Directory containing month subdirectories
```

Reads `.compaction.md` files from `<output>/<YYYY-MM>/`, writes `<YYYY-MM>.monthly.md` in the same directory. Automatically chunks large months across multiple LLM calls (150KB input budget per call, ~37.5K tokens at 4 bytes/token).

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
                                              Extract → Claude → Daily compaction (md)
```

The AST layer cleanly separates parsing from formatting, making it straightforward to add new input parsers or output formatters. The compaction pipeline operates on formatted output, extracting key signals and sending them through Claude for structured summarization.
