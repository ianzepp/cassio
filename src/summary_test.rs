use super::*;

// --- parse_duration tests ---

#[test]
fn test_parse_duration_hours_and_minutes() {
    assert_eq!(parse_duration("1h 4m"), 3840);
}

#[test]
fn test_parse_duration_minutes_only() {
    assert_eq!(parse_duration("12m"), 720);
}

#[test]
fn test_parse_duration_seconds_only() {
    assert_eq!(parse_duration("45s"), 45);
}

#[test]
fn test_parse_duration_combined() {
    assert_eq!(parse_duration("2h 30m"), 9000);
}

#[test]
fn test_parse_duration_empty() {
    assert_eq!(parse_duration(""), 0);
}

// --- parse_token_value tests ---

#[test]
fn test_parse_token_value_thousands() {
    assert_eq!(parse_token_value("1.5K"), 1500);
}

#[test]
fn test_parse_token_value_millions() {
    assert_eq!(parse_token_value("2.3M"), 2300000);
}

#[test]
fn test_parse_token_value_raw() {
    assert_eq!(parse_token_value("42"), 42);
}

#[test]
fn test_parse_token_value_zero() {
    assert_eq!(parse_token_value("0"), 0);
}

// --- shorten_project tests ---

#[test]
fn test_shorten_project_short() {
    assert_eq!(shorten_project("foo/bar"), "foo/bar");
}

#[test]
fn test_shorten_project_long() {
    // "/home/user/projects/myapp" splits to ["", "home", "user", "projects", "myapp"] -> last 3
    assert_eq!(
        shorten_project("/home/user/projects/myapp"),
        "user/projects/myapp"
    );
}

#[test]
fn test_shorten_project_exactly_three() {
    assert_eq!(shorten_project("a/b/c"), "a/b/c");
}

#[test]
fn test_shorten_project_trailing_slash() {
    // trailing slash stripped, then last 3 of ["", "home", "user", "projects", "myapp"]
    assert_eq!(
        shorten_project("/home/user/projects/myapp/"),
        "user/projects/myapp"
    );
}

// --- strip_emoji_prefix tests ---

#[test]
fn test_strip_emoji_prefix_match() {
    let result = strip_emoji_prefix("📋 Session: abc", "📋");
    assert_eq!(result, Some(" Session: abc"));
}

#[test]
fn test_strip_emoji_prefix_no_match() {
    let result = strip_emoji_prefix("👤 Hello", "📋");
    assert!(result.is_none());
}

// --- format_duration tests ---

#[test]
fn test_format_duration_zero_or_negative() {
    assert_eq!(format_duration(0), "-");
    assert_eq!(format_duration(-5), "-");
}

#[test]
fn test_format_duration_minutes() {
    assert_eq!(format_duration(300), "5m");
}

#[test]
fn test_format_duration_hours() {
    assert_eq!(format_duration(3660), "1h 1m");
}

// --- format_tokens tests ---

#[test]
fn test_format_tokens_small() {
    assert_eq!(format_tokens(500), "500");
}

#[test]
fn test_format_tokens_k() {
    assert_eq!(format_tokens(1500), "1.5K");
}

#[test]
fn test_format_tokens_m() {
    assert_eq!(format_tokens(1_500_000), "1.5M");
}

// --- TranscriptKind tests ---

#[test]
fn test_transcript_kind_classification_is_inclusive_at_two_user_messages() {
    assert_eq!(TranscriptKind::classify(2, 3), TranscriptKind::Agentic);
    assert_eq!(TranscriptKind::classify(2, 2), TranscriptKind::Abandoned);
    assert_eq!(TranscriptKind::classify(1, 1), TranscriptKind::Abandoned);
    assert_eq!(TranscriptKind::classify(0, 3), TranscriptKind::Agentic);
}

#[test]
fn test_transcript_kind_classification_interactive_after_two_user_messages() {
    assert_eq!(TranscriptKind::classify(3, 0), TranscriptKind::Interactive);
    assert_eq!(TranscriptKind::classify(3, 10), TranscriptKind::Interactive);
}

// --- Aggregate tests ---

#[test]
fn test_aggregate_add() {
    let mut agg = Aggregate::default();
    let stats = TranscriptStats {
        tool_name: "claude".to_string(),
        date: "2025-01-15".to_string(),
        project: "/proj".to_string(),
        model: Some("opus-4.5".to_string()),
        kind: TranscriptKind::Interactive,
        user_msgs: 5,
        asst_msgs: 10,
        tool_ok: 3,
        tool_fail: 1,
        input_tokens: 1000,
        output_tokens: 500,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        duration_secs: 60,
    };
    agg.add(&stats);
    assert_eq!(agg.sessions, 1);
    assert_eq!(agg.kind_breakdown(), "1/0/0");
    assert_eq!(agg.user_msgs, 5);
    assert_eq!(agg.input_tokens, 1000);
    // 1000 input @ $5/MTok + 500 output @ $25/MTok = $0.005 + $0.0125
    assert!((agg.cost - 0.0175).abs() < 0.0001);
}

#[test]
fn test_aggregate_kind_breakdown() {
    let mut agg = Aggregate::default();
    for kind in [
        TranscriptKind::Interactive,
        TranscriptKind::Agentic,
        TranscriptKind::Agentic,
        TranscriptKind::Abandoned,
    ] {
        agg.add(&TranscriptStats {
            kind,
            ..Default::default()
        });
    }

    assert_eq!(agg.sessions, 4);
    assert_eq!(agg.kind_breakdown(), "1/2/1");
}

#[test]
fn test_aggregate_add_agg() {
    let mut a = Aggregate {
        sessions: 1,
        interactive_sessions: 1,
        user_msgs: 5,
        ..Default::default()
    };
    let b = Aggregate {
        sessions: 2,
        agentic_sessions: 1,
        abandoned_sessions: 1,
        user_msgs: 10,
        ..Default::default()
    };
    a.add_agg(&b);
    assert_eq!(a.sessions, 3);
    assert_eq!(a.kind_breakdown(), "1/1/1");
    assert_eq!(a.user_msgs, 15);
}
