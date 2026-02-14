use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use walkdir::WalkDir;

use crate::error::CassioError;

const COMPACT_PROMPT: &str = include_str!("prompts/compact.md");
const MONTHLY_PROMPT: &str = include_str!("prompts/monthly.md");
const MONTHLY_MERGE_PROMPT: &str = include_str!("prompts/monthly_merge.md");

/// Max input bytes per LLM call (~150KB ≈ 37.5K tokens at 4 bytes/token).
const MAX_INPUT_BYTES: usize = 150 * 1024;

/// Run daily compaction: group sessions by date, extract, send to Claude, write .compaction.md.
pub fn run_dailies(
    input_dir: &Path,
    output_dir: &Path,
    limit: Option<usize>,
    model: &str,
) -> Result<(), CassioError> {
    let pending = find_pending_days(input_dir, output_dir)?;

    if pending.is_empty() {
        eprintln!("No pending days to compact.");
        return Ok(());
    }

    let to_process: Vec<_> = match limit {
        Some(n) => pending.into_iter().take(n).collect(),
        None => pending,
    };

    let total = to_process.len();
    let start = Instant::now();
    let mut compacted = 0u32;
    let mut failed = 0u32;

    for (i, (day, files)) in to_process.iter().enumerate() {
        let month = &day[..7];
        let relative = format!("{month}/{day}");
        eprint!("dailies: [{} of {total}] {relative}...", i + 1);

        match compact_day(output_dir, day, month, files, model) {
            Ok(true) => {
                eprintln!(" ok");
                compacted += 1;
            }
            Ok(false) => {
                eprintln!(" [FAIL]");
                failed += 1;
            }
            Err(e) => {
                eprintln!(" [ERROR: {e}]");
                failed += 1;
            }
        }
    }

    let elapsed = start.elapsed();
    eprintln!("finished: {}, {compacted} compacted, {failed} failed", format_elapsed(elapsed));
    Ok(())
}

/// Run monthly compaction for a single month (YYYY-MM).
/// Reads .compaction.md files, chunks if needed, produces YYYY-MM.monthly.md.
pub fn run_monthly(dir: &Path, month: &str, model: &str) -> Result<(), CassioError> {
    // Validate month format
    if month.len() != 7 || month.as_bytes()[4] != b'-' {
        return Err(CassioError::Other(format!(
            "Invalid month format: {month} (expected YYYY-MM)"
        )));
    }

    let month_dir = dir.join(month);
    if !month_dir.is_dir() {
        return Err(CassioError::Other(format!(
            "Month directory not found: {}",
            month_dir.display()
        )));
    }

    let output_path = month_dir.join(format!("{month}.monthly.md"));
    if output_path.exists() {
        eprintln!("monthly: {month}.monthly.md already exists, skipping");
        return Ok(());
    }

    // Collect compaction files sorted by date
    let mut compactions: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&month_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".compaction.md") {
                compactions.push(entry.path());
            }
        }
    }
    compactions.sort();

    if compactions.is_empty() {
        return Err(CassioError::Other(format!(
            "No .compaction.md files found in {month}/"
        )));
    }

    eprintln!(
        "monthly: {month} ({} compaction files)",
        compactions.len()
    );

    let start = Instant::now();

    // Read all compaction contents
    let mut contents: Vec<(String, String)> = Vec::new(); // (filename, content)
    for path in &compactions {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let content = std::fs::read_to_string(path)?;
        contents.push((name, content));
    }

    // Calculate total size to decide chunking strategy
    let prompt_overhead = MONTHLY_PROMPT.len() + 100; // prompt + delimiters
    let total_content_bytes: usize = contents.iter().map(|(_, c)| c.len()).sum();
    let total_with_prompt = total_content_bytes + prompt_overhead;

    let result = if total_with_prompt <= MAX_INPUT_BYTES {
        // Single pass — everything fits
        eprintln!("  single pass ({} bytes)", total_content_bytes);
        let input = build_monthly_input(MONTHLY_PROMPT, month, &contents);
        eprint!("  processing...");
        let output = invoke_claude(&input, model)?;
        eprintln!(" ok");
        output
    } else {
        // Chunked — split into chunks, summarize each, then merge
        let chunks = build_chunks(&contents, prompt_overhead);
        eprintln!(
            "  chunked: {} chunks ({} bytes total)",
            chunks.len(),
            total_content_bytes
        );

        let mut chunk_summaries = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let days: Vec<&str> = chunk.iter().map(|(n, _)| {
                n.strip_suffix(".compaction.md").unwrap_or(n.as_str())
            }).collect();
            eprint!(
                "  chunk [{} of {}] {} to {}...",
                i + 1,
                chunks.len(),
                days.first().unwrap_or(&"?"),
                days.last().unwrap_or(&"?"),
            );

            let input = build_monthly_input(MONTHLY_PROMPT, month, chunk);
            let output = invoke_claude(&input, model)?;

            if output.trim().is_empty() {
                eprintln!(" [FAIL]");
                return Err(CassioError::Other(format!(
                    "Claude returned empty output for chunk {}",
                    i + 1
                )));
            }

            eprintln!(" ok");
            chunk_summaries.push((format!("chunk-{}", i + 1), output));
        }

        // Merge pass
        eprint!("  merging {} chunk summaries...", chunk_summaries.len());
        let merge_input = build_monthly_input(MONTHLY_MERGE_PROMPT, month, &chunk_summaries);
        let merged = invoke_claude(&merge_input, model)?;
        eprintln!(" ok");
        merged
    };

    if result.trim().is_empty() {
        eprintln!("monthly: [FAIL] empty output");
        return Err(CassioError::Other("Claude returned empty output".into()));
    }

    std::fs::write(&output_path, &result)?;

    let elapsed = start.elapsed();
    eprintln!("finished: {}, wrote {month}.monthly.md", format_elapsed(elapsed));
    Ok(())
}

