use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::error::CassioError;
use crate::search::{SearchArtifact, artifact_for_path, strip_path_noise};

const DEFAULT_PROVIDER: &str = "ollama";
const DEFAULT_MODEL: &str = "cassio-embedding";
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";
const DEFAULT_CHUNK_CHARS: usize = 1_800;

#[derive(Debug, Clone)]
pub struct IndexOptions {
    pub month: Option<String>,
    pub include_training: bool,
    pub include_paths: bool,
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub batch_size: usize,
    pub timeout_secs: u64,
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            month: None,
            include_training: false,
            include_paths: false,
            provider: DEFAULT_PROVIDER.to_string(),
            model: DEFAULT_MODEL.to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            batch_size: 16,
            timeout_secs: 120,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexReport {
    pub index_path: PathBuf,
    pub files: usize,
    pub chunks: usize,
    pub embedded: usize,
    pub reused: usize,
    pub stale_deleted: usize,
}

#[derive(Debug, Clone)]
struct IndexChunk {
    id: String,
    source_path: String,
    artifact: SearchArtifact,
    line_start: usize,
    line_end: usize,
    content_hash: String,
    chunk_text: String,
    embedding_text: String,
}

pub fn run_index(root: &Path, options: IndexOptions) -> Result<(), CassioError> {
    let report = build_index(root, &options)?;
    println!("cassio index: {}", root.display());
    println!("Index: {}", report.index_path.display());
    println!("Files scanned: {}", report.files);
    println!("Chunks indexed: {}", report.chunks);
    println!("Embedded: {}", report.embedded);
    println!("Reused: {}", report.reused);
    if report.stale_deleted > 0 {
        println!("Stale chunks deleted: {}", report.stale_deleted);
    }
    Ok(())
}

pub fn build_index(root: &Path, options: &IndexOptions) -> Result<IndexReport, CassioError> {
    let target = if let Some(month) = &options.month {
        root.join(month)
    } else {
        root.to_path_buf()
    };
    if !target.exists() {
        return Err(CassioError::Other(format!(
            "Index target does not exist: {}",
            target.display()
        )));
    }

    let files = files_to_index(&target, options.include_training);
    let mut chunks = Vec::new();
    for path in &files {
        chunks.extend(chunk_file(
            root,
            path,
            options.include_paths,
            DEFAULT_CHUNK_CHARS,
        )?);
    }

    let index_path = index_path_for(root, &options.provider, &options.model);
    let conn = open_index(&index_path, root, options)?;
    let (to_embed, reused) = changed_chunks(&conn, &chunks)?;
    let embedded = embed_and_store(&conn, &to_embed, options)?;
    let stale_deleted = delete_stale_chunks(&conn, &files, &chunks, root)?;

    Ok(IndexReport {
        index_path,
        files: files.len(),
        chunks: chunks.len(),
        embedded,
        reused,
        stale_deleted,
    })
}

fn files_to_index(root: &Path, include_training: bool) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(artifact) = artifact_for_path(path) else {
            continue;
        };
        if artifact == SearchArtifact::Training && !include_training {
            continue;
        }
        paths.push(path.to_path_buf());
    }
    paths.sort();
    paths
}

fn chunk_file(
    root: &Path,
    path: &Path,
    include_paths: bool,
    max_chars: usize,
) -> Result<Vec<IndexChunk>, CassioError> {
    let artifact = artifact_for_path(path).ok_or_else(|| {
        CassioError::Other(format!("Unsupported index artifact: {}", path.display()))
    })?;
    let source_path = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();
    let content = fs::read_to_string(path)?;
    Ok(chunk_content(
        &source_path,
        artifact,
        &content,
        include_paths,
        max_chars,
    ))
}

fn chunk_content(
    source_path: &str,
    artifact: SearchArtifact,
    content: &str,
    include_paths: bool,
    max_chars: usize,
) -> Vec<IndexChunk> {
    let mut chunks = Vec::new();
    let mut original_lines: Vec<String> = Vec::new();
    let mut embedding_lines: Vec<String> = Vec::new();
    let mut line_start = 0usize;
    let mut line_end = 0usize;

    for (index, line) in content.lines().enumerate() {
        let line_no = index + 1;
        let mut embedding_line = if include_paths {
            line.trim().to_string()
        } else {
            strip_path_noise(line).trim().to_string()
        };
        if !include_paths && is_tool_status_line(line) {
            embedding_line.clear();
        }

        if embedding_line.is_empty() {
            push_chunk(
                &mut chunks,
                source_path,
                artifact,
                &mut original_lines,
                &mut embedding_lines,
                line_start,
                line_end,
            );
            line_start = 0;
            line_end = 0;
            continue;
        }

        let next_len = embedding_lines.iter().map(String::len).sum::<usize>()
            + embedding_lines.len()
            + embedding_line.len();
        if !embedding_lines.is_empty() && next_len > max_chars {
            push_chunk(
                &mut chunks,
                source_path,
                artifact,
                &mut original_lines,
                &mut embedding_lines,
                line_start,
                line_end,
            );
            line_start = 0;
        }

        if embedding_lines.is_empty() {
            line_start = line_no;
        }
        line_end = line_no;
        original_lines.push(line.trim().to_string());
        embedding_lines.push(embedding_line);
    }

    push_chunk(
        &mut chunks,
        source_path,
        artifact,
        &mut original_lines,
        &mut embedding_lines,
        line_start,
        line_end,
    );

    chunks
}

