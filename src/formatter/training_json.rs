//! Training JSON formatter: pretty-print the `TrainingSession` sidecar only.
//!
//! The output is the sanitized training schema (events, stats, redaction report)
//! without re-embedding the full session AST. Indexing and external training
//! pipelines consume this artifact directly.

use std::io::Write;

use crate::error::CassioError;
use crate::formatter::Formatter;
use crate::training::ParsedSession;

pub struct TrainingJsonFormatter;

impl Formatter for TrainingJsonFormatter {
    fn format(&self, parsed: &ParsedSession, writer: &mut dyn Write) -> Result<(), CassioError> {
        serde_json::to_writer_pretty(&mut *writer, &parsed.training)?;
        writeln!(writer)?;
        Ok(())
    }
}
