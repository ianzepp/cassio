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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_display() {
        assert_eq!(Tool::Claude.to_string(), "claude");
        assert_eq!(Tool::Codex.to_string(), "codex");
        assert_eq!(Tool::OpenCode.to_string(), "opencode");
    }

    #[test]
    fn test_tool_serde_roundtrip() {
        let json = serde_json::to_string(&Tool::Claude).unwrap();
        assert_eq!(json, "\"claude\"");
        let parsed: Tool = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Tool::Claude);
    }

    #[test]
    fn test_role_serde_roundtrip() {
        let json = serde_json::to_string(&Role::User).unwrap();
        assert_eq!(json, "\"user\"");
        let parsed: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Role::User);
    }

    #[test]
    fn test_content_block_text_serde() {
        let block = ContentBlock::Text { text: "hello".to_string() };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
        if let ContentBlock::Text { text } = parsed {
            assert_eq!(text, "hello");
        } else {
            panic!("expected Text variant");
        }
    }

    #[test]
    fn test_content_block_tool_use_serde() {
        let block = ContentBlock::ToolUse {
            id: "t1".to_string(),
            name: "Read".to_string(),
            input: serde_json::json!({"file_path": "/test.rs"}),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"tool_use\""));
        let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
        if let ContentBlock::ToolUse { id, name, .. } = parsed {
            assert_eq!(id, "t1");
            assert_eq!(name, "Read");
        } else {
            panic!("expected ToolUse variant");
        }
    }

    #[test]
    fn test_session_stats_default() {
        let stats = SessionStats::default();
        assert_eq!(stats.user_messages, 0);
        assert_eq!(stats.assistant_messages, 0);
        assert_eq!(stats.tool_calls, 0);
        assert_eq!(stats.tool_errors, 0);
        assert_eq!(stats.total_tokens.input_tokens, 0);
        assert_eq!(stats.total_tokens.output_tokens, 0);
        assert!(stats.files_read.is_empty());
        assert!(stats.duration_seconds.is_none());
        assert!(stats.cost.is_none());
    }

    #[test]
    fn test_token_usage_default() {
        let usage = TokenUsage::default();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.cache_creation_tokens, 0);
    }
}
