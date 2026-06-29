use std::io::Write;

use crate::error::CassioError;
use crate::formatter::Formatter;
use crate::training::ParsedSession;

pub struct JsonlFormatter;

impl Formatter for JsonlFormatter {
    fn format(&self, parsed: &ParsedSession, writer: &mut dyn Write) -> Result<(), CassioError> {
        let session = &parsed.session;
        // Output each message as a JSONL line, with metadata on the first line
        let meta_line = serde_json::to_string(&session.metadata)?;
        writeln!(writer, "{meta_line}")?;

        for msg in &session.messages {
            let line = serde_json::to_string(msg)?;
            writeln!(writer, "{line}")?;
        }

        // Stats as final line
        let stats_line = serde_json::to_string(&session.stats)?;
        writeln!(writer, "{stats_line}")?;

        Ok(())
    }
}

#[cfg(test)]
#[path = "jsonl_test.rs"]
mod tests;
