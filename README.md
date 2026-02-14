# cassio

A Rust CLI that processes conversation logs from Claude Code, OpenAI Codex, and OpenCode into human-readable transcripts or structured JSONL.

```
Input (JSONL/JSON) → Parser → AST (Session) → Formatter → Output (txt/jsonl)
```

## Install

```sh
cargo build --release
# Binary at target/release/cassio
```

## Usage

### Single file to stdout

```sh
cassio session.jsonl
```

### Pipe from stdin

```sh
cat session.jsonl | cassio
```

### Choose output format

```sh
cassio --format emoji-text session.jsonl   # default
cassio --format jsonl session.jsonl
```

### Batch mode (directory → directory)

```sh
cassio ~/.claude/projects -o ~/transcripts
```

Output is organized into `YYYY-MM/` subdirectories with tool-suffixed filenames:

```
~/transcripts/
  2025-11/
    2025-11-12T21-52-16-claude.txt
    2025-11-11T14-12-49-codex.txt
  2025-12/
    2025-12-01T09-30-00-opencode.txt
```

### Discover and process all tools

```sh
cassio --all -o ~/transcripts
```

This checks the default paths for each tool:

| Tool | Default path |
|------|-------------|
| Claude Code | `~/.claude/projects` |
| OpenAI Codex | `~/.codex/sessions` |
| OpenCode | `~/.local/share/opencode/storage` |

### Skip unchanged files

By default, batch mode skips files whose output is already newer than the input. Use `--force` to regenerate everything:

```sh
cassio --all -o ~/transcripts --force
```

## Input formats

### Claude Code (JSONL)

One JSON object per line. Each line has `type` (`user`, `assistant`, `queue-operation`), `sessionId`, `timestamp`, `cwd`, `version`, and a `message` object containing `role`, `content` blocks, and `usage`.

```jsonl
{"type":"user","sessionId":"abc-123","timestamp":"2025-11-12T21:54:35.451Z","cwd":"/project","version":"2.0.37","message":{"role":"user","content":"Hello"}}
{"type":"assistant","sessionId":"abc-123","timestamp":"2025-11-12T21:54:42.848Z","cwd":"/project","version":"2.0.37","message":{"role":"assistant","model":"claude-sonnet-4-5-20250929","content":[{"type":"text","text":"Hi there!"}],"usage":{"input_tokens":100,"output_tokens":50}}}
```

### OpenAI Codex (JSONL)

One JSON object per line. Each line has `timestamp`, `type` (`session_meta`, `response_item`, `event_msg`, `turn_context`), and a `payload` object. Files are named `rollout-YYYY-MM-DDTHH-MM-SS-<uuid>.jsonl`.

```jsonl
{"timestamp":"2025-11-11T14:12:49Z","type":"session_meta","payload":{"id":"sess-1","cwd":"/project","cli_version":"0.1.0","model_provider":"openai"}}
{"timestamp":"2025-11-11T14:13:00Z","type":"event_msg","payload":{"type":"user_message","message":"Build a hello world app"}}
{"timestamp":"2025-11-11T14:13:05Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Sure, I'll create that for you."}]}}
```

### OpenCode (fragmented JSON)

Data is spread across three directories under the storage root (`~/.local/share/opencode/storage`):

```
storage/
  session/<project-id>/<session-id>.json
  message/<session-id>/<message-id>.json
  part/<message-id>/<part-id>.json
```

Each file is a standalone JSON object. cassio assembles them by session ID.

## Output formats

### emoji-text (default)

Human-readable text with emoji prefixes:

```
📋 Session: abc-123
📋 Project: /project
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

Emoji key:

| Emoji | Meaning |
|-------|---------|
| 📋 | Metadata / summary |
| 👤 | User message |
| 🤖 | Assistant message |
| ✅ | Successful tool call |
| ❌ | Failed tool call |
| ⏳ | Queue operation |

### jsonl

Structured JSONL with the session metadata on the first line, one message per line, and session stats on the last line:

```jsonl
{"session_id":"abc-123","tool":"claude","project_path":"/project","started_at":"2025-11-12T21:52:16.079Z","version":"2.0.37","model":"sonnet-4.5"}
{"role":"user","timestamp":"2025-11-12T21:54:35Z","content":[{"type":"text","text":"Hello"}],"usage":null}
{"role":"assistant","timestamp":"2025-11-12T21:54:42Z","content":[{"type":"text","text":"Hi!"}],"usage":{"input_tokens":100,"output_tokens":50,"cache_read_tokens":0,"cache_creation_tokens":0}}
{"user_messages":1,"assistant_messages":1,"tool_calls":0,"tool_errors":0,"duration_seconds":7}
```

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