fn push_chunk(
    chunks: &mut Vec<IndexChunk>,
    source_path: &str,
    artifact: SearchArtifact,
    original_lines: &mut Vec<String>,
    embedding_lines: &mut Vec<String>,
    line_start: usize,
    line_end: usize,
) {
    if embedding_lines.is_empty() {
        return;
    }

    let chunk_text = original_lines.join("\n");
    let embedding_text = embedding_lines.join("\n");
    let content_hash = hash_text(&embedding_text);
    let id = chunk_id(source_path, artifact, line_start, line_end);
    chunks.push(IndexChunk {
        id,
        source_path: source_path.to_string(),
        artifact,
        line_start,
        line_end,
        content_hash,
        chunk_text,
        embedding_text,
    });
    original_lines.clear();
    embedding_lines.clear();
}

fn is_tool_status_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('✅') || trimmed.starts_with('❌')
}

fn open_index(path: &Path, root: &Path, options: &IndexOptions) -> Result<Connection, CassioError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)
        .map_err(|e| CassioError::Other(format!("Failed to open index database: {e}")))?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS chunks (
            id TEXT PRIMARY KEY,
            source_path TEXT NOT NULL,
            artifact TEXT NOT NULL,
            line_start INTEGER NOT NULL,
            line_end INTEGER NOT NULL,
            content_hash TEXT NOT NULL,
            chunk_text TEXT NOT NULL,
            embedding_text TEXT NOT NULL,
            embedding BLOB NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_chunks_source_path ON chunks(source_path);
        CREATE INDEX IF NOT EXISTS idx_chunks_artifact ON chunks(artifact);
        "#,
    )
    .map_err(|e| CassioError::Other(format!("Failed to initialize index database: {e}")))?;

    set_metadata(&conn, "schema_version", "1")?;
    set_metadata(&conn, "source_root", &root.display().to_string())?;
    set_metadata(&conn, "provider", &options.provider)?;
    set_metadata(&conn, "model", &options.model)?;
    set_metadata(&conn, "base_url", &options.base_url)?;
    set_metadata(&conn, "updated_at", &Utc::now().to_rfc3339())?;

    Ok(conn)
}

fn set_metadata(conn: &Connection, key: &str, value: &str) -> Result<(), CassioError> {
    conn.execute(
        r#"
        INSERT INTO metadata(key, value)
        VALUES (?1, ?2)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        "#,
        params![key, value],
    )
    .map_err(|e| CassioError::Other(format!("Failed to write index metadata: {e}")))?;
    Ok(())
}

fn changed_chunks(
    conn: &Connection,
    chunks: &[IndexChunk],
) -> Result<(Vec<IndexChunk>, usize), CassioError> {
    let mut changed = Vec::new();
    let mut reused = 0usize;
    let mut stmt = conn
        .prepare("SELECT content_hash FROM chunks WHERE id = ?1")
        .map_err(|e| CassioError::Other(format!("Failed to query index: {e}")))?;
    for chunk in chunks {
        let existing: Option<String> = stmt
            .query_row(params![chunk.id], |row| row.get(0))
            .optional()
            .map_err(|e| CassioError::Other(format!("Failed to query index chunk: {e}")))?;
        if existing.as_deref() == Some(&chunk.content_hash) {
            reused += 1;
        } else {
            changed.push(chunk.clone());
        }
    }
    Ok((changed, reused))
}

fn embed_and_store(
    conn: &Connection,
    chunks: &[IndexChunk],
    options: &IndexOptions,
) -> Result<usize, CassioError> {
    if chunks.is_empty() {
        return Ok(0);
    }
    if options.provider != "ollama" {
        return Err(CassioError::Other(format!(
            "Unsupported index provider: {} (supported: ollama)",
            options.provider
        )));
    }

    let batch_size = options.batch_size.max(1);
    let mut embedded = 0usize;
    for batch in chunks.chunks(batch_size) {
        let input: Vec<&str> = batch
            .iter()
            .map(|chunk| chunk.embedding_text.as_str())
            .collect();
        let embeddings = embed_ollama(
            &options.base_url,
            &options.model,
            &input,
            Duration::from_secs(options.timeout_secs),
        )?;
        if embeddings.len() != batch.len() {
            return Err(CassioError::Other(format!(
                "Embedding provider returned {} embeddings for {} chunks",
                embeddings.len(),
                batch.len()
            )));
        }
        for (chunk, embedding) in batch.iter().zip(embeddings) {
            store_chunk(conn, chunk, &embedding)?;
            embedded += 1;
        }
    }
    Ok(embedded)
}

#[derive(Debug, Deserialize)]
struct OllamaEmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

