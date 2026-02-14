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

## CLI reference

```
cassio [OPTIONS] [PATH]

Arguments:
  [PATH]  Input file or directory (omit for stdin)

Options:
  -o, --output <DIR>     Output directory for batch mode
  -f, --format <FORMAT>  Output format: emoji-text, jsonl [default: emoji-text]
      --all              Discover and process all tools' default paths
      --force            Regenerate even if output is newer than input
  -h, --help             Print help
```

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
```

The AST layer cleanly separates parsing from formatting, making it straightforward to add new input parsers or output formatters.
