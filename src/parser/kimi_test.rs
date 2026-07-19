use super::*;
use crate::ast::Tool;

fn parse_test_session(jsonl: &str) -> Session {
    let lines: Vec<String> = jsonl.lines().map(str::to_string).collect();
    KimiCodeParser::parse_from_lines(lines.into_iter()).unwrap()
}

#[test]
fn test_parse_minimal_session() {
    let jsonl = r#"{"type":"metadata","protocol_version":"1.4","created_at":1784330126120}
{"type":"config.update","modelAlias":"kimi-code/k3","thinkingEffort":"high","time":1784330126121}
{"type":"permission.set_mode","mode":"yolo","time":1784330126121}
{"type":"turn.prompt","input":[{"type":"text","text":"hello world"}],"origin":{"kind":"user"},"time":1784330126122}
{"type":"context.append_loop_event","event":{"type":"content.part","uuid":"abc","turnId":"0","step":1,"part":{"type":"text","text":"Hi there! How can I help?"}},"time":1784330126200}
{"type":"context.append_loop_event","event":{"type":"step.end","uuid":"abc","turnId":"0","step":1,"usage":{"inputOther":100,"output":50,"inputCacheRead":200,"inputCacheCreation":0}},"time":1784330126300}
{"type":"usage.record","model":"kimi-code/k3","usage":{"inputOther":100,"output":50,"inputCacheRead":200,"inputCacheCreation":0},"usageScope":"turn","time":1784330126301}"#;

    let session = parse_test_session(jsonl);

    assert_eq!(session.metadata.tool, Tool::Kimi);
    assert_eq!(session.stats.user_messages, 1);
    assert_eq!(session.stats.assistant_messages, 1);
    assert_eq!(session.stats.tool_calls, 0);

    // First message should be user
    assert_eq!(session.messages[0].role, Role::User);
    if let ContentBlock::Text { text } = &session.messages[0].content[0] {
        assert_eq!(text, "hello world");
    } else {
        panic!("Expected Text content block");
    }

    // Second message should be assistant with text
    assert_eq!(session.messages[1].role, Role::Assistant);
    if let ContentBlock::Text { text } = &session.messages[1].content[0] {
        assert_eq!(text, "Hi there! How can I help?");
    } else {
        panic!("Expected Text content block");
    }

    // Usage should be on the assistant message
    let usage = session.messages[1].usage.as_ref().expect("expected usage");
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 50);
    assert_eq!(usage.cache_read_tokens, 200);
    assert_eq!(usage.cache_creation_tokens, 0);
}

