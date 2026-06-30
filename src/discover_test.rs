use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

fn temp_dir(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("cassio-{name}-{unique}"))
}

#[test]
fn test_derive_codex_output_path_valid() {
    let path = PathBuf::from("/sessions/rollout-2025-11-11T14-12-49-019a7455-abcd.jsonl");
    let (folder, filename) = derive_codex_output_path(&path);
    assert_eq!(folder, "2025-11");
    assert_eq!(filename, "2025-11-11T14-12-49-codex.md");
}

#[test]
fn test_derive_codex_output_path_short_filename() {
    let path = PathBuf::from("/sessions/rollout-short.jsonl");
    let (folder, filename) = derive_codex_output_path(&path);
    assert_eq!(folder, "unknown");
    assert_eq!(filename, "unknown-codex.md");
}

#[test]
fn test_derive_codex_output_path_no_prefix() {
    let path = PathBuf::from("/sessions/something.jsonl");
    let (folder, filename) = derive_codex_output_path(&path);
    assert_eq!(folder, "unknown");
    assert_eq!(filename, "unknown-codex.md");
}

#[test]
fn test_derive_output_path_opencode_placeholder() {
    let path = PathBuf::from("/storage/message/ses_123");
    let (folder, filename) = derive_output_path(Tool::OpenCode, &path);
    assert_eq!(folder, "unknown");
    assert!(filename.contains("opencode"));
}

#[test]
fn test_derive_pi_output_path_valid() {
    let path = PathBuf::from(
        "/sessions/2026-04-13T09-45-42-886Z_0c85082c-220c-4e56-8ae5-9463d6228494.jsonl",
    );
    let (folder, filename) = derive_pi_output_path(&path);
    assert_eq!(folder, "2026-04");
    assert_eq!(filename, "2026-04-13T09-45-42-pi.md");
}

#[test]
fn test_find_pi_files_collects_jsonl() {
    let dir = temp_dir("discover-pi");
    fs::create_dir_all(dir.join("nested")).unwrap();
    fs::write(dir.join("nested").join("session.jsonl"), "{}\n").unwrap();
    fs::write(dir.join("nested").join("ignore.txt"), "x").unwrap();

    let mut results = Vec::new();
    find_pi_files(&dir, &mut results);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, Tool::Pi);
    assert!(results[0].1.ends_with("session.jsonl"));

    fs::remove_dir_all(dir).ok();
}

#[test]
fn test_find_hermes_sessions_recurses_and_keeps_suffix() {
    let dir = temp_dir("discover-hermes");
    let root = dir.join("worker").join("var").join("lib").join("hermes");
    fs::create_dir_all(root.join("sessions")).unwrap();
    fs::write(
        root.join("sessions")
            .join("session_20260509_122904_e7da7d.json"),
        r#"{"session_id":"20260509_122904_e7da7d","session_start":"2026-05-09T12:29:04","messages":[]}"#,
    )
    .unwrap();

    let mut results = Vec::new();
    find_hermes_sessions(&dir, &mut results);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, Tool::Hermes);

    let (folder, filename) = derive_output_path(Tool::Hermes, &results[0].1);
    assert_eq!(folder, "2026-05");
    assert_eq!(filename, "2026-05-09T12-29-04-e7da7d-hermes.md");

    let (folder, filename) = derive_hermes_output_path_from_id("20260509_122904_e7da7d");
    assert_eq!(folder, "2026-05");
    assert_eq!(filename, "2026-05-09T12-29-04-e7da7d-hermes.md");

    fs::remove_dir_all(dir).ok();
}

#[test]
fn test_derive_claude_output_path_uses_first_non_empty_line() {
    let dir = temp_dir("discover-claude");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.jsonl");
    fs::write(
        &path,
        "\n  \n{\"timestamp\":\"2025-11-12T21:52:16.079Z\"}\n{\"timestamp\":\"2025-11-12T22:00:00Z\"}\n",
    )
    .unwrap();

    let (folder, filename) = derive_claude_output_path(&path);
    assert_eq!(folder, "2025-11");
    assert_eq!(filename, "2025-11-12T21-52-16-claude.md");

    fs::remove_dir_all(dir).ok();
}

#[test]
fn test_find_grok_files_collects_chat_history_only() {
    let dir = temp_dir("discover-grok");
    let session_dir = dir.join("project").join("019f15e0-2078-7bb1-98d0-5554a486aafc");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(session_dir.join("chat_history.jsonl"), "{}\n").unwrap();
    fs::write(dir.join("project").join("prompt_history.jsonl"), "{}\n").unwrap();

    let mut results = Vec::new();
    find_grok_files(&dir, &mut results);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, Tool::Grok);
    assert!(results[0].1.ends_with("chat_history.jsonl"));

    fs::remove_dir_all(dir).ok();
}

#[test]
fn test_find_cursor_files_collects_agent_transcripts() {
    let dir = temp_dir("discover-cursor");
    let transcript_dir = dir
        .join("Users-me-app")
        .join("agent-transcripts")
        .join("abc");
    fs::create_dir_all(&transcript_dir).unwrap();
    fs::write(transcript_dir.join("abc.jsonl"), "{}\n").unwrap();
    fs::write(dir.join("Users-me-app").join("worker.log"), "x").unwrap();

    let mut results = Vec::new();
    find_cursor_files(&dir, &mut results);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, Tool::Cursor);

    fs::remove_dir_all(dir).ok();
}
