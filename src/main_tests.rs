use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use serde_json::json;

fn temp_dir(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("cassio-{name}-{unique}"))
}

#[test]
fn index_options_from_config_uses_embedding_defaults() {
    let embedding = cassio::config::EmbeddingConfig {
        auto_index: true,
        provider: Some("ollama".to_string()),
        model: Some("cassio-embedding".to_string()),
        base_url: Some("http://127.0.0.1:11434".to_string()),
        include_training: true,
        include_paths: true,
        batch_size: Some(4),
        timeout_secs: Some(45),
    };
    let options = index_options_from_config(
        Some(&embedding),
        Some("2026-04".to_string()),
        false,
        false,
        None,
        None,
        None,
        None,
        None,
    );

    assert_eq!(options.month.as_deref(), Some("2026-04"));
    assert!(options.include_training);
    assert!(options.include_paths);
    assert_eq!(options.provider, "ollama");
    assert_eq!(options.model, "cassio-embedding");
    assert_eq!(options.base_url, "http://127.0.0.1:11434");
    assert_eq!(options.batch_size, 4);
    assert_eq!(options.timeout_secs, 45);
}

#[test]
fn test_derive_output_path_for_opencode_uses_session_timestamp() {
    let dir = temp_dir("main-opencode-path");
    let session_id = "ses_123";
    let message_dir = dir.join("message").join(session_id);
    fs::create_dir_all(&message_dir).unwrap();
    fs::create_dir_all(dir.join("session").join("proj_1")).unwrap();
    fs::write(
        dir.join("session")
            .join("proj_1")
            .join(format!("{session_id}.json")),
        r#"{
                "id": "ses_123",
                "time": { "created": 1704067200000.0 }
            }"#,
    )
    .unwrap();

    let (folder, filename) = derive_output_stem_for(Tool::OpenCode, &message_dir).unwrap();
    assert_eq!(folder, "2024-01");
    assert_eq!(filename, "2024-01-01T00-00-00-opencode");

    fs::remove_dir_all(dir).ok();
}

#[test]
fn test_derive_output_path_for_opencode_falls_back_when_metadata_missing() {
    let dir = temp_dir("main-opencode-fallback");
    let session_id = "ses_missing";
    let message_dir = dir.join("message").join(session_id);
    fs::create_dir_all(&message_dir).unwrap();

    let (folder, filename) = derive_output_stem_for(Tool::OpenCode, &message_dir).unwrap();
    assert_eq!(folder, "unknown");
    assert_eq!(filename, "ses_missing-opencode");

    fs::remove_dir_all(dir).ok();
}

#[test]
fn test_derive_output_path_for_pi_uses_filename_timestamp() {
    let path = PathBuf::from(
        "/sessions/2026-04-13T09-45-42-886Z_0c85082c-220c-4e56-8ae5-9463d6228494.jsonl",
    );
    let (folder, filename) = derive_output_stem_for(Tool::Pi, &path).unwrap();
    assert_eq!(folder, "2026-04");
    assert_eq!(filename, "2026-04-13T09-45-42-pi");
}

fn write_grok_session(dir: &PathBuf, id: &str, text: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("summary.json"),
        json!({
            "info": { "id": format!("019fb8eb-0000-7000-8000-00000000000{id}"), "cwd": "/tmp/proj" },
            "created_at": "2026-07-31T16:05:00.100000Z"
        })
        .to_string(),
    )
    .unwrap();
    let chat = format!(
        "{}\n{}\n",
        json!({"type": "user", "content": [{"type": "text", "text": format!("hello {text}")}]}),
        json!({"type": "assistant", "content": format!("hi {text}"), "model_id": "deepseek-v4-flash"})
    );
    fs::write(dir.join("chat_history.jsonl"), chat).unwrap();
}

#[test]
fn test_process_file_list_disambiguates_same_second_collision() {
    let dir = temp_dir("collision");
    let out = dir.join("out");
    write_grok_session(&dir.join("sess-a"), "a", "alpha");
    write_grok_session(&dir.join("sess-b"), "b", "bravo");

    let files = vec![
        (Tool::Grok, dir.join("sess-a/chat_history.jsonl")),
        (Tool::Grok, dir.join("sess-b/chat_history.jsonl")),
    ];
    process_file_list(
        &files,
        &out,
        None,
        false,
        OutputFormat::EmojiText,
        None,
        false,
    )
    .unwrap();

    // Both same-second sessions must be written, with distinct hash-suffixed names.
    let md_files = |root: &PathBuf| -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    let month = out.join("2026-07");
    let names = md_files(&month);
    assert_eq!(names.len(), 2, "both sessions must be written: {names:?}");
    // The earliest session (stable source-path sort: sess-a) keeps the
    // canonical name; the later one gets a hash suffix before the tool suffix.
    let canonical = "2026-07-31T16-05-00-grok.md";
    assert!(
        names.iter().any(|name| name == canonical),
        "names: {names:?}"
    );
    let suffixed = names
        .iter()
        .find(|name| *name != canonical)
        .expect("two names");
    assert!(suffixed.ends_with("-grok.md"), "names: {names:?}");
    assert!(
        suffixed.starts_with("2026-07-31T16-05-00-"),
        "names: {names:?}"
    );
    for name in &names {
        // Both must remain recognizable session transcripts so summary tables
        // and compact discovery still pick them up.
        assert!(
            cassio::ast::is_session_transcript_filename(name),
            "not a session transcript name: {name}"
        );
    }

    let canonical_contents: String = fs::read_to_string(month.join(canonical)).unwrap();
    let suffixed_contents: String = fs::read_to_string(month.join(suffixed)).unwrap();
    assert!(
        canonical_contents.contains("hello alpha"),
        "canonical holds first session"
    );
    assert!(
        suffixed_contents.contains("hello bravo"),
        "suffixed holds second session"
    );

    // Determinism: rerunning with --force rewrites the exact same names, so
    // Git only sees content changes (plus new files on the first run).
    process_file_list(
        &files,
        &out,
        None,
        true,
        OutputFormat::EmojiText,
        None,
        false,
    )
    .unwrap();
    assert_eq!(md_files(&month), names);

    fs::remove_dir_all(dir).ok();
}

