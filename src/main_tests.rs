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
