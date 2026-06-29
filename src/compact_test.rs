use super::*;

#[test]
fn test_format_elapsed_seconds() {
    let d = std::time::Duration::from_secs(45);
    assert_eq!(format_elapsed(d), "45s");
}

#[test]
fn test_format_elapsed_minutes() {
    let d = std::time::Duration::from_secs(125);
    assert_eq!(format_elapsed(d), "2m05s");
}

#[test]
fn test_build_monthly_input_structure() {
    let items = vec![
        ("2025-01-15".to_string(), "day 15 content".to_string()),
        ("2025-01-16".to_string(), "day 16 content".to_string()),
    ];
    let result = build_monthly_input("PROMPT", "2025-01", &items);
    assert!(result.starts_with("PROMPT"));
    assert!(result.contains("2025-01"));
    assert!(result.contains("2 days"));
    assert!(result.contains("day 15 content"));
    assert!(result.contains("day 16 content"));
    assert!(result.contains("---BEGIN MONTHLY COMPACTIONS"));
    assert!(result.contains("---END MONTHLY COMPACTIONS---"));
}

#[test]
fn test_build_daily_input_structure() {
    let sessions = vec![
        ("s1.md".to_string(), "session one".to_string()),
        ("s2.md".to_string(), "session two".to_string()),
    ];
    let result = build_daily_input("PROMPT", "2025-01-15", &sessions);
    assert!(result.starts_with("PROMPT"));
    assert!(result.contains("2025-01-15"));
    assert!(result.contains("2 sessions"));
    assert!(result.contains("session one"));
    assert!(result.contains("session two"));
    assert!(result.contains("---BEGIN TRANSCRIPTS"));
    assert!(result.contains("---END TRANSCRIPTS---"));
}

#[test]
fn test_plan_daily_compaction_single() {
    let sessions = vec![("a.md".to_string(), "small content".to_string())];
    let plan = plan_daily_compaction("2025-01-15", &sessions, DEFAULT_MAX_INPUT_BYTES);
    match plan {
        DailyCompactionPlan::Single(input) => {
            assert!(input.contains("2025-01-15"));
            assert!(input.contains("small content"));
        }
        DailyCompactionPlan::Chunked(_) => panic!("expected single-pass plan"),
    }
}

#[test]
fn test_plan_daily_compaction_chunked() {
    let big = "x".repeat(100_000);
    let sessions = vec![("a.md".to_string(), big.clone()), ("b.md".to_string(), big)];
    let plan = plan_daily_compaction("2025-01-15", &sessions, DEFAULT_MAX_INPUT_BYTES);
    match plan {
        DailyCompactionPlan::Chunked(chunk_inputs) => {
            assert!(chunk_inputs.len() >= 2);
            assert!(chunk_inputs[0].contains("2025-01-15"));
        }
        DailyCompactionPlan::Single(_) => panic!("expected chunked plan"),
    }
}

#[test]
fn test_build_daily_merge_input_structure() {
    let chunks = vec![
        ("chunk-1".to_string(), "partial one".to_string()),
        ("chunk-2".to_string(), "partial two".to_string()),
    ];
    let result = build_daily_merge_input("2025-01-15", &chunks);
    assert!(result.starts_with(DAILY_MERGE_PROMPT));
    assert!(result.contains("2025-01-15"));
    assert!(result.contains("2 chunks"));
    assert!(result.contains("partial one"));
    assert!(result.contains("partial two"));
    assert!(result.contains("---BEGIN DAILY PARTIALS"));
    assert!(result.contains("---END DAILY PARTIALS (2025-01-15)---"));
}

#[test]
fn test_build_chunks_single_chunk() {
    let contents = vec![("a".to_string(), "small content".to_string())];
    let chunks = build_chunks(&contents, 100, DEFAULT_MAX_INPUT_BYTES);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].len(), 1);
}