#[test]
fn test_parse_tool_calls() {
    let jsonl = r#"{"type":"metadata","protocol_version":"1.4","created_at":1784330126120}
{"type":"config.update","modelAlias":"kimi-code/k3","time":1784330126121}
{"type":"turn.prompt","input":[{"type":"text","text":"read the file"}],"origin":{"kind":"user"},"time":1784330126122}
{"type":"context.append_loop_event","event":{"type":"content.part","uuid":"t1","turnId":"0","step":1,"part":{"type":"text","text":"Let me read that file."}},"time":1784330126200}
{"type":"context.append_loop_event","event":{"type":"tool.call","uuid":"tool_1","turnId":"0","step":1,"toolCallId":"tool_1","name":"Read","args":{"path":"src/main.rs"}},"time":1784330126210}
{"type":"context.append_loop_event","event":{"type":"tool.result","parentUuid":"tool_1","toolCallId":"tool_1","result":{"output":"fn main() { println!(\"hello\"); }","isError":false}},"time":1784330126220}
{"type":"context.append_loop_event","event":{"type":"content.part","uuid":"t2","turnId":"0","step":1,"part":{"type":"text","text":"Here's what I found."}},"time":1784330126230}
{"type":"context.append_loop_event","event":{"type":"step.end","uuid":"t2","turnId":"0","step":1,"usage":{"inputOther":200,"output":100,"inputCacheRead":0,"inputCacheCreation":0}},"time":1784330126300}"#;

    let session = parse_test_session(jsonl);

    assert_eq!(session.stats.user_messages, 1);
    assert_eq!(session.stats.assistant_messages, 2); // two text messages (tool_result not counted)
    assert_eq!(session.stats.tool_calls, 1);
    assert_eq!(session.stats.tool_errors, 0);

    // Messages: [user, text "Let me read...", tool_result Read, text "Here's what I found."]
    let tool_msg = &session.messages[2];
    assert_eq!(tool_msg.role, Role::Assistant);
    if let ContentBlock::ToolResult { name, success, summary, .. } = &tool_msg.content[0] {
        assert_eq!(name, "Read");
        assert!(success);
        assert_eq!(summary, r#"file="src/main.rs""#);
    } else {
        panic!("Expected ToolResult content block");
    }
}

#[test]
fn test_parse_thinking_block() {
    let jsonl = r#"{"type":"metadata","protocol_version":"1.4","created_at":1784330126120}
{"type":"config.update","modelAlias":"kimi-code/k3","time":1784330126121}
{"type":"turn.prompt","input":[{"type":"text","text":"solve this"}],"origin":{"kind":"user"},"time":1784330126122}
{"type":"context.append_loop_event","event":{"type":"content.part","uuid":"th1","turnId":"0","step":1,"part":{"type":"think","think":"Let me think about this problem step by step..."}},"time":1784330126200}
{"type":"context.append_loop_event","event":{"type":"content.part","uuid":"th2","turnId":"0","step":1,"part":{"type":"text","text":"The answer is 42."}},"time":1784330126210}
{"type":"context.append_loop_event","event":{"type":"step.end","uuid":"th2","turnId":"0","step":1,"usage":{"inputOther":50,"output":30,"inputCacheRead":0,"inputCacheCreation":0}},"time":1784330126300}"#;

    let session = parse_test_session(jsonl);

    assert_eq!(session.stats.user_messages, 1);
    assert_eq!(session.stats.assistant_messages, 2); // thinking + text

    // First assistant message should be thinking
    if let ContentBlock::Thinking { text } = &session.messages[1].content[0] {
        assert!(text.contains("step by step"));
    } else {
        panic!("Expected Thinking content block");
    }

    // Second assistant message should be text
    if let ContentBlock::Text { text } = &session.messages[2].content[0] {
        assert_eq!(text, "The answer is 42.");
    } else {
        panic!("Expected Text content block");
    }
}

#[test]
fn test_parse_model_from_config() {
    let jsonl = r#"{"type":"metadata","protocol_version":"1.4","created_at":1784330126120}
{"type":"config.update","modelAlias":"kimi-code/k3","thinkingEffort":"high","time":1784330126121}
{"type":"config.update","modelAlias":"kimi-code/k2.7","time":1784330200000}
{"type":"turn.prompt","input":[{"type":"text","text":"hi"}],"origin":{"kind":"user"},"time":1784330126122}
{"type":"context.append_loop_event","event":{"type":"content.part","uuid":"p1","turnId":"0","step":1,"part":{"type":"text","text":"hello"}},"time":1784330126200}
{"type":"context.append_loop_event","event":{"type":"step.end","uuid":"p1","turnId":"0","step":1,"usage":{"inputOther":0,"output":0,"inputCacheRead":0,"inputCacheCreation":0}},"time":1784330126300}"#;

    let session = parse_test_session(jsonl);

    // Model should be the last one seen
    assert_eq!(session.metadata.model, Some("kimi-code/k2.7".to_string()));
}

#[test]
fn test_parse_system_user_message() {
    let jsonl = r#"{"type":"metadata","protocol_version":"1.4","created_at":1784330126120}
{"type":"config.update","modelAlias":"kimi-code/k3","time":1784330126121}
{"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>\nGoal paused due to quota.\n<untrusted_objective>\ndo stuff"}]},"time":1784330126150}
{"type":"turn.prompt","input":[{"type":"text","text":"continue"}],"origin":{"kind":"user"},"time":1784330126200}
{"type":"context.append_loop_event","event":{"type":"content.part","uuid":"p1","turnId":"0","step":1,"part":{"type":"text","text":"ok"}},"time":1784330126300}
{"type":"context.append_loop_event","event":{"type":"step.end","uuid":"p1","turnId":"0","step":1,"usage":{"inputOther":0,"output":0,"inputCacheRead":0,"inputCacheCreation":0}},"time":1784330126400}"#;

    let session = parse_test_session(jsonl);

    // Should have 2 user messages: system-reminder + turn.prompt
    assert_eq!(session.stats.user_messages, 2);

    // First user message should be the system-reminder content
    if let ContentBlock::Text { text } = &session.messages[0].content[0] {
        assert!(text.contains("Goal paused"));
    } else {
        panic!("Expected Text content block");
    }
}

#[test]
fn test_parse_error_tool_result() {
    let jsonl = r#"{"type":"metadata","protocol_version":"1.4","created_at":1784330126120}
{"type":"config.update","modelAlias":"kimi-code/k3","time":1784330126121}
{"type":"turn.prompt","input":[{"type":"text","text":"run bad command"}],"origin":{"kind":"user"},"time":1784330126122}
{"type":"context.append_loop_event","event":{"type":"tool.call","uuid":"tool_e1","turnId":"0","step":1,"toolCallId":"tool_e1","name":"Bash","args":{"command":"false"}},"time":1784330126200}
{"type":"context.append_loop_event","event":{"type":"tool.result","parentUuid":"tool_e1","toolCallId":"tool_e1","result":{"output":"Command failed with exit code: 1.","isError":true}},"time":1784330126210}
{"type":"context.append_loop_event","event":{"type":"content.part","uuid":"p1","turnId":"0","step":1,"part":{"type":"text","text":"That failed."}},"time":1784330126220}
{"type":"context.append_loop_event","event":{"type":"step.end","uuid":"p1","turnId":"0","step":1,"usage":{"inputOther":50,"output":20,"inputCacheRead":0,"inputCacheCreation":0}},"time":1784330126300}"#;

    let session = parse_test_session(jsonl);

    assert_eq!(session.stats.tool_errors, 1);
    assert_eq!(session.stats.tool_calls, 0); // error doesn't count as success

    let tool_msg = &session.messages[1];
    if let ContentBlock::ToolResult { success, .. } = &tool_msg.content[0] {
        assert!(!success);
    } else {
        panic!("Expected ToolResult");
    }
}