#[test]
fn test_parallel_map_returns_results_in_item_order() {
    let items = [10, 20, 30, 40, 50];
    // Hand work out in a scrambled order; results must still line up with `items`.
    let order = [3, 1, 4, 0, 2];
    let results = parallel_map(&items, &order, 3, |index, value| value + index as i32);
    assert_eq!(results, vec![10, 21, 32, 43, 54]);

    let single = parallel_map(&items, &[0, 1, 2, 3, 4], 1, |_, value| *value);
    assert_eq!(single, items.to_vec());
}

#[test]
fn test_process_file_list_writes_every_session_in_a_large_collision_group() {
    let dir = temp_dir("collision-many");
    let out = dir.join("out");
    let ids = ["a", "b", "c", "d", "e", "f", "g", "h", "i"];
    let mut files = Vec::new();
    for id in ids {
        write_grok_session(&dir.join(format!("sess-{id}")), id, &format!("text-{id}"));
        files.push((
            Tool::Grok,
            dir.join(format!("sess-{id}/chat_history.jsonl")),
        ));
    }
    // A zero-byte session must be skipped without disturbing the others.
    let empty = dir.join("sess-empty");
    write_grok_session(&empty, "z", "never");
    fs::write(empty.join("chat_history.jsonl"), "").unwrap();
    files.push((Tool::Grok, empty.join("chat_history.jsonl")));

    process_file_list(
        &files,
        &out,
        None,
        false,
        OutputFormat::EmojiText,
        None,
        false,
    )
    .unwrap();

    let month = out.join("2026-07");
    let mut names: Vec<String> = fs::read_dir(&month)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".md"))
        .collect();
    names.sort();
    assert_eq!(names.len(), ids.len(), "names: {names:?}");

    // Exactly one session keeps the canonical name: the earliest source path.
    let canonical = "2026-07-31T16-05-00-grok.md";
    assert!(names.iter().any(|name| name == canonical), "{names:?}");
    assert!(
        fs::read_to_string(month.join(canonical))
            .unwrap()
            .contains("hello text-a")
    );

    // Every session's text lands in exactly one file, and none were lost.
    for id in ids {
        let needle = format!("hello text-{id}");
        let hits = names
            .iter()
            .filter(|name| {
                fs::read_to_string(month.join(name))
                    .map(|body| body.contains(&needle))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(hits, 1, "session {id} written {hits} times");
    }

    fs::remove_dir_all(dir).ok();
}

#[test]
fn test_is_up_to_date_uses_state_db_and_wal_for_hermes_virtual_paths() {
    use std::time::Duration;

    let dir = temp_dir("hermes-fresh");
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("state.db");
    let wal = dir.join("state.db-wal");
    let output = dir.join("out.md");
    for path in [&db, &wal, &output] {
        fs::write(path, "x").unwrap();
    }
    let base = SystemTime::now() - Duration::from_secs(1_000);
    let set = |path: &PathBuf, offset: u64| {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(base + Duration::from_secs(offset))
            .unwrap();
    };
    let session = dir.join("state.db/20260508_183040_abc123");

    // Output newer than both DB and WAL: nothing to regenerate.
    set(&db, 10);
    set(&wal, 20);
    set(&output, 30);
    assert!(is_up_to_date(&session, &output));

    // A newer WAL (uncheckpointed write) makes the session stale.
    set(&wal, 40);
    assert!(!is_up_to_date(&session, &output));

    // Without a WAL the DB file alone decides.
    fs::remove_file(&wal).unwrap();
    set(&db, 10);
    assert!(is_up_to_date(&session, &output));
    set(&db, 50);
    assert!(!is_up_to_date(&session, &output));

    // A missing database is never "up to date".
    fs::remove_file(&db).unwrap();
    assert!(!is_up_to_date(&session, &output));

    fs::remove_dir_all(dir).ok();
}