fn embed_ollama(
    base_url: &str,
    model: &str,
    input: &[&str],
    timeout: Duration,
) -> Result<Vec<Vec<f32>>, CassioError> {
    let endpoint = format!("{}/api/embed", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "input": input,
    });
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .timeout_per_call(Some(timeout))
        .build()
        .into();
    let raw_response = agent
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .send_json(&body)
        .map_err(|e| CassioError::Other(format!("Ollama embedding request failed: {e}")))?
        .body_mut()
        .read_to_string()
        .map_err(|e| CassioError::Other(format!("Ollama embedding response read failed: {e}")))?;
    let response: OllamaEmbedResponse = serde_json::from_str(&raw_response).map_err(|e| {
        CassioError::Other(format!(
            "Ollama embedding response parse failed: {e}; body: {}",
            truncate_for_error(&raw_response)
        ))
    })?;
    Ok(response.embeddings)
}

fn store_chunk(
    conn: &Connection,
    chunk: &IndexChunk,
    embedding: &[f32],
) -> Result<(), CassioError> {
    let now = Utc::now().to_rfc3339();
    let embedding_blob = encode_embedding(embedding);
    conn.execute(
        r#"
        INSERT INTO chunks(
            id, source_path, artifact, line_start, line_end, content_hash,
            chunk_text, embedding_text, embedding, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        ON CONFLICT(id) DO UPDATE SET
            content_hash = excluded.content_hash,
            chunk_text = excluded.chunk_text,
            embedding_text = excluded.embedding_text,
            embedding = excluded.embedding,
            updated_at = excluded.updated_at
        "#,
        params![
            chunk.id,
            chunk.source_path,
            artifact_name(chunk.artifact),
            chunk.line_start as i64,
            chunk.line_end as i64,
            chunk.content_hash,
            chunk.chunk_text,
            chunk.embedding_text,
            embedding_blob,
            now
        ],
    )
    .map_err(|e| CassioError::Other(format!("Failed to store index chunk: {e}")))?;
    Ok(())
}

fn delete_stale_chunks(
    conn: &Connection,
    files: &[PathBuf],
    chunks: &[IndexChunk],
    root: &Path,
) -> Result<usize, CassioError> {
    let mut ids_by_source: HashMap<String, HashSet<String>> = HashMap::new();
    for chunk in chunks {
        ids_by_source
            .entry(chunk.source_path.clone())
            .or_default()
            .insert(chunk.id.clone());
    }

    let mut deleted = 0usize;
    for path in files {
        let source_path = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let keep = ids_by_source.get(&source_path).cloned().unwrap_or_default();
        let mut stmt = conn
            .prepare("SELECT id FROM chunks WHERE source_path = ?1")
            .map_err(|e| CassioError::Other(format!("Failed to query stale chunks: {e}")))?;
        let existing = stmt
            .query_map(params![source_path], |row| row.get::<_, String>(0))
            .map_err(|e| CassioError::Other(format!("Failed to read stale chunks: {e}")))?;
        let mut stale = Vec::new();
        for id in existing {
            let id = id.map_err(|e| CassioError::Other(format!("Failed to read chunk id: {e}")))?;
            if !keep.contains(&id) {
                stale.push(id);
            }
        }
        for id in stale {
            deleted += conn
                .execute("DELETE FROM chunks WHERE id = ?1", params![id])
                .map_err(|e| CassioError::Other(format!("Failed to delete stale chunk: {e}")))?;
        }
    }
    Ok(deleted)
}

fn index_path_for(root: &Path, provider: &str, model: &str) -> PathBuf {
    root.join(".cassio")
        .join("index")
        .join(format!("{}.sqlite", slug(&format!("{provider}-{model}"))))
}

fn slug(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn artifact_name(artifact: SearchArtifact) -> &'static str {
    match artifact {
        SearchArtifact::Monthly => "monthly",
        SearchArtifact::Daily => "daily",
        SearchArtifact::Session => "session",
        SearchArtifact::Training => "training",
    }
}

fn chunk_id(
    source_path: &str,
    artifact: SearchArtifact,
    line_start: usize,
    line_end: usize,
) -> String {
    hash_text(&format!(
        "{}\0{}\0{}\0{}",
        source_path,
        artifact_name(artifact),
        line_start,
        line_end
    ))
}

fn hash_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn encode_embedding(embedding: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(embedding.len() * std::mem::size_of::<f32>());
    for value in embedding {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

fn truncate_for_error(value: &str) -> String {
    const MAX: usize = 500;
    let mut chars = value.chars();
    let mut out: String = chars.by_ref().take(MAX).collect();
    if chars.next().is_some() {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
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
    fn embedding_encoding_is_little_endian_f32_blob() {
        let blob = encode_embedding(&[1.0, -2.0]);
        assert_eq!(blob.len(), 8);
        assert_eq!(f32::from_le_bytes(blob[0..4].try_into().unwrap()), 1.0);
        assert_eq!(f32::from_le_bytes(blob[4..8].try_into().unwrap()), -2.0);
    }
}
