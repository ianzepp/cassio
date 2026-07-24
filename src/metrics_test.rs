use super::*;
use std::io::Write;

#[test]
fn collect_day_metrics_from_fixture() {
    let dir = std::env::temp_dir().join(format!("cassio-metrics-{}", std::process::id()));
    let month = dir.join("2026-06");
    std::fs::create_dir_all(&month).unwrap();
    let path = month.join("2026-06-11T10-00-00-codex.md");
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(
        f,
        "📋 Session: abc\n📋 Project: /tmp/proj\n📋 Model: gpt-5.5\n📋 Messages: 4 user, 5 assistant\n📋 Tool calls: 3 total, 1 failed\n📋 Tokens: 1.0K in, 2.0K out\n📋 Cost: $0.05\n👤 hello\n"
    )
    .unwrap();

    let m = collect_day_metrics(&dir, "2026-06-11").unwrap();
    assert_eq!(m.sessions, 1);
    assert_eq!(m.user_msgs, 4);
    assert_eq!(m.by_tool.get("codex").map(|b| b.sessions), Some(1));
    assert!((m.cost_usd - 0.05).abs() < 1e-9);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn iso_week_roundtrip() {
    let d = chrono::NaiveDate::from_ymd_opt(2026, 6, 11).unwrap();
    let id = iso_week_id(d);
    assert!(id.starts_with("2026-W"));
    let days = iso_week_days(&id).unwrap();
    assert_eq!(days.len(), 7);
    assert!(days.iter().any(|x| x == "2026-06-11"));
}
