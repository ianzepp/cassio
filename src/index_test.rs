use super::*;

#[test]
fn chunks_scrub_path_only_tool_lines_by_default() {
    let content = r#"✅ Read: file="/Users/ianzepp/github/thing/src/main.rs"
👤 Should we add semantic transcript indexing?
"#;
    let chunks = chunk_content(
        "2026-04/2026-04-30T10-00-00-codex.md",
        SearchArtifact::Session,
        content,
        false,
        1_800,
    );
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].line_start, 2);
    assert!(
        chunks[0]
            .embedding_text
            .contains("semantic transcript indexing")
    );
    assert!(!chunks[0].embedding_text.contains("/Users/"));
}

#[test]
fn include_paths_keeps_path_text_for_embeddings() {
    let content = r#"✅ Read: file="/Users/ianzepp/github/thing/src/main.rs""#;
    let chunks = chunk_content(
        "2026-04/2026-04-30T10-00-00-codex.md",
        SearchArtifact::Session,
        content,
        true,
        1_800,
    );
    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].embedding_text.contains("/Users/ianzepp"));
}

#[test]
fn oversized_single_lines_are_split_before_embedding() {
    let long = "x".repeat(4_250);
    let chunks = chunk_content(
        "2026-04/2026-04-30T10-00-00-codex.md",
        SearchArtifact::Session,
        &long,
        false,
        1_800,
    );

    assert_eq!(chunks.len(), 3);
    assert!(chunks.iter().all(|chunk| chunk.line_start == 1));
    assert!(chunks.iter().all(|chunk| chunk.line_end == 1));
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.embedding_text.len() <= 1_800)
    );
    assert_ne!(chunks[0].id, chunks[1].id);
    assert_ne!(chunks[1].id, chunks[2].id);
}

#[test]
fn unsplit_chunk_ids_keep_original_shape() {
    let id = chunk_id(
        "2026-04/2026-04-30.daily.md",
        SearchArtifact::Daily,
        10,
        12,
        0,
    );
    let old_shape = hash_text(&format!(
        "{}\0{}\0{}\0{}",
        "2026-04/2026-04-30.daily.md", "daily", 10, 12
    ));
    assert_eq!(id, old_shape);
}

#[test]
fn split_long_text_respects_character_boundaries() {
    let chunks = split_long_text("åß∂ƒ", 2);
    assert_eq!(chunks, vec!["åß".to_string(), "∂ƒ".to_string()]);
}

#[test]
fn default_file_selection_excludes_training_metadata() {
    let root = std::env::temp_dir().join(format!("cassio_index_files_{}", std::process::id()));
    let month = root.join("2026-04");
    fs::create_dir_all(&month).unwrap();
    fs::write(month.join("2026-04.monthly.md"), "monthly").unwrap();
    fs::write(month.join("2026-04-30.daily.md"), "daily").unwrap();
    fs::write(month.join("2026-04-30T10-00-00-codex.md"), "session").unwrap();
    fs::write(month.join("2026-04-30T10-00-00-codex.training.json"), "{}").unwrap();

    let files = files_to_index(&root, false);
    assert_eq!(files.len(), 3);
    let files = files_to_index(&root, true);
    assert_eq!(files.len(), 4);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn index_path_is_provider_model_scoped() {
    let path = index_path_for(
        Path::new("/tmp/transcripts"),
        "ollama",
        "cassio-embedding:latest",
    );
    assert_eq!(
        path,
        Path::new("/tmp/transcripts/.cassio/index/ollama-cassio-embedding-latest.sqlite")
    );
}

#[test]
fn parses_openai_compatible_embeddings() {
    let raw = r#"{
        "object": "list",
        "data": [
            {"object": "embedding", "index": 0, "embedding": [0.25, -0.5]},
            {"object": "embedding", "index": 1, "embedding": [1.0, 2.0]}
        ],
        "model": "text-embedding-nomic-embed-text-v1.5"
    }"#;

    let embeddings = parse_openai_embeddings(raw).unwrap();
    assert_eq!(embeddings, vec![vec![0.25, -0.5], vec![1.0, 2.0]]);
}

#[test]
fn embedding_encoding_is_little_endian_f32_blob() {
    let blob = encode_embedding(&[1.0, -2.0]);
    assert_eq!(blob.len(), 8);
    assert_eq!(f32::from_le_bytes(blob[0..4].try_into().unwrap()), 1.0);
    assert_eq!(f32::from_le_bytes(blob[4..8].try_into().unwrap()), -2.0);
}

#[test]
fn embedding_decoding_round_trips_f32_blob() {
    let blob = encode_embedding(&[0.25, 0.5, -1.0]);
    let decoded = decode_embedding(&blob).unwrap();
    assert_eq!(decoded, vec![0.25, 0.5, -1.0]);
}

#[test]
fn embedding_decoding_rejects_invalid_blob_length() {
    assert!(decode_embedding(&[1, 2, 3]).is_err());
}
