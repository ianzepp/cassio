use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Claude,
    Codex,
    OpenCode,
}

impl std::fmt::Display for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tool::Claude => write!(f, "claude"),
            Tool::Codex => write!(f, "codex"),
            Tool::OpenCode => write!(f, "opencode"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub metadata: SessionMetadata,
    pub messages: Vec<Message>,
    pub stats: SessionStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub tool: Tool,
    pub project_path: String,
    pub started_at: DateTime<Utc>,
    pub version: Option<String>,
    pub git_branch: Option<String>,
    pub model: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub timestamp: Option<DateTime<Utc>>,
    pub model: Option<String>,
    pub content: Vec<ContentBlock>,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        name: String,
        success: bool,
        summary: String,
    },
    ModelChange {
        model: String,
    },
    QueueOperation {
        summary: String,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    pub user_messages: u32,
    pub assistant_messages: u32,
    pub tool_calls: u32,
    pub tool_errors: u32,
    pub total_tokens: TokenUsage,
    pub files_read: HashSet<String>,
    pub files_written: HashSet<String>,
    pub files_edited: HashSet<String>,
    pub duration_seconds: Option<i64>,
    pub cost: Option<f64>,
}

impl Default for SessionStats {
    fn default() -> Self {
        Self {
            user_messages: 0,
            assistant_messages: 0,
            tool_calls: 0,
            tool_errors: 0,
            total_tokens: TokenUsage::default(),
            files_read: HashSet::new(),
            files_written: HashSet::new(),
            files_edited: HashSet::new(),
            duration_seconds: None,
            cost: None,
        }
    }
}