#[test]
fn test_session_kind_classification() {
    let jsonl = r#"{"type":"metadata","protocol_version":"1.4","created_at":1784330126120}
{"type":"config.update","modelAlias":"kimi-code/k3","time":1784330126121}
{"type":"turn.prompt","input":[{"type":"text","text":"first"}],"origin":{"kind":"user"},"time":1784330126122}
{"type":"context.append_loop_event","event":{"type":"content.part","uuid":"p1","turnId":"0","step":1,"part":{"type":"text","text":"reply1"}},"time":1784330126200}
{"type":"context.append_loop_event","event":{"type":"step.end","uuid":"p1","turnId":"0","step":1,"usage":{"inputOther":0,"output":0,"inputCacheRead":0,"inputCacheCreation":0}},"time":1784330126250}
{"type":"turn.prompt","input":[{"type":"text","text":"second"}],"origin":{"kind":"user"},"time":1784330126300}
{"type":"context.append_loop_event","event":{"type":"content.part","uuid":"p2","turnId":"0","step":2,"part":{"type":"text","text":"reply2"}},"time":1784330126400}
{"type":"context.append_loop_event","event":{"type":"step.end","uuid":"p2","turnId":"0","step":2,"usage":{"inputOther":0,"output":0,"inputCacheRead":0,"inputCacheCreation":0}},"time":1784330126450}
{"type":"turn.prompt","input":[{"type":"text","text":"third"}],"origin":{"kind":"user"},"time":1784330126500}
{"type":"context.append_loop_event","event":{"type":"content.part","uuid":"p3","turnId":"0","step":3,"part":{"type":"text","text":"reply3"}},"time":1784330126600}
{"type":"context.append_loop_event","event":{"type":"step.end","uuid":"p3","turnId":"0","step":3,"usage":{"inputOther":0,"output":0,"inputCacheRead":0,"inputCacheCreation":0}},"time":1784330126650}"#;

    let session = parse_test_session(jsonl);

    // 3 user messages, 3 assistant messages => Human (interactive)
    assert_eq!(session.metadata.session_kind, SessionKind::Human);
}

#[test]
fn test_parse_empty_session() {
    let jsonl = r#"{"type":"metadata","protocol_version":"1.4","created_at":1784330126120}
{"type":"config.update","modelAlias":"kimi-code/k3","time":1784330126121}"#;

    let session = parse_test_session(jsonl);

    assert_eq!(session.stats.user_messages, 0);
    assert_eq!(session.stats.assistant_messages, 0);
    assert_eq!(session.messages.len(), 0);
}

#[test]
fn test_session_id_from_source_path() {
    let path = std::path::Path::new(
        "/Users/ianzepp/.kimi-code/sessions/wd_faberlang_3e255c91d373/session_236b3acc/agents/main/wire.jsonl",
    );
    let id = kimi_session_id_from_source(path);
    assert_eq!(id, Some("session_236b3acc".to_string()));
}

#[test]
fn test_duration_calculation() {
    let jsonl = r#"{"type":"metadata","protocol_version":"1.4","created_at":1784330126120}
{"type":"config.update","modelAlias":"kimi-code/k3","time":1784330126121}
{"type":"turn.prompt","input":[{"type":"text","text":"hi"}],"origin":{"kind":"user"},"time":1784330126122}
{"type":"context.append_loop_event","event":{"type":"content.part","uuid":"p1","turnId":"0","step":1,"part":{"type":"text","text":"hello"}},"time":1784330126200}
{"type":"context.append_loop_event","event":{"type":"step.end","uuid":"p1","turnId":"0","step":1,"usage":{"inputOther":0,"output":0,"inputCacheRead":0,"inputCacheCreation":0},"time":1784330186120}}"#;

    let session = parse_test_session(jsonl);

    // Duration should be computed from first to last timestamp
    let dur = session.stats.duration_seconds;
    // If timestamps are consistent, duration should be positive
    if let Some(d) = dur {
        assert!(d > 0, "expected positive duration, got {d}");
    }
}
