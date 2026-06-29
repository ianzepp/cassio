//! Output formatters that render `ParsedSession` values for humans or downstream tools.
//!
//! `OutputFormat` selects among emoji-text (default), JSONL, and training JSON.
//! Each formatter implements the shared `Formatter` trait and writes to any `Write`
//! target so the CLI can stream to stdout or files without duplicating dispatch.

pub mod emoji_text;
pub mod jsonl;
pub mod training_json;

use std::io::Write;

use crate::error::CassioError;
use crate::training::ParsedSession;

pub trait Formatter {
    fn format(&self, parsed: &ParsedSession, writer: &mut dyn Write) -> Result<(), CassioError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    EmojiText,
    Jsonl,
    TrainingJson,
}

impl OutputFormat {
    pub fn formatter(&self) -> Box<dyn Formatter> {
        match self {
            OutputFormat::EmojiText => Box::new(emoji_text::EmojiTextFormatter),
            OutputFormat::Jsonl => Box::new(jsonl::JsonlFormatter),
            OutputFormat::TrainingJson => Box::new(training_json::TrainingJsonFormatter),
        }
    }
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "emoji-text" | "text" => Ok(OutputFormat::EmojiText),
            "jsonl" | "json" => Ok(OutputFormat::Jsonl),
            "training-json" | "training" => Ok(OutputFormat::TrainingJson),
            _ => Err(format!(
                "Unknown format: {s}. Valid: emoji-text, jsonl, training-json"
            )),
        }
    }
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::EmojiText => write!(f, "emoji-text"),
            OutputFormat::Jsonl => write!(f, "jsonl"),
            OutputFormat::TrainingJson => write!(f, "training-json"),
        }
    }
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;