/// Find months that have .compaction.md files but no .monthly.md, and run monthly for each.
pub fn run_pending_monthlies(dir: &Path, model: &str) -> Result<(), CassioError> {
    let pending = find_pending_months(dir)?;

    if pending.is_empty() {
        eprintln!("No pending months to compact.");
        return Ok(());
    }

    eprintln!("Found {} pending month(s): {}", pending.len(), pending.join(", "));

    for month in &pending {
        run_monthly(dir, month, model)?;
    }

    Ok(())
}

/// Find month directories that have compaction files but no monthly summary yet.
fn find_pending_months(dir: &Path) -> Result<Vec<String>, CassioError> {
    let mut months = Vec::new();

    let entries = std::fs::read_dir(dir)
        .map_err(|e| CassioError::Other(format!("Cannot read directory {}: {e}", dir.display())))?;

    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        // Match YYYY-MM directory names
        if name.len() == 7 && name.as_bytes()[4] == b'-' && entry.path().is_dir() {
            let month_dir = entry.path();
            let monthly_path = month_dir.join(format!("{name}.monthly.md"));
            if monthly_path.exists() {
                continue;
            }
            // Check if there are any compaction files
            let has_compactions = std::fs::read_dir(&month_dir)
                .ok()
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .any(|e| {
                            e.file_name()
                                .to_string_lossy()
                                .ends_with(".compaction.md")
                        })
                })
                .unwrap_or(false);
            if has_compactions {
                months.push(name);
            }
        }
    }

    months.sort();
    Ok(months)
}

// --- daily helpers ---

/// Find days that have session .txt files but no .compaction.md yet.
fn find_pending_days(
    input_dir: &Path,
    output_dir: &Path,
) -> Result<Vec<(String, Vec<PathBuf>)>, CassioError> {
    let mut by_date: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();

    for entry in WalkDir::new(input_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };

        if name.ends_with(".txt") && name.len() >= 10 {
            let date = &name[..10];
            if date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-' {
                by_date
                    .entry(date.to_string())
                    .or_default()
                    .push(path.to_path_buf());
            }
        }
    }

    let mut pending = Vec::new();
    for (date, mut files) in by_date {
        let month = &date[..7];
        let compaction_path = output_dir.join(month).join(format!("{date}.compaction.md"));
        if compaction_path.exists() {
            continue;
        }
        files.sort();
        pending.push((date, files));
    }

    Ok(pending)
}

