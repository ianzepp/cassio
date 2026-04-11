use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ast::{Session, SessionStats, TokenUsage};

pub const TRAINING_SCHEMA_VERSION: &str = "training_session.v1";
pub const SANITIZATION_POLICY_VERSION: &str = "sanitization.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedSession {
    pub session: Session,
    pub training: TrainingSession,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingSession {
    pub schema_version: String,
    pub cassio_version: String,
    pub parser_version: String,
    pub source: TrainingSource,
    pub metadata: TrainingMetadata,
    pub events: Vec<TrainingEvent>,
    pub stats: TrainingStats,
    pub sanitization: SanitizationReport,
    pub quality_flags: QualityFlags,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingSource {
    pub tool: String,
    pub source_path: String,
    pub session_id: String,
    pub source_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_record_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingMetadata {
    pub project_path_raw: String,
    pub project_path_sanitized: String,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub session_kind: String,
    pub models_seen: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingEvent {
    pub event_id: String,
    pub sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub event_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sanitized_text: Option<String>,
    pub embedded_content_flags: EmbeddedContentFlags,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input_raw: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input_sanitized: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_output_raw: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_output_sanitized: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<EventUsage>,
    pub source_record_refs: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmbeddedContentFlags {
    pub contains_embedded_transcript: bool,
    pub contains_generated_summary: bool,
    pub contains_large_pasted_block: bool,
    pub contains_prompt_template: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingStats {
    pub user_messages: u32,
    pub assistant_messages: u32,
    pub tool_calls: u32,
    pub tool_errors: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<i64>,
    pub files_read: Vec<String>,
    pub files_written: Vec<String>,
    pub files_edited: Vec<String>,
    pub total_tokens: EventUsage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SanitizationReport {
    pub policy_version: String,
    pub redaction_count: u32,
    pub redaction_kinds: Vec<String>,
    pub dropped_block_count: u32,
    pub dropped_block_kinds: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityFlags {
    pub contains_embedded_transcript: bool,
    pub contains_generated_summary: bool,
    pub contains_large_pasted_block: bool,
    pub contains_prompt_template: bool,
    pub likely_meta_session: bool,
    pub tool_output_truncated: bool,
    pub ordering_confidence: String,
}

impl TrainingSession {
    pub fn new(
        parser_version: &str,
        source: TrainingSource,
        metadata: TrainingMetadata,
        stats: TrainingStats,
    ) -> Self {
        Self {
            schema_version: TRAINING_SCHEMA_VERSION.to_string(),
            cassio_version: env!("CARGO_PKG_VERSION").to_string(),
            parser_version: parser_version.to_string(),
            source,
            metadata,
            events: Vec::new(),
            stats,
            sanitization: SanitizationReport {
                policy_version: SANITIZATION_POLICY_VERSION.to_string(),
                redaction_count: 0,
                redaction_kinds: Vec::new(),
                dropped_block_count: 0,
                dropped_block_kinds: Vec::new(),
            },
            quality_flags: QualityFlags {
                contains_embedded_transcript: false,
                contains_generated_summary: false,
                contains_large_pasted_block: false,
                contains_prompt_template: false,
                likely_meta_session: false,
                tool_output_truncated: false,
                ordering_confidence: "high".to_string(),
            },
        }
    }

    pub fn push_event(&mut self, event: TrainingEvent) {
        self.merge_event_flags(&event.embedded_content_flags);
        if is_tool_output_truncated(&event) {
            self.quality_flags.tool_output_truncated = true;
        }
        if event.event_kind == "meta_record" || event.event_kind == "system_context" {
            self.quality_flags.likely_meta_session = true;
        }
        self.events.push(event);
    }

    pub fn record_dropped(&mut self, kind: &str) {
        self.sanitization.dropped_block_count += 1;
        push_unique(&mut self.sanitization.dropped_block_kinds, kind);
    }

    pub fn merge_event_flags(&mut self, flags: &EmbeddedContentFlags) {
        self.quality_flags.contains_embedded_transcript |= flags.contains_embedded_transcript;
        self.quality_flags.contains_generated_summary |= flags.contains_generated_summary;
        self.quality_flags.contains_large_pasted_block |= flags.contains_large_pasted_block;
        self.quality_flags.contains_prompt_template |= flags.contains_prompt_template;
    }
}

pub fn next_event_id(sequence: u64) -> String {
    format!("evt-{sequence:06}")
}

pub fn event_usage_from_tokens(usage: &TokenUsage) -> EventUsage {
    EventUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_tokens: usage.cache_read_tokens,
        cache_creation_tokens: usage.cache_creation_tokens,
    }
}

pub fn training_stats_from_session(stats: &SessionStats) -> TrainingStats {
    TrainingStats {
        user_messages: stats.user_messages,
        assistant_messages: stats.assistant_messages,
        tool_calls: stats.tool_calls,
        tool_errors: stats.tool_errors,
        duration_seconds: stats.duration_seconds,
        files_read: sorted_strings(stats.files_read.iter().cloned()),
        files_written: sorted_strings(stats.files_written.iter().cloned()),
        files_edited: sorted_strings(stats.files_edited.iter().cloned()),
        total_tokens: event_usage_from_tokens(&stats.total_tokens),
        cost_usd: stats.cost,
    }
}

pub fn sorted_strings<I>(values: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let set: BTreeSet<String> = values.into_iter().collect();
    set.into_iter().collect()
}

pub fn detect_embedded_content(text: &str) -> EmbeddedContentFlags {
    let lower = text.to_lowercase();
    EmbeddedContentFlags {
        contains_embedded_transcript: lower.contains("📋 session:")
            || lower.contains("\"schema_version\":\"training_session.")
            || lower.contains("\"event_kind\":")
            || lower.contains("assistant:")
            || lower.contains("user:")
            || lower.contains("tool_result"),
        contains_generated_summary: lower.contains("daily summary")
            || lower.contains("monthly summary")
            || lower.contains("generated summary")
            || lower.contains("compaction"),
        contains_large_pasted_block: text.len() > 8_000 || text.lines().count() > 200,
        contains_prompt_template: lower.contains("src/prompts/")
            || lower.contains("you are codex")
            || lower.contains("you are claude")
            || lower.contains("compact.md")
            || lower.contains("system prompt"),
    }
}

pub fn merge_embedded_flags(flags: &[EmbeddedContentFlags]) -> EmbeddedContentFlags {
    let mut merged = EmbeddedContentFlags::default();
    for flag in flags {
        merged.contains_embedded_transcript |= flag.contains_embedded_transcript;
        merged.contains_generated_summary |= flag.contains_generated_summary;
        merged.contains_large_pasted_block |= flag.contains_large_pasted_block;
        merged.contains_prompt_template |= flag.contains_prompt_template;
    }
    merged
}

pub fn hash_named_chunks<I, A, B>(chunks: I) -> String
where
    I: IntoIterator<Item = (A, B)>,
    A: AsRef<[u8]>,
    B: AsRef<[u8]>,
{
    let mut hasher = Sha256::new();
    for (name, body) in chunks {
        hasher.update(name.as_ref());
        hasher.update([0]);
        hasher.update(body.as_ref());
        hasher.update([0xff]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn is_tool_output_truncated(event: &TrainingEvent) -> bool {
    event
        .tool_output_raw
        .as_ref()
        .and_then(|value| value.as_str())
        .is_some_and(|text| text.ends_with("..."))
}
