use std::collections::{HashMap, HashSet};
use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::Utc;
use llama_cpp_2::context::params::{LlamaAttentionType, LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::{AddBos, LlamaModel, params::LlamaModelParams};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::error::CassioError;
use crate::search::{SearchArtifact, artifact_for_path, strip_path_noise};

const DEFAULT_PROVIDER: &str = "builtin";
const DEFAULT_MODEL: &str = "nomic-embed-text-v1.5.Q4_K_M";
const DEFAULT_BASE_URL: &str = "";
const DEFAULT_CHUNK_CHARS: usize = 1_800;
const BUILTIN_MODEL_BYTES: &[u8] = include_bytes!("../assets/nomic-embed-text-v1.5.Q4_K_M.gguf");
const BUILTIN_MODEL_FILE: &str = "nomic-embed-text-v1.5.Q4_K_M.gguf";

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

    eprintln!("index: target {}", target.display());
    eprintln!(
        "index: provider={} model={} base_url={}",
        options.provider, options.model, options.base_url
    );

    let files = files_to_index(&target, options.include_training);
    eprintln!("index: found {} file(s) to scan", files.len());
    let mut chunks = Vec::new();
    for (index, path) in files.iter().enumerate() {
        chunks.extend(chunk_file(
            root,
            path,
            options.include_paths,
            DEFAULT_CHUNK_CHARS,
        )?);
        let current = index + 1;
        if current == 1 || current == files.len() || current % 100 == 0 {
            eprintln!(
                "index: chunked {}/{} file(s), {} chunk(s) prepared",
                current,
                files.len(),
                chunks.len()
            );
        }
    }
    eprintln!(
        "index: prepared {} chunk(s) from {} file(s)",
        chunks.len(),
        files.len()
    );

    let index_path = index_path_for(root, &options.provider, &options.model);
    eprintln!("index: database {}", index_path.display());
    let conn = open_index(&index_path, root, options)?;
    let (to_embed, reused) = changed_chunks(&conn, &chunks)?;
    let embedded = embed_and_store(&conn, &to_embed, options)?;
    let stale_deleted = delete_stale_chunks(&conn, &files, &chunks, root)?;
    if stale_deleted > 0 {
        eprintln!("index: deleted {stale_deleted} stale chunk(s)");
    }

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

        if embedding_line.len() > max_chars {
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
            for (segment_index, segment) in split_long_text(&embedding_line, max_chars)
                .into_iter()
                .enumerate()
            {
                push_index_chunk(
                    &mut chunks,
                    source_path,
                    artifact,
                    line_no,
                    line_no,
                    segment_index,
                    segment.clone(),
                    segment,
                );
            }
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
    push_index_chunk(
        chunks,
        source_path,
        artifact,
        line_start,
        line_end,
        0,
        chunk_text,
        embedding_text,
    );
    original_lines.clear();
    embedding_lines.clear();
}

#[allow(clippy::too_many_arguments)]
fn push_index_chunk(
    chunks: &mut Vec<IndexChunk>,
    source_path: &str,
    artifact: SearchArtifact,
    line_start: usize,
    line_end: usize,
    segment_index: usize,
    chunk_text: String,
    embedding_text: String,
) {
    let content_hash = hash_text(&embedding_text);
    let id = chunk_id(source_path, artifact, line_start, line_end, segment_index);
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
}

fn is_tool_status_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('✅') || trimmed.starts_with('❌')
}

