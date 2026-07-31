use super::*;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn line(value: serde_json::Value) -> String {
    value.to_string()
}

fn temp_dir(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("cassio-grok-{name}-{unique}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn test_parse_minimal_grok_session() {
    let lines = vec![
        line(json!({
            "type": "user",
            "content": [{"type": "text", "text": "<user_query>\nwarm up please\n</user_query>"}]
        })),
        line(json!({
            "type": "assistant",
            "content": "On it.",
            "model_id": "grok-composer-2.5-fast"
        })),
    ];

    let session = GrokParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.metadata.tool, Tool::Grok);
    assert_eq!(session.metadata.session_id, "stdin");
    assert_eq!(session.stats.user_messages, 1);
    assert_eq!(session.stats.assistant_messages, 1);
    assert_eq!(
        session.metadata.model.as_deref(),
        Some("grok-composer-2.5-fast")
    );
}

#[test]
fn test_parse_grok_tool_call_and_result() {
    let lines = vec![
        line(json!({
            "type": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "call-1",
                "name": "Read",
                "arguments": "{\"path\":\"src/lib.rs\"}"
            }],
            "model_id": "grok-composer-2.5-fast"
        })),
        line(json!({
            "type": "tool_result",
            "tool_call_id": "call-1",
            "content": "fn main() {}\n"
        })),
    ];

    let session = GrokParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.stats.tool_calls, 1);
    assert_eq!(session.stats.tool_errors, 0);
    assert!(session.stats.files_read.contains("src/lib.rs"));
    assert!(session.messages.iter().any(|msg| {
        msg.content.iter().any(|block| {
            matches!(
                block,
                ContentBlock::ToolResult {
                    name,
                    success,
                    summary,
                    ..
                } if name == "Read" && *success && summary == "file=\"src/lib.rs\""
            )
        })
    }));
}

#[test]
fn test_grok_tool_result_detects_shell_failure() {
    assert!(!grok_tool_result_failed("Exit code: 0\n"));
    assert!(grok_tool_result_failed("Exit code: 1\n"));
}

#[test]
fn test_extract_grok_user_text_skips_context_injection() {
    let record = json!({
        "type": "user",
        "content": [{"type": "text", "text": "<user_info>\nOS: darwin\n</user_info>"}]
    });
    assert!(extract_grok_user_text(&record).is_none());
}

#[test]
fn test_grok_token_usage_from_updates() {
    let chat = [
        line(json!({
            "type": "user",
            "content": [{"type": "text", "text": "hello"}]
        })),
        line(json!({
            "type": "assistant",
            "content": "hi",
            "model_id": "deepseek-v4-flash"
        })),
    ]
    .join("\n");
    // First turn: inputTokens includes cachedReadTokens (1000 + 200 == 1200
    // total), so fresh input is 400. Second turn: no cached reads, input stays
    // 500. A tool_call record without usage must be ignored.
    let updates = concat!(
        r#"{"params":{"sessionId":"s","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":1000,"outputTokens":200,"cachedReadTokens":600,"cacheCreationTokens":50,"totalTokens":1200}}}}"#,
        "\n",
        r#"{"params":{"sessionId":"s","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":500,"outputTokens":100,"cachedReadTokens":0,"cacheCreationTokens":10,"totalTokens":600}}}}"#,
        "\n",
        r#"{"params":{"sessionId":"s","update":{"sessionUpdate":"tool_call","title":"Read"}}}"#,
        "\n",
    );
    let dir = temp_dir("usage");
    fs::write(dir.join("chat_history.jsonl"), chat).unwrap();
    fs::write(dir.join("updates.jsonl"), updates).unwrap();

    let parsed = GrokParser
        .parse_export(&dir.join("chat_history.jsonl"))
        .unwrap();
    assert_eq!(parsed.session.stats.total_tokens.input_tokens, 900);
    assert_eq!(parsed.session.stats.total_tokens.output_tokens, 300);
    assert_eq!(parsed.session.stats.total_tokens.cache_read_tokens, 600);
    assert_eq!(parsed.session.stats.total_tokens.cache_creation_tokens, 60);
}

#[test]
fn test_grok_token_usage_disjoint_schema() {
    let chat = [line(json!({
        "type": "user",
        "content": [{"type": "text", "text": "hello"}]
    }))]
    .join("\n");
    // totalTokens == input + output + cachedRead: cached reads are NOT inside
    // inputTokens, so input must be kept as reported (no subtraction).
    let updates = concat!(
        r#"{"params":{"sessionId":"s","update":{"sessionUpdate":"turn_completed","usage":{"inputTokens":1000,"outputTokens":100,"cachedReadTokens":300,"cacheCreationTokens":0,"totalTokens":1400}}}}"#,
        "\n",
    );
    let dir = temp_dir("disjoint");
    fs::write(dir.join("chat_history.jsonl"), chat).unwrap();
    fs::write(dir.join("updates.jsonl"), updates).unwrap();

    let parsed = GrokParser
        .parse_export(&dir.join("chat_history.jsonl"))
        .unwrap();
    assert_eq!(parsed.session.stats.total_tokens.input_tokens, 1000);
    assert_eq!(parsed.session.stats.total_tokens.cache_read_tokens, 300);
}

#[test]
fn test_grok_context_from_signals() {
    let chat = line(json!({
        "type": "user",
        "content": [{"type": "text", "text": "hello"}]
    }));
    let dir = temp_dir("signals");
    fs::write(dir.join("chat_history.jsonl"), chat).unwrap();
    fs::write(
        dir.join("signals.json"),
        r#"{"contextTokensUsed":43343,"contextWindowTokens":1048576,"contextWindowUsage":4}"#,
    )
    .unwrap();

    let parsed = GrokParser
        .parse_export(&dir.join("chat_history.jsonl"))
        .unwrap();
    assert_eq!(parsed.session.stats.context_tokens_used, Some(43343));
    assert_eq!(parsed.session.stats.context_window_tokens, Some(1048576));
}

#[test]
fn test_grok_missing_usage_siblings() {
    let chat = [
        line(json!({
            "type": "user",
            "content": [{"type": "text", "text": "hello"}]
        })),
        line(json!({
            "type": "assistant",
            "content": "hi",
            "model_id": "deepseek-v4-flash"
        })),
    ]
    .join("\n");
    let dir = temp_dir("nosiblings");
    fs::write(dir.join("chat_history.jsonl"), chat).unwrap();

    let parsed = GrokParser
        .parse_export(&dir.join("chat_history.jsonl"))
        .unwrap();
    assert_eq!(parsed.session.stats.total_tokens.input_tokens, 0);
    assert_eq!(parsed.session.stats.total_tokens.output_tokens, 0);
    assert_eq!(parsed.session.stats.total_tokens.cache_read_tokens, 0);
    assert_eq!(parsed.session.stats.total_tokens.cache_creation_tokens, 0);
    assert_eq!(parsed.session.stats.context_tokens_used, None);
    assert_eq!(parsed.session.stats.context_window_tokens, None);
}
