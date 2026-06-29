use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

fn temp_dir(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("cassio-hermes-{name}-{unique}"))
}

#[test]
fn parses_legacy_json_with_tool_call_and_result() {
    let dir = temp_dir("json");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session_20260508_183040_abc123.json");
    fs::write(
        &path,
        r#"{
          "session_id": "20260508_183040_abc123",
          "session_start": "2026-05-08T18:30:40.000000",
          "last_updated": "2026-05-08T18:31:00.000000",
          "platform": "tui",
          "model": "gpt-5.4",
          "messages": [
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": "checking", "tool_calls": [
              {"id": "call_1", "function": {"name": "read_file", "arguments": "{\"path\":\"/tmp/a.rs\"}"}}
            ]},
            {"role": "tool", "tool_call_id": "call_1", "name": "read_file", "content": "contents"},
            {"role": "assistant", "content": "done"}
          ]
        }"#,
    )
    .unwrap();

    let parsed = HermesParser.parse_export(&path).unwrap();
    assert_eq!(parsed.session.metadata.tool, Tool::Hermes);
    assert_eq!(parsed.session.stats.user_messages, 1);
    assert_eq!(parsed.session.stats.assistant_messages, 2);
    assert_eq!(parsed.session.stats.tool_calls, 1);
    assert!(parsed.session.stats.files_read.contains("/tmp/a.rs"));
    assert_eq!(
        parsed.training.source.source_format.as_deref(),
        Some("hermes.session-json")
    );

    fs::remove_dir_all(dir).ok();
}

#[test]
fn parses_state_db_virtual_path() {
    let dir = temp_dir("db");
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("state.db");
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (
            id TEXT PRIMARY KEY, source TEXT NOT NULL, model TEXT, title TEXT,
            started_at REAL NOT NULL, ended_at REAL, input_tokens INTEGER,
            output_tokens INTEGER, cache_read_tokens INTEGER, cache_write_tokens INTEGER,
            estimated_cost_usd REAL, actual_cost_usd REAL
         );
         CREATE TABLE messages (
            id INTEGER PRIMARY KEY, session_id TEXT NOT NULL, role TEXT NOT NULL,
            content TEXT, tool_call_id TEXT, tool_calls TEXT, tool_name TEXT,
            timestamp REAL NOT NULL, finish_reason TEXT, reasoning TEXT, reasoning_content TEXT
         );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sessions (id, source, model, title, started_at) VALUES (?1, 'tui', 'gpt-5.4', 'Test', 1778279440.0)",
        ["20260508_183040_abc123"],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO messages (session_id, role, content, timestamp) VALUES (?1, 'user', 'hi', 1778279441.0)",
        ["20260508_183040_abc123"],
    )
    .unwrap();

    let virtual_path = db.join("20260508_183040_abc123");
    let parsed = HermesParser.parse_export(&virtual_path).unwrap();
    assert_eq!(parsed.session.metadata.session_id, "20260508_183040_abc123");
    assert_eq!(parsed.session.stats.user_messages, 1);
    assert_eq!(
        parsed.training.source.source_format.as_deref(),
        Some("hermes.sqlite")
    );

    fs::remove_dir_all(dir).ok();
}