fn split_long_text(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if current.chars().count() >= max_chars {
            out.push(current);
            current = String::new();
        }
        current.push(ch);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
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
    eprintln!(
        "index: checking {} chunk(s) against existing index",
        chunks.len()
    );
    let mut stmt = conn
        .prepare("SELECT content_hash FROM chunks WHERE id = ?1")
        .map_err(|e| CassioError::Other(format!("Failed to query index: {e}")))?;
    for (index, chunk) in chunks.iter().enumerate() {
        let existing: Option<String> = stmt
            .query_row(params![chunk.id], |row| row.get(0))
            .optional()
            .map_err(|e| CassioError::Other(format!("Failed to query index chunk: {e}")))?;
        if existing.as_deref() == Some(&chunk.content_hash) {
            reused += 1;
        } else {
            changed.push(chunk.clone());
        }
        let current = index + 1;
        if current == chunks.len() || current % 1_000 == 0 {
            eprintln!(
                "index: checked {}/{} chunk(s), reused {}, pending embed {}",
                current,
                chunks.len(),
                reused,
                changed.len()
            );
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
        eprintln!("index: all chunks already indexed; no embeddings needed");
        return Ok(0);
    }
    let batch_size = options.batch_size.max(1);
    let total_batches = chunks.len().div_ceil(batch_size);
    eprintln!(
        "index: embedding {} chunk(s) in {} batch(es), batch_size={}",
        chunks.len(),
        total_batches,
        batch_size
    );
    let mut embedded = 0usize;
    for (batch_index, batch) in chunks.chunks(batch_size).enumerate() {
        let batch_number = batch_index + 1;
        let start = embedded + 1;
        let end = embedded + batch.len();
        eprintln!(
            "index: embedding batch {}/{} (chunk {}-{} of {})",
            batch_number,
            total_batches,
            start,
            end,
            chunks.len()
        );
        let input: Vec<&str> = batch
            .iter()
            .map(|chunk| chunk.embedding_text.as_str())
            .collect();
        let embeddings = embed_texts(
            &options.provider,
            &options.base_url,
            &options.model,
            &input,
            options.timeout_secs,
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
        eprintln!(
            "index: stored batch {}/{}; embedded {}/{} chunk(s)",
            batch_number,
            total_batches,
            embedded,
            chunks.len()
        );
    }
    Ok(embedded)
}

#[derive(Debug, Deserialize)]
struct OllamaEmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiEmbeddingsResponse {
    data: Vec<OpenAiEmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct OpenAiEmbeddingData {
    embedding: Vec<f32>,
}

pub(crate) fn embed_texts(
    provider: &str,
    base_url: &str,
    model: &str,
    input: &[&str],
    timeout_secs: u64,
) -> Result<Vec<Vec<f32>>, CassioError> {
    let timeout = Duration::from_secs(timeout_secs);
    match provider {
        "builtin" => embed_builtin(input),
        "ollama" => embed_ollama(base_url, model, input, timeout),
        "openai" | "lmstudio" => embed_openai_compatible(base_url, model, input, timeout),
        _ => Err(CassioError::Other(format!(
            "Unsupported embedding provider: {provider} (supported: builtin, ollama, openai, lmstudio)"
        ))),
    }
}

fn embed_builtin(input: &[&str]) -> Result<Vec<Vec<f32>>, CassioError> {
    let backend = builtin_backend()?;
    let model_path = builtin_model_path()?;
    let model = LlamaModel::load_from_file(backend, &model_path, &LlamaModelParams::default())
        .map_err(|e| CassioError::Other(format!("Builtin embedding model load failed: {e}")))?;
    let params = LlamaContextParams::default()
        .with_embeddings(true)
        .with_pooling_type(LlamaPoolingType::Mean)
        .with_attention_type(LlamaAttentionType::NonCausal)
        .with_n_ctx(NonZeroU32::new(2048))
        .with_n_batch(2048)
        .with_n_ubatch(2048)
        .with_n_seq_max(1);
    let mut context = model
        .new_context(backend, params)
        .map_err(|e| CassioError::Other(format!("Builtin embedding context load failed: {e}")))?;

    let mut out = Vec::with_capacity(input.len());
    for text in input {
        let tokens = model
            .str_to_token(text, AddBos::Always)
            .map_err(|e| CassioError::Other(format!("Builtin embedding tokenize failed: {e}")))?;
        if tokens.is_empty() {
            return Err(CassioError::Other(
                "Builtin embedding tokenizer returned no tokens".into(),
            ));
        }
        if tokens.len() > context.n_batch() as usize {
            return Err(CassioError::Other(format!(
                "Builtin embedding input has {} tokens, exceeding batch size {}",
                tokens.len(),
                context.n_batch()
            )));
        }

        context.clear_kv_cache();
        let mut batch = LlamaBatch::new(tokens.len(), 1);
        for (pos, token) in tokens.iter().enumerate() {
            batch
                .add(*token, pos as i32, &[0], true)
                .map_err(|e| CassioError::Other(format!("Builtin embedding batch failed: {e}")))?;
        }
        context
            .encode(&mut batch)
            .map_err(|e| CassioError::Other(format!("Builtin embedding encode failed: {e}")))?;
        let embedding = context
            .embeddings_seq_ith(0)
            .map_err(|e| CassioError::Other(format!("Builtin embedding read failed: {e}")))?;
        out.push(normalize_l2(embedding));
    }
    Ok(out)
}

fn builtin_backend() -> Result<&'static LlamaBackend, CassioError> {
    static BACKEND: OnceLock<LlamaBackend> = OnceLock::new();
    if let Some(backend) = BACKEND.get() {
        return Ok(backend);
    }

    let mut backend = LlamaBackend::init().map_err(|e| {
        CassioError::Other(format!(
            "Builtin embedding backend initialization failed: {e}"
        ))
    })?;
    backend.void_logs();
    let _ = BACKEND.set(backend);
    BACKEND
        .get()
        .ok_or_else(|| CassioError::Other("Builtin embedding backend unavailable".into()))
}

fn builtin_model_path() -> Result<PathBuf, CassioError> {
    let base = dirs::cache_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("cassio").join("models");
    fs::create_dir_all(&dir)?;
    let path = dir.join(BUILTIN_MODEL_FILE);
    let current_len = fs::metadata(&path).map(|meta| meta.len()).ok();
    if current_len != Some(BUILTIN_MODEL_BYTES.len() as u64) {
        fs::write(&path, BUILTIN_MODEL_BYTES)?;
    }
    Ok(path)
}

fn normalize_l2(values: &[f32]) -> Vec<f32> {
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm == 0.0 {
        return values.to_vec();
    }
    values.iter().map(|value| value / norm).collect()
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

fn embed_openai_compatible(
    base_url: &str,
    model: &str,
    input: &[&str],
    timeout: Duration,
) -> Result<Vec<Vec<f32>>, CassioError> {
    let endpoint = format!("{}/embeddings", base_url.trim_end_matches('/'));
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
        .map_err(|e| {
            CassioError::Other(format!("OpenAI-compatible embedding request failed: {e}"))
        })?
        .body_mut()
        .read_to_string()
        .map_err(|e| {
            CassioError::Other(format!(
                "OpenAI-compatible embedding response read failed: {e}"
            ))
        })?;
    parse_openai_embeddings(&raw_response)
}

fn parse_openai_embeddings(raw_response: &str) -> Result<Vec<Vec<f32>>, CassioError> {
    let response: OpenAiEmbeddingsResponse = serde_json::from_str(raw_response).map_err(|e| {
        CassioError::Other(format!(
            "OpenAI-compatible embedding response parse failed: {e}; body: {}",
            truncate_for_error(raw_response)
        ))
    })?;
    Ok(response
        .data
        .into_iter()
        .map(|item| item.embedding)
        .collect())
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

pub(crate) fn index_path_for(root: &Path, provider: &str, model: &str) -> PathBuf {
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
    segment_index: usize,
) -> String {
    if segment_index == 0 {
        hash_text(&format!(
            "{}\0{}\0{}\0{}",
            source_path,
            artifact_name(artifact),
            line_start,
            line_end
        ))
    } else {
        hash_text(&format!(
            "{}\0{}\0{}\0{}\0{}",
            source_path,
            artifact_name(artifact),
            line_start,
            line_end,
            segment_index
        ))
    }
}

fn hash_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn encode_embedding(embedding: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(std::mem::size_of_val(embedding));
    for value in embedding {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

pub(crate) fn decode_embedding(blob: &[u8]) -> Result<Vec<f32>, CassioError> {
    if !blob.len().is_multiple_of(std::mem::size_of::<f32>()) {
        return Err(CassioError::Other(format!(
            "Invalid embedding blob length: {}",
            blob.len()
        )));
    }
    Ok(blob
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|bytes| {
            let mut raw = [0u8; std::mem::size_of::<f32>()];
            raw.copy_from_slice(bytes);
            f32::from_le_bytes(raw)
        })
        .collect())
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
#[path = "index_test.rs"]
mod tests;
