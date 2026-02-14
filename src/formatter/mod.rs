pub mod emoji_text;
pub mod jsonl;

use std::io::Write;

use crate::ast::Session;
use crate::error::CassioError;

pub trait Formatter {
    fn format(&self, session: &Session, writer: &mut dyn Write) -> Result<(), CassioError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    EmojiText,
    Jsonl,
}

impl OutputFormat {
    pub fn formatter(&self) -> Box<dyn Formatter> {
        match self {
            OutputFormat::EmojiText => Box::new(emoji_text::EmojiTextFormatter),
            OutputFormat::Jsonl => Box::new(jsonl::JsonlFormatter),
        }
    }
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "emoji-text" | "text" => Ok(OutputFormat::EmojiText),
            "jsonl" | "json" => Ok(OutputFormat::Jsonl),
            _ => Err(format!("Unknown format: {s}. Valid: emoji-text, jsonl")),
        }
    }
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::EmojiText => write!(f, "emoji-text"),
            OutputFormat::Jsonl => write!(f, "jsonl"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_str_emoji_text() {
        assert_eq!("emoji-text".parse::<OutputFormat>().unwrap(), OutputFormat::EmojiText);
    }

    #[test]
    fn test_from_str_text_alias() {
        assert_eq!("text".parse::<OutputFormat>().unwrap(), OutputFormat::EmojiText);
    }

    #[test]
    fn test_from_str_jsonl() {
        assert_eq!("jsonl".parse::<OutputFormat>().unwrap(), OutputFormat::Jsonl);
    }

    #[test]
    fn test_from_str_json_alias() {
        assert_eq!("json".parse::<OutputFormat>().unwrap(), OutputFormat::Jsonl);
    }

    #[test]
    fn test_from_str_invalid() {
        assert!("xml".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn test_display_roundtrip() {
        let fmt = OutputFormat::EmojiText;
        assert_eq!(fmt.to_string().parse::<OutputFormat>().unwrap(), fmt);
        let fmt = OutputFormat::Jsonl;
        assert_eq!(fmt.to_string().parse::<OutputFormat>().unwrap(), fmt);
    }
}