#[test]
fn test_build_chunks_multiple() {
    // Create content that exceeds the default budget when combined
    let big = "x".repeat(100_000);
    let contents = vec![
        ("a".to_string(), big.clone()),
        ("b".to_string(), big.clone()),
    ];
    let chunks = build_chunks(&contents, 100, DEFAULT_MAX_INPUT_BYTES);
    assert!(chunks.len() >= 2);
}

#[test]
fn test_build_chunks_oversized_single_file() {
    // Single file that exceeds budget still gets its own chunk
    let huge = "x".repeat(200_000);
    let contents = vec![
        ("small".to_string(), "tiny".to_string()),
        ("big".to_string(), huge),
    ];
    let chunks = build_chunks(&contents, 100, DEFAULT_MAX_INPUT_BYTES);
    assert!(chunks.len() >= 2);
}

#[test]
fn test_extract_session_filters_correctly() {
    let content = "\
📋 Session: abc
📋 Project: /proj
👤 user prompt here
🤖 assistant response line 1
second line of response
third line
fourth line
fifth line
sixth line
seventh line that should be cut
✅ Read: file=\"test.rs\"
👤 another question
";
    // Write to temp file and test extract_session
    let dir = std::env::temp_dir().join("cassio_test_extract");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("test.md");
    std::fs::write(&path, content).unwrap();

    let result = extract_session(&path).unwrap();

    // Should include metadata, user, and assistant lines
    assert!(result.contains("📋 Session: abc"));
    assert!(result.contains("👤 user prompt here"));
    assert!(result.contains("🤖 assistant response line 1"));
    // Lines after LLM_LINE_LIMIT (5) should be cut
    assert!(result.contains("sixth line"));
    assert!(!result.contains("seventh line"));
    // Tool result lines (✅/❌) should be excluded
    assert!(!result.contains("✅ Read"));
    // Second user prompt should be included
    assert!(result.contains("👤 another question"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn test_daily_chunk_checkpoint_path_format() {
    let dir = Path::new("/tmp/checkpoints");
    let path = daily_chunk_checkpoint_path(dir, 0);
    assert_eq!(path, Path::new("/tmp/checkpoints/chunk-0001.md"));
}

#[test]
fn test_read_cached_chunk_summary_missing_and_empty() {
    let dir = std::env::temp_dir().join(format!("cassio_test_chunk_cache_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let missing = dir.join("missing.md");
    assert!(read_cached_chunk_summary(&missing).unwrap().is_none());

    let empty = dir.join("empty.md");
    std::fs::write(&empty, "   \n").unwrap();
    assert!(read_cached_chunk_summary(&empty).unwrap().is_none());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn test_read_cached_chunk_summary_returns_content() {
    let dir = std::env::temp_dir().join(format!(
        "cassio_test_chunk_cache_hit_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let path = dir.join("chunk-0001.md");
    std::fs::write(&path, "saved partial").unwrap();

    assert_eq!(
        read_cached_chunk_summary(&path).unwrap().as_deref(),
        Some("saved partial")
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn test_chat_completions_endpoint_normalizes_base_url() {
    assert_eq!(
        chat_completions_endpoint("http://127.0.0.1:18173/v1"),
        "http://127.0.0.1:18173/v1/chat/completions"
    );
    assert_eq!(
        chat_completions_endpoint("http://127.0.0.1:18173/v1/"),
        "http://127.0.0.1:18173/v1/chat/completions"
    );
    assert_eq!(
        chat_completions_endpoint("http://127.0.0.1:18173/v1/chat/completions"),
        "http://127.0.0.1:18173/v1/chat/completions"
    );
}

#[test]
fn test_openai_provider_requires_base_url() {
    let err = invoke_llm_once(
        "hello",
        "local",
        "openai",
        None,
        std::time::Duration::from_secs(1),
    )
    .unwrap_err();
    assert_eq!(err.class, FailureClass::Io);
    assert!(err.detail.contains("requires base_url"));
}
