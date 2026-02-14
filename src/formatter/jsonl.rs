use std::io::Write;

use crate::ast::Session;
use crate::error::CassioError;
use crate::formatter::Formatter;

pub struct JsonlFormatter;

impl Formatter for JsonlFormatter {
    fn format(&self, session: &Session, writer: &mut dyn Write) -> Result<(), CassioError> {
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