/// Compact a single day's sessions into a .compaction.md file.
fn compact_day(
    output_dir: &Path,
    day: &str,
    month: &str,
    files: &[PathBuf],
    model: &str,
) -> Result<bool, CassioError> {
    let mut input = String::new();
    input.push_str(COMPACT_PROMPT);
    input.push_str("\n\n---BEGIN TRANSCRIPTS---\n\n");

    for file in files {
        let extracted = extract_session(file)?;
        if !extracted.is_empty() {
            input.push_str(&extracted);
            input.push('\n');
        }
    }

    input.push_str("\n---END TRANSCRIPTS---\n");

    let output = invoke_claude(&input, model)?;

    if output.trim().is_empty() {
        return Ok(false);
    }

    let out_dir = output_dir.join(month);
    std::fs::create_dir_all(&out_dir)?;
    let out_path = out_dir.join(format!("{day}.compaction.md"));
    std::fs::write(&out_path, &output)?;

    Ok(true)
}

/// Extract key content from a session transcript.
fn extract_session(path: &Path) -> Result<String, CassioError> {
    let content = std::fs::read_to_string(path)?;
    let mut out = String::new();
    let mut in_llm = false;
    let mut llm_lines = 0u32;
    const LLM_LINE_LIMIT: u32 = 5;

    for line in content.lines() {
        if line.starts_with("📋") {
            out.push_str(line);
            out.push('\n');
            in_llm = false;
        } else if line.starts_with("👤") {
            out.push_str(line);
            out.push('\n');
            in_llm = false;
            llm_lines = 0;
        } else if line.starts_with("🤖") {
            out.push_str(line);
            out.push('\n');
            in_llm = true;
            llm_lines = 0;
        } else if line.starts_with("✅") || line.starts_with("❌") {
            in_llm = false;
        } else if in_llm {
            if !line.trim().is_empty() {
                llm_lines += 1;
                if llm_lines <= LLM_LINE_LIMIT {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
    }

    Ok(out)
}

// --- monthly helpers ---

/// Build the full input string for a monthly (or chunk) LLM call.
fn build_monthly_input(prompt: &str, month: &str, items: &[(String, String)]) -> String {
    let mut input = String::new();
    input.push_str(prompt);
    input.push_str(&format!(
        "\n\n---BEGIN MONTHLY COMPACTIONS ({month}, {} days)---\n\n",
        items.len()
    ));

    for (name, content) in items {
        input.push_str(content);
        input.push_str(&format!("\n--- (end {name}) ---\n\n"));
    }

    input.push_str("---END MONTHLY COMPACTIONS---\n");
    input
}

/// Split compaction contents into chunks that fit within MAX_INPUT_BYTES.
/// Each chunk is a slice of (filename, content) pairs.
fn build_chunks(contents: &[(String, String)], prompt_overhead: usize) -> Vec<Vec<(String, String)>> {
    let budget = MAX_INPUT_BYTES - prompt_overhead;
    let mut chunks: Vec<Vec<(String, String)>> = Vec::new();
    let mut current_chunk: Vec<(String, String)> = Vec::new();
    let mut current_size: usize = 0;

    for (name, content) in contents {
        let entry_size = content.len() + name.len() + 30; // delimiters overhead

        // If a single file exceeds the budget, it gets its own chunk
        if !current_chunk.is_empty() && current_size + entry_size > budget {
            chunks.push(std::mem::take(&mut current_chunk));
            current_size = 0;
        }

        current_chunk.push((name.clone(), content.clone()));
        current_size += entry_size;
    }

    if !current_chunk.is_empty() {
        chunks.push(current_chunk);
    }

    chunks
}

// --- shared helpers ---

/// Invoke `claude -p --model <model>` with the given input on stdin.
fn invoke_claude(input: &str, model: &str) -> Result<String, CassioError> {
    let mut child = std::process::Command::new("claude")
        .args(["-p", "--model", model])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| CassioError::Other(format!("Failed to start claude: {e}")))?;

    if let Some(ref mut stdin) = child.stdin {
        stdin
            .write_all(input.as_bytes())
            .map_err(|e| CassioError::Other(format!("Failed to write to claude stdin: {e}")))?;
    }
    drop(child.stdin.take());

    let output = child
        .wait_with_output()
        .map_err(|e| CassioError::Other(format!("Failed to read claude output: {e}")))?;

    if !output.status.success() {
        return Err(CassioError::Other(format!(
            "claude exited with status {}",
            output.status
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn format_elapsed(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_elapsed_seconds() {
        let d = std::time::Duration::from_secs(45);
        assert_eq!(format_elapsed(d), "45s");
    }

    #[test]
    fn test_format_elapsed_minutes() {
        let d = std::time::Duration::from_secs(125);
        assert_eq!(format_elapsed(d), "2m05s");
    }

    #[test]
    fn test_build_monthly_input_structure() {
        let items = vec![
            ("2025-01-15".to_string(), "day 15 content".to_string()),
            ("2025-01-16".to_string(), "day 16 content".to_string()),
        ];
        let result = build_monthly_input("PROMPT", "2025-01", &items);
        assert!(result.starts_with("PROMPT"));
        assert!(result.contains("2025-01"));
        assert!(result.contains("2 days"));
        assert!(result.contains("day 15 content"));
        assert!(result.contains("day 16 content"));
        assert!(result.contains("---BEGIN MONTHLY COMPACTIONS"));
        assert!(result.contains("---END MONTHLY COMPACTIONS---"));
    }

    #[test]
    fn test_build_chunks_single_chunk() {
        let contents = vec![
            ("a".to_string(), "small content".to_string()),
        ];
        let chunks = build_chunks(&contents, 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 1);
    }

    #[test]
    fn test_build_chunks_multiple() {
        // Create content that exceeds MAX_INPUT_BYTES when combined
        let big = "x".repeat(100_000);
        let contents = vec![
            ("a".to_string(), big.clone()),
            ("b".to_string(), big.clone()),
        ];
        let chunks = build_chunks(&contents, 100);
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn test_build_chunks_oversized_single_file() {
        // Single file that exceeds budget still gets its own chunk
        let huge = "x".repeat(200_000);
        let contents = vec![
            ("small".to_string(), "tiny".to_string()),
            ("big".to_string(), huge),
        ];
        let chunks = build_chunks(&contents, 100);
        assert!(chunks.len() >= 2);
    }

    #[test]
    fn test_extract_session_filters_correctly() {
        let content = "\
📋 Session: abc
📋 Project: /proj
👤 user prompt here
🤖 assistant response line 1
second line of response
third line
fourth line
fifth line
sixth line
seventh line that should be cut
✅ Read: file=\"test.rs\"
👤 another question
";
        // Write to temp file and test extract_session
        let dir = std::env::temp_dir().join("cassio_test_extract");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.txt");
        std::fs::write(&path, content).unwrap();

        let result = extract_session(&path).unwrap();

        // Should include metadata, user, and assistant lines
        assert!(result.contains("📋 Session: abc"));
        assert!(result.contains("👤 user prompt here"));
        assert!(result.contains("🤖 assistant response line 1"));
        // Lines after LLM_LINE_LIMIT (5) should be cut
        assert!(result.contains("sixth line"));
        assert!(!result.contains("seventh line"));
        // Tool result lines (✅/❌) should be excluded
        assert!(!result.contains("✅ Read"));
        // Second user prompt should be included
        assert!(result.contains("👤 another question"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
