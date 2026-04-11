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
