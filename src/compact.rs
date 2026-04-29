//! Compaction pipeline: daily and monthly LLM summarization of session transcripts.
//!
//! # Architecture overview
//!
//! Compaction is the second stage of the cassio pipeline, operating on the
//! formatted session transcripts produced by the session stage:
//!
//! ```text
//! Sessions (.md) → extract_session → LLM prompt → .daily.md
//! Daily compactions → build_monthly_input → LLM prompt → .monthly.md
//! ```
//!
//! # Daily compaction
//!
//! `run_dailies` groups session transcript files by their date prefix (YYYY-MM-DD),
//! skips days that already have a `.daily.md`, and sends the remaining days
//! to the LLM one at a time.
//!
//! The `extract_session` function reduces each transcript to a compact signal:
//! metadata lines, user messages, the first 5 lines of each assistant response,
//! and no tool call details. This compression is intentional — the LLM receives
//! the human-level conversation, not raw tool I/O.
//!
//! # Monthly compaction
//!
//! `run_monthly` aggregates all `.daily.md` files for a month. When the
//! combined size fits within the 150KB input budget, a single LLM call suffices.
//! When it doesn't, a chunked multi-pass approach is used: each chunk is summarized
//! separately, then a merge pass synthesizes the chunk summaries into a final monthly.
//!
//! # LLM provider abstraction
//!
//! Providers supported:
//! - **ollama** — `ollama run <model>` (stdin/stdout)
//! - **claude** — `claude -p --model <model>` (stdin/stdout)
//! - **codex** — `codex exec -m <model> -o <tmpfile>` (stdin + output file)
//! - **openrouter** — direct HTTPS call using `OPENROUTER_API_KEY`
//! - **openai** — OpenAI-compatible chat-completions endpoint configured by
//!   `base_url`, including local llama.cpp servers
//!
//! WHY mostly external CLIs rather than API calls: cassio avoids bespoke auth flows
//! where possible. `openrouter` is the exception because it provides a stable
//! generic chat-completions endpoint across many hosted models.
//!
//! # TRADE-OFFS
//!
//! - `extract_session` limits assistant response lines to 5 per message. This loses
//!   detail but keeps compaction prompts within token budgets. The 5-line limit was
//!   chosen empirically — enough context for the LLM to understand what was discussed.
//! - Chunking uses a 150KB byte budget as a proxy for token count, assuming ~4 bytes
//!   per token. This is conservative; actual LLM context limits vary by provider and
//!   model. Users with larger context windows can increase `MAX_INPUT_BYTES`.
//! - `BTreeMap` is used for date grouping so days are always processed in
//!   chronological order without an explicit sort step.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Output};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use wait_timeout::ChildExt;
use walkdir::WalkDir;

use crate::error::CassioError;

const COMPACT_PROMPT: &str = include_str!("prompts/compact.md");
const DAILY_MERGE_PROMPT: &str = include_str!("prompts/daily_merge.md");
const MONTHLY_PROMPT: &str = include_str!("prompts/monthly.md");
const MONTHLY_MERGE_PROMPT: &str = include_str!("prompts/monthly_merge.md");

/// Max input bytes per LLM call (~150KB ≈ 37.5K tokens at 4 bytes/token).
///
/// This is intentionally conservative. Most providers support larger contexts,
/// but staying well under the limit avoids truncation errors and ensures fast
/// response times from smaller models.
const MAX_INPUT_BYTES: usize = 150 * 1024;
const LLM_TIMEOUT: Duration = Duration::from_secs(300);
const LLM_RETRY_LIMIT: usize = 3;
const RETRY_BACKOFF_SECS: [u64; 2] = [2, 5];

enum DailyCompactionPlan {
    Single(String),
    Chunked(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum FailureClass {
    Timeout,
    Process,
    ProviderHttpError,
    Transport,
    ParseError,
    EmptyResponse,
    Io,
}

impl FailureClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Process => "process",
            Self::ProviderHttpError => "provider_http_error",
            Self::Transport => "transport",
            Self::ParseError => "parse_error",
            Self::EmptyResponse => "empty_response",
            Self::Io => "io",
        }
    }

    fn retryable(self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::Process | Self::ProviderHttpError | Self::Transport
        )
    }
}

#[derive(Debug)]
struct InvocationError {
    class: FailureClass,
    detail: String,
    raw_response_body: Option<String>,
}

impl InvocationError {
    fn new(class: FailureClass, detail: impl Into<String>) -> Self {
        Self {
            class,
            detail: detail.into(),
            raw_response_body: None,
        }
    }

    fn with_raw_response_body(mut self, raw_response_body: impl Into<String>) -> Self {
        self.raw_response_body = Some(raw_response_body.into());
        self
    }
}

#[derive(Debug, Serialize)]
struct DailyCheckpointStatus<'a> {
    day: &'a str,
    status: &'a str,
    phase: &'a str,
    provider: &'a str,
    model: &'a str,
    total_chunks: usize,
    completed_chunks: usize,
    finalized_chunks: usize,
    failure_class: Option<&'a str>,
    failure_detail: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct CompactOptions {
    pub chunk_timeout: Duration,
    pub max_retries: usize,
    pub resume: bool,
}

impl CompactOptions {
    pub fn new(chunk_timeout_secs: u64, max_retries: usize) -> Self {
        Self {
            chunk_timeout: Duration::from_secs(chunk_timeout_secs.max(1)),
            max_retries: max_retries.max(1),
            resume: true,
        }
    }
}

impl Default for CompactOptions {
    fn default() -> Self {
        Self {
            chunk_timeout: LLM_TIMEOUT,
            max_retries: LLM_RETRY_LIMIT,
            resume: true,
        }
    }
}

#[derive(Debug)]
struct DayFailure {
    detail: String,
}

#[derive(Debug, Serialize)]
struct ProgressEvent<'a> {
    day: &'a str,
    phase: &'a str,
    chunk_index: Option<usize>,
    total_chunks: usize,
    retry_count: usize,
    elapsed_ms: u128,
    outcome: &'a str,
    failure_class: Option<&'a str>,
    detail: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
struct InvocationContext<'a> {
    checkpoint_dir: &'a Path,
    day: &'a str,
    phase: &'a str,
    chunk_index: Option<usize>,
    total_chunks: usize,
}

#[derive(Debug, Clone)]
pub struct DailyRunReport {
    pub compacted: usize,
    pub failed: usize,
    pub failed_details: Vec<String>,
}

/// Run daily compaction over all pending days in `input_dir`.
///
/// A "pending day" is any date that has at least one `YYYY-MM-DD*` session
/// transcript in `input_dir` but no corresponding `.daily.md` in `output_dir`.
///
/// Progress is reported to stderr as `dailies: [N of M] YYYY-MM/YYYY-MM-DD... ok`.
/// Days that fail (empty LLM output or LLM error) are counted and reported at the end
/// but do not abort the remaining days.
pub fn run_dailies(
    input_dir: &Path,
    output_dir: &Path,
    limit: Option<usize>,
    model: &str,
    provider: &str,
    base_url: Option<&str>,
    options: &CompactOptions,
) -> Result<DailyRunReport, CassioError> {
    let pending = find_pending_days(input_dir, output_dir)?;

    if pending.is_empty() {
        eprintln!("No pending days to compact.");
        return Ok(DailyRunReport {
            compacted: 0,
            failed: 0,
            failed_details: Vec::new(),
        });
    }

    let to_process: Vec<_> = match limit {
        Some(n) => pending.into_iter().take(n).collect(),
        None => pending,
    };

    let total = to_process.len();
    let start = Instant::now();
    let mut compacted = 0usize;
    let mut failed = 0usize;
    let mut failed_details = Vec::new();

    for (i, (day, files)) in to_process.iter().enumerate() {
        let month = day.get(..7).unwrap_or("unknown");
        let relative = format!("{month}/{day}");
        eprint!("dailies: [{} of {total}] {relative}...", i + 1);

        match compact_day(
            output_dir, day, month, files, model, provider, base_url, options,
        ) {
            Ok(()) => {
                eprintln!(" ok");
                compacted += 1;
            }
            Err(err) => {
                eprintln!(" [FAIL]");
                failed += 1;
                failed_details.push(err.detail);
            }
        }
    }

    let elapsed = start.elapsed();
    eprintln!(
        "finished: {}, {compacted} compacted, {failed} failed",
        format_elapsed(elapsed)
    );
    Ok(DailyRunReport {
        compacted,
        failed,
        failed_details,
    })
}

/// Compact all daily summary files for a single month into a `.monthly.md` summary.
///
/// Reads every `.daily.md` from `<dir>/<month>/`, and either:
/// - Sends all content in a single LLM call (when total size ≤ `MAX_INPUT_BYTES`)
/// - Chunks the content, summarizes each chunk, then merges the chunk summaries
///
/// The output is written to `<dir>/<month>/<month>.monthly.md`. If that file
/// already exists, the function skips processing and returns `Ok(())`.
///
/// # Error behavior
///
/// Returns `Err` if the month directory is missing, there are no compaction files,
/// or the LLM returns empty output.
pub fn run_monthly(
    dir: &Path,
    month: &str,
    model: &str,
    provider: &str,
    base_url: Option<&str>,
) -> Result<(), CassioError> {
    let options = CompactOptions::default();

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

    // Collect daily summary files sorted by date
    let mut daily_files: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&month_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if is_daily_summary_name(&name) {
                daily_files.push(entry.path());
            }
        }
    }
    daily_files.sort();

    if daily_files.is_empty() {
        return Err(CassioError::Other(format!(
            "No .daily.md files found in {month}/"
        )));
    }

    eprintln!("monthly: {month} ({} daily files)", daily_files.len());

    let start = Instant::now();

    // Read all compaction contents
    let mut contents: Vec<(String, String)> = Vec::new(); // (filename, content)
    for path in &daily_files {
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
        let output = invoke_llm(
            &input,
            model,
            provider,
            base_url,
            &options,
            InvocationContext {
                checkpoint_dir: &month_dir,
                day: month,
                phase: "monthly_single",
                chunk_index: Some(1),
                total_chunks: 1,
            },
        )
        .map_err(|e| CassioError::Other(format!("monthly single-pass failed: {}", e.detail)))?;
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
            let days: Vec<&str> = chunk
                .iter()
                .map(|(n, _)| strip_daily_suffix(n).unwrap_or(n.as_str()))
                .collect();
            eprint!(
                "  chunk [{} of {}] {} to {}...",
                i + 1,
                chunks.len(),
                days.first().unwrap_or(&"?"),
                days.last().unwrap_or(&"?"),
            );

            let input = build_monthly_input(MONTHLY_PROMPT, month, chunk);
            let output = invoke_llm(
                &input,
                model,
                provider,
                base_url,
                &options,
                InvocationContext {
                    checkpoint_dir: &month_dir,
                    day: month,
                    phase: "monthly_chunk",
                    chunk_index: Some(i + 1),
                    total_chunks: chunks.len(),
                },
            )
            .map_err(|e| {
                CassioError::Other(format!("monthly chunk {} failed: {}", i + 1, e.detail))
            })?;

            if output.trim().is_empty() {
                eprintln!(" [FAIL]");
                return Err(CassioError::Other(format!(
                    "Ollama returned empty output for chunk {}",
                    i + 1
                )));
            }

            eprintln!(" ok");
            chunk_summaries.push((format!("chunk-{}", i + 1), output));
        }

        // Merge pass
        eprint!("  merging {} chunk summaries...", chunk_summaries.len());
        let merge_input = build_monthly_input(MONTHLY_MERGE_PROMPT, month, &chunk_summaries);
        let merged = invoke_llm(
            &merge_input,
            model,
            provider,
            base_url,
            &options,
            InvocationContext {
                checkpoint_dir: &month_dir,
                day: month,
                phase: "monthly_merge",
                chunk_index: None,
                total_chunks: chunk_summaries.len(),
            },
        )
        .map_err(|e| CassioError::Other(format!("monthly merge failed: {}", e.detail)))?;
        eprintln!(" ok");
        merged
    };

    if result.trim().is_empty() {
        eprintln!("monthly: [FAIL] empty output");
        return Err(CassioError::Other("Ollama returned empty output".into()));
    }

    std::fs::write(&output_path, &result)?;

    let elapsed = start.elapsed();
    eprintln!(
        "finished: {}, wrote {month}.monthly.md",
        format_elapsed(elapsed)
    );
    Ok(())
}

/// Discover and compact all months that have daily summaries but no monthly summary.
///
/// Scans `dir` for `YYYY-MM/` subdirectories that contain `.daily.md` files
/// but no `.monthly.md`, then calls `run_monthly` for each in chronological order.
pub fn run_pending_monthlies(
    dir: &Path,
    model: &str,
    provider: &str,
    base_url: Option<&str>,
) -> Result<(), CassioError> {
    let pending = find_pending_months(dir)?;

    if pending.is_empty() {
        eprintln!("No pending months to compact.");
        return Ok(());
    }

    eprintln!(
        "Found {} pending month(s): {}",
        pending.len(),
        pending.join(", ")
    );

    for month in &pending {
        run_monthly(dir, month, model, provider, base_url)?;
    }

    Ok(())
}

// ── Private helpers ───────────────────────────────────────────────────────────
//
// These functions implement the details of file discovery, content extraction,
// LLM invocation, and output assembly. They are private because all coordination
// is managed by the public run_* functions above.

/// Return all `YYYY-MM` months under `dir` that have daily summaries but no monthly.
///
/// WHY: Checking for `.daily.md` presence before adding a month to the pending
/// list means empty month directories (e.g., months with sessions but no daily summaries
/// yet) are skipped without producing an error.
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
            // Check if there are any daily summary files
            let has_dailies = std::fs::read_dir(&month_dir)
                .ok()
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .any(|e| is_daily_summary_name(&e.file_name().to_string_lossy()))
                })
                .unwrap_or(false);
            if has_dailies {
                months.push(name);
            }
        }
    }

    months.sort();
    Ok(months)
}

/// Group session transcript files by date and return those without a `.daily.md`.
///
/// WHY: Using `BTreeMap` ensures days are returned in chronological order without
/// a separate sort. The date prefix is extracted from the filename (first 10 chars
/// must be `YYYY-MM-DD`) rather than filesystem metadata to be portable.
fn find_pending_days(
    input_dir: &Path,
    output_dir: &Path,
) -> Result<Vec<(String, Vec<PathBuf>)>, CassioError> {
    let mut by_date: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();

    for entry in WalkDir::new(input_dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };

        if is_session_transcript_name(name) && name.len() >= 10 && name.is_char_boundary(10) {
            let date = &name[..10];
            if date.len() == 10 && date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-' {
                by_date
                    .entry(date.to_string())
                    .or_default()
                    .push(path.to_path_buf());
            }
        }
    }

    let mut pending = Vec::new();
    for (date, mut files) in by_date {
        if daily_summary_exists(output_dir, &date) {
            continue;
        }
        files.sort();
        pending.push((date, files));
    }

    Ok(pending)
}

/// Send one day's extracted session content to the LLM and write the result.
///
/// Returns `Ok(true)` when the LLM produced output, `Ok(false)` when it returned
/// empty output (indicating a soft failure), and `Err` for hard failures (I/O, LLM
/// process error).
fn compact_day(
    output_dir: &Path,
    day: &str,
    month: &str,
    files: &[PathBuf],
    model: &str,
    provider: &str,
    base_url: Option<&str>,
    options: &CompactOptions,
) -> Result<(), DayFailure> {
    let mut sessions = Vec::new();
    for file in files {
        let extracted = extract_session(file).map_err(|e| DayFailure {
            detail: format!("{day}: failed to extract {}: {e}", file.display()),
        })?;
        if !extracted.is_empty() {
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("session")
                .to_string();
            sessions.push((name, extracted));
        }
    }

    let out_dir = output_dir.join(month);
    std::fs::create_dir_all(&out_dir).map_err(|e| DayFailure {
        detail: format!("{day}: failed to create {}: {e}", out_dir.display()),
    })?;
    let checkpoint_dir = daily_checkpoint_dir(&out_dir, day);
    std::fs::create_dir_all(&checkpoint_dir).map_err(|e| DayFailure {
        detail: format!("{day}: failed to create {}: {e}", checkpoint_dir.display()),
    })?;
    let progress_path = progress_log_path(&checkpoint_dir);
    let plan = plan_daily_compaction(day, &sessions);
    let total_chunks = match &plan {
        DailyCompactionPlan::Single(_) => 1,
        DailyCompactionPlan::Chunked(chunk_inputs) => chunk_inputs.len(),
    };

    let output = match plan {
        DailyCompactionPlan::Single(input) => {
            write_day_status(
                &checkpoint_dir,
                day,
                "running",
                "single",
                provider,
                model,
                1,
                0,
                0,
                None,
                None,
            )
            .map_err(day_status_err(day))?;
            let context = InvocationContext {
                checkpoint_dir: &checkpoint_dir,
                day,
                phase: "single",
                chunk_index: Some(1),
                total_chunks: 1,
            };
            match invoke_llm(&input, model, provider, base_url, options, context) {
                Ok(output) => output,
                Err(err) => {
                    let _ = write_parse_failure_artifact(&checkpoint_dir, "single", Some(1), &err);
                    write_day_status(
                        &checkpoint_dir,
                        day,
                        "failed",
                        "single",
                        provider,
                        model,
                        1,
                        0,
                        0,
                        Some(err.class.as_str()),
                        Some(&err.detail),
                    )
                    .map_err(day_status_err(day))?;
                    return Err(DayFailure {
                        detail: format!(
                            "{day}: chunk 1 failed [{}] {}",
                            err.class.as_str(),
                            err.detail
                        ),
                    });
                }
            }
        }
        DailyCompactionPlan::Chunked(chunk_inputs) => {
            write_day_status(
                &checkpoint_dir,
                day,
                "running",
                "chunk",
                provider,
                model,
                chunk_inputs.len(),
                0,
                0,
                None,
                None,
            )
            .map_err(day_status_err(day))?;
            let mut chunk_summaries = Vec::new();
            for (i, chunk_input) in chunk_inputs.iter().enumerate() {
                eprint!(" chunk [{}/{}]...", i + 1, chunk_inputs.len());
                let checkpoint_path = daily_chunk_checkpoint_path(&checkpoint_dir, i);
                let output = if options.resume {
                    read_cached_chunk_summary(&checkpoint_path).map_err(|e| DayFailure {
                        detail: format!("{day}: failed to read cached chunk {}: {e}", i + 1),
                    })?
                } else {
                    None
                };
                let output = if let Some(cached) = output {
                    eprintln!(" cached");
                    let _ = append_progress_event(
                        &progress_path,
                        &ProgressEvent {
                            day,
                            phase: "chunk",
                            chunk_index: Some(i + 1),
                            total_chunks: chunk_inputs.len(),
                            retry_count: 0,
                            elapsed_ms: 0,
                            outcome: "cached",
                            failure_class: None,
                            detail: None,
                        },
                    );
                    cached
                } else {
                    let context = InvocationContext {
                        checkpoint_dir: &checkpoint_dir,
                        day,
                        phase: "chunk",
                        chunk_index: Some(i + 1),
                        total_chunks: chunk_inputs.len(),
                    };
                    let output = match invoke_llm(
                        chunk_input,
                        model,
                        provider,
                        base_url,
                        options,
                        context,
                    ) {
                        Ok(output) => output,
                        Err(err) => {
                            let _ = write_parse_failure_artifact(
                                &checkpoint_dir,
                                "chunk",
                                Some(i + 1),
                                &err,
                            );
                            write_day_status(
                                &checkpoint_dir,
                                day,
                                "failed",
                                "chunk",
                                provider,
                                model,
                                chunk_inputs.len(),
                                i,
                                0,
                                Some(err.class.as_str()),
                                Some(&err.detail),
                            )
                            .map_err(day_status_err(day))?;
                            eprintln!(" [FAIL: {}]", err.class.as_str());
                            return Err(DayFailure {
                                detail: format!(
                                    "{day}: chunk {} failed [{}] {}",
                                    i + 1,
                                    err.class.as_str(),
                                    err.detail
                                ),
                            });
                        }
                    };
                    if output.trim().is_empty() {
                        write_day_status(
                            &checkpoint_dir,
                            day,
                            "failed",
                            "chunk",
                            provider,
                            model,
                            chunk_inputs.len(),
                            i,
                            0,
                            Some(FailureClass::EmptyResponse.as_str()),
                            Some("provider returned empty chunk output"),
                        )
                        .map_err(day_status_err(day))?;
                        eprintln!(" [FAIL]");
                        return Err(DayFailure {
                            detail: format!("{day}: chunk {} failed [empty_response]", i + 1),
                        });
                    }
                    write_atomic(&checkpoint_path, &output).map_err(|e| DayFailure {
                        detail: format!(
                            "{day}: failed to write chunk {} checkpoint {}: {e}",
                            i + 1,
                            checkpoint_path.display()
                        ),
                    })?;
                    eprintln!(" ok");
                    output
                };
                if output.trim().is_empty() {
                    write_day_status(
                        &checkpoint_dir,
                        day,
                        "failed",
                        "chunk",
                        provider,
                        model,
                        chunk_inputs.len(),
                        i,
                        0,
                        Some(FailureClass::EmptyResponse.as_str()),
                        Some("checkpointed chunk output was empty"),
                    )
                    .map_err(day_status_err(day))?;
                    eprintln!(" [FAIL]");
                    return Err(DayFailure {
                        detail: format!(
                            "{day}: chunk {} failed [empty_response] cached output was empty",
                            i + 1
                        ),
                    });
                }
                chunk_summaries.push((format!("chunk-{}", i + 1), output));
                write_day_status(
                    &checkpoint_dir,
                    day,
                    "running",
                    "chunk",
                    provider,
                    model,
                    chunk_inputs.len(),
                    i + 1,
                    0,
                    None,
                    None,
                )
                .map_err(day_status_err(day))?;
            }

            let merge_input = build_daily_merge_input(day, &chunk_summaries);
            write_day_status(
                &checkpoint_dir,
                day,
                "running",
                "merge",
                provider,
                model,
                chunk_inputs.len(),
                chunk_inputs.len(),
                0,
                None,
                None,
            )
            .map_err(day_status_err(day))?;
            let context = InvocationContext {
                checkpoint_dir: &checkpoint_dir,
                day,
                phase: "merge",
                chunk_index: None,
                total_chunks: chunk_inputs.len(),
            };
            match invoke_llm(&merge_input, model, provider, base_url, options, context) {
                Ok(output) => output,
                Err(err) => {
                    let _ = write_parse_failure_artifact(&checkpoint_dir, "merge", None, &err);
                    write_day_status(
                        &checkpoint_dir,
                        day,
                        "failed",
                        "merge",
                        provider,
                        model,
                        chunk_inputs.len(),
                        chunk_inputs.len(),
                        0,
                        Some(err.class.as_str()),
                        Some(&err.detail),
                    )
                    .map_err(day_status_err(day))?;
                    return Err(DayFailure {
                        detail: format!(
                            "{day}: merge failed [{}] {}",
                            err.class.as_str(),
                            err.detail
                        ),
                    });
                }
            }
        }
    };

    if output.trim().is_empty() {
        write_day_status(
            &checkpoint_dir,
            day,
            "failed",
            "finalize",
            provider,
            model,
            total_chunks,
            0,
            0,
            Some(FailureClass::EmptyResponse.as_str()),
            Some("provider returned empty final output"),
        )
        .map_err(day_status_err(day))?;
        return Err(DayFailure {
            detail: format!("{day}: final output failed [empty_response]"),
        });
    }

    let out_path = out_dir.join(format!("{day}.daily.md"));
    write_atomic(&out_path, &output).map_err(|e| DayFailure {
        detail: format!("{day}: failed to write {}: {e}", out_path.display()),
    })?;
    write_day_status(
        &checkpoint_dir,
        day,
        "completed",
        "finalize",
        provider,
        model,
        total_chunks,
        total_chunks,
        total_chunks,
        None,
        None,
    )
    .map_err(day_status_err(day))?;
    let _ = append_progress_event(
        &progress_path,
        &ProgressEvent {
            day,
            phase: "finalize",
            chunk_index: None,
            total_chunks,
            retry_count: 0,
            elapsed_ms: 0,
            outcome: "completed",
            failure_class: None,
            detail: None,
        },
    );

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_day_status(
    checkpoint_dir: &Path,
    day: &str,
    status: &str,
    phase: &str,
    provider: &str,
    model: &str,
    total_chunks: usize,
    completed_chunks: usize,
    finalized_chunks: usize,
    failure_class: Option<&str>,
    failure_detail: Option<&str>,
) -> Result<(), CassioError> {
    let path = checkpoint_dir.join("status.json");
    let status = DailyCheckpointStatus {
        day,
        status,
        phase,
        provider,
        model,
        total_chunks,
        completed_chunks,
        finalized_chunks,
        failure_class,
        failure_detail,
    };
    let body = serde_json::to_string_pretty(&status)
        .map_err(|e| CassioError::Other(format!("Failed to serialize day status: {e}")))?;
    write_atomic(&path, &body)
}

fn plan_daily_compaction(day: &str, sessions: &[(String, String)]) -> DailyCompactionPlan {
    let prompt_overhead = COMPACT_PROMPT.len() + 100;
    let total_content_bytes: usize = sessions.iter().map(|(_, c)| c.len()).sum();
    let total_with_prompt = total_content_bytes + prompt_overhead;

    if total_with_prompt <= MAX_INPUT_BYTES {
        return DailyCompactionPlan::Single(build_daily_input(COMPACT_PROMPT, day, sessions));
    }

    let chunks = build_chunks(sessions, prompt_overhead);
    let chunk_inputs = chunks
        .iter()
        .map(|chunk| build_daily_input(COMPACT_PROMPT, day, chunk))
        .collect();
    DailyCompactionPlan::Chunked(chunk_inputs)
}

fn build_daily_input(prompt: &str, day: &str, sessions: &[(String, String)]) -> String {
    let mut input = String::new();
    input.push_str(prompt);
    input.push_str(&format!(
        "\n\n---BEGIN TRANSCRIPTS ({day}, {} sessions)---\n\n",
        sessions.len()
    ));

    for (name, content) in sessions {
        input.push_str(content);
        input.push_str(&format!("\n--- (end {name}) ---\n\n"));
    }

    input.push_str("---END TRANSCRIPTS---\n");
    input
}

fn build_daily_merge_input(day: &str, chunks: &[(String, String)]) -> String {
    let mut input = String::new();
    input.push_str(DAILY_MERGE_PROMPT);
    input.push_str(&format!(
        "\n\n---BEGIN DAILY PARTIALS ({day}, {} chunks)---\n\n",
        chunks.len()
    ));

    for (name, content) in chunks {
        input.push_str(content);
        input.push_str(&format!("\n--- (end {name}) ---\n\n"));
    }

    input.push_str(&format!("---END DAILY PARTIALS ({day})---\n"));
    input
}

fn daily_checkpoint_dir(out_dir: &Path, day: &str) -> PathBuf {
    out_dir.join(".cassio-checkpoints").join(day)
}

fn daily_chunk_checkpoint_path(checkpoint_dir: &Path, index: usize) -> PathBuf {
    checkpoint_dir.join(format!("chunk-{:04}.md", index + 1))
}

fn read_cached_chunk_summary(path: &Path) -> Result<Option<String>, CassioError> {
    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(path)?;
    if content.trim().is_empty() {
        return Ok(None);
    }

    Ok(Some(content))
}

fn write_atomic(path: &Path, content: &str) -> Result<(), CassioError> {
    let parent = path.parent().ok_or_else(|| {
        CassioError::Other(format!(
            "Cannot determine parent directory for {}",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent)?;

    let tmp_path = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("tmp")
    ));
    std::fs::write(&tmp_path, content)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

fn is_session_transcript_name(name: &str) -> bool {
    let stem = if let Some(stem) = name.strip_suffix(".md") {
        stem
    } else if let Some(stem) = name.strip_suffix(".txt") {
        stem
    } else {
        return false;
    };

    let tool = stem.rsplit('-').next().unwrap_or("");
    matches!(tool, "claude" | "codex" | "opencode" | "pi")
}

fn is_daily_summary_name(name: &str) -> bool {
    name.ends_with(".daily.md") || name.ends_with(".compaction.md")
}

fn strip_daily_suffix(name: &str) -> Option<&str> {
    name.strip_suffix(".daily.md")
        .or_else(|| name.strip_suffix(".compaction.md"))
}

fn daily_summary_exists(output_dir: &Path, date: &str) -> bool {
    let month = date.get(..7).unwrap_or("unknown");
    output_dir
        .join(month)
        .join(format!("{date}.daily.md"))
        .exists()
        || output_dir
            .join(month)
            .join(format!("{date}.compaction.md"))
            .exists()
}

/// Reduce a session transcript to the content most useful for LLM compaction.
///
/// The extraction strategy compresses assistant responses while preserving the
/// complete user-level conversation. Rules applied per line:
///
/// - Lines starting with 📋 (metadata) → always included
/// - Lines starting with 👤 (user message) → always included; resets assistant line count
/// - Lines starting with 🤖 (assistant message) → included; begins assistant line tracking
/// - Lines starting with ✅ / ❌ (tool calls) → excluded entirely
/// - Subsequent non-empty lines after 🤖 → included up to `LLM_LINE_LIMIT` (5) lines
///
/// WHY: Tool call details (file paths, command output) are not useful signal for
/// daily compaction — the LLM cares about what was discussed, not which files were
/// touched. Limiting assistant lines to 5 keeps prompts within token budgets while
/// capturing the intent of each response.
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
            // WHY: Tool call lines reset the assistant context — the next non-tool
            // content may be a continuation that we want to capture.
            in_llm = false;
        } else if in_llm && !line.trim().is_empty() {
            llm_lines += 1;
            if llm_lines <= LLM_LINE_LIMIT {
                out.push_str(line);
                out.push('\n');
            }
        }
    }

    Ok(out)
}

/// Assemble the full input string for a monthly (or chunk) LLM call.
///
/// Wraps the prompt and all compaction content in named delimiters so the LLM
/// can distinguish between the instruction and the data sections.
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

/// Split compaction content into chunks that each fit within the per-call byte budget.
///
/// Each returned chunk is a `Vec<(filename, content)>` pair list suitable for
/// passing to `build_monthly_input`. Chunks respect the budget on a best-effort
/// basis: a single file that exceeds the budget gets its own chunk rather than
/// being dropped.
///
/// TRADE-OFF: Chunking by byte count rather than token count is an approximation.
/// It works well in practice because the 4 bytes/token estimate is conservative for
/// English markdown content.
fn build_chunks(
    contents: &[(String, String)],
    prompt_overhead: usize,
) -> Vec<Vec<(String, String)>> {
    let budget = MAX_INPUT_BYTES.saturating_sub(prompt_overhead);
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

/// Dispatch to the appropriate LLM provider CLI and return its output.
///
/// All providers read the prompt from stdin. Ollama and Claude write output to
/// stdout; Codex uses a temporary file for output because its CLI does not support
/// stdout streaming in exec mode.
fn invoke_llm(
    input: &str,
    model: &str,
    provider: &str,
    base_url: Option<&str>,
    options: &CompactOptions,
    context: InvocationContext<'_>,
) -> Result<String, InvocationError> {
    let mut last_error = None;

    for attempt in 1..=options.max_retries {
        let started = Instant::now();
        match invoke_llm_once(input, model, provider, base_url, options.chunk_timeout) {
            Ok(output) => {
                let _ = append_progress_event(
                    &progress_log_path(context.checkpoint_dir),
                    &ProgressEvent {
                        day: context.day,
                        phase: context.phase,
                        chunk_index: context.chunk_index,
                        total_chunks: context.total_chunks,
                        retry_count: attempt - 1,
                        elapsed_ms: started.elapsed().as_millis(),
                        outcome: "success",
                        failure_class: None,
                        detail: None,
                    },
                );
                return Ok(output);
            }
            Err(err) => {
                let retryable = err.class.retryable() && attempt < options.max_retries;
                let _ = append_progress_event(
                    &progress_log_path(context.checkpoint_dir),
                    &ProgressEvent {
                        day: context.day,
                        phase: context.phase,
                        chunk_index: context.chunk_index,
                        total_chunks: context.total_chunks,
                        retry_count: attempt - 1,
                        elapsed_ms: started.elapsed().as_millis(),
                        outcome: if retryable { "retry" } else { "failure" },
                        failure_class: Some(err.class.as_str()),
                        detail: Some(&err.detail),
                    },
                );
                eprintln!(
                    " llm: attempt {attempt}/{} failed [{}]: {}",
                    options.max_retries,
                    err.class.as_str(),
                    err.detail
                );
                last_error = Some(err);
                if retryable {
                    let backoff = RETRY_BACKOFF_SECS
                        .get(attempt - 1)
                        .copied()
                        .unwrap_or(*RETRY_BACKOFF_SECS.last().unwrap_or(&5));
                    eprintln!(" llm: retrying in {backoff}s");
                    thread::sleep(Duration::from_secs(backoff));
                    continue;
                }
                break;
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        InvocationError::new(FailureClass::Io, "llm invocation failed without detail")
    }))
}

fn invoke_llm_once(
    input: &str,
    model: &str,
    provider: &str,
    base_url: Option<&str>,
    chunk_timeout: Duration,
) -> Result<String, InvocationError> {
    match provider {
        "ollama" => invoke_stdio("ollama", &["run", model], input, chunk_timeout),
        "claude" => invoke_stdio("claude", &["-p", "--model", model], input, chunk_timeout),
        "codex" => invoke_codex(input, model, chunk_timeout),
        "openrouter" => invoke_openrouter(input, model, chunk_timeout),
        "openai" => invoke_openai_compatible(input, model, base_url, chunk_timeout),
        _ => Err(InvocationError::new(
            FailureClass::Io,
            format!(
                "Unknown provider: {provider} (supported: ollama, claude, codex, openrouter, openai)"
            ),
        )),
    }
}

/// Spawn a CLI process, write `input` to its stdin, and capture stdout.
///
/// Stderr is suppressed to avoid LLM progress output (e.g., Ollama token streaming)
/// mixing with cassio's own progress reporting.
fn invoke_stdio(
    cmd: &str,
    args: &[&str],
    input: &str,
    chunk_timeout: Duration,
) -> Result<String, InvocationError> {
    let mut child = std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            InvocationError::new(FailureClass::Process, format!("Failed to start {cmd}: {e}"))
        })?;

    if let Some(ref mut stdin) = child.stdin {
        stdin.write_all(input.as_bytes()).map_err(|e| {
            InvocationError::new(
                FailureClass::Io,
                format!("Failed to write to {cmd} stdin: {e}"),
            )
        })?;
    }
    drop(child.stdin.take());

    let output = wait_with_timeout(child, cmd, chunk_timeout)?;

    if !output.status.success() {
        let stderr = truncate_for_error(&String::from_utf8_lossy(&output.stderr));
        return Err(InvocationError::new(
            FailureClass::Process,
            format!("{cmd} exited with status {} ({stderr})", output.status),
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Invoke Codex in exec mode, capturing its output via a temporary file.
///
/// WHY: `codex exec` does not write structured output to stdout — it uses a separate
/// `-o` output file flag. The temp file is cleaned up on both success and failure.
///
/// EDGE: If the process succeeds but the output file is missing or unreadable,
/// this returns an error. The temp file name includes the process ID to avoid
/// collisions when multiple cassio instances run simultaneously.
fn invoke_codex(
    input: &str,
    model: &str,
    chunk_timeout: Duration,
) -> Result<String, InvocationError> {
    let tmp = std::env::temp_dir().join(format!("cassio-codex-{}.md", std::process::id()));
    let tmp_str = tmp.to_string_lossy().to_string();

    let mut child = std::process::Command::new("codex")
        .args(["exec", "-m", model, "-o", &tmp_str])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            InvocationError::new(FailureClass::Process, format!("Failed to start codex: {e}"))
        })?;

    if let Some(ref mut stdin) = child.stdin {
        stdin.write_all(input.as_bytes()).map_err(|e| {
            InvocationError::new(
                FailureClass::Io,
                format!("Failed to write to codex stdin: {e}"),
            )
        })?;
    }
    drop(child.stdin.take());

    let output = wait_with_timeout(child, "codex", chunk_timeout)?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        let stderr = truncate_for_error(&String::from_utf8_lossy(&output.stderr));
        return Err(InvocationError::new(
            FailureClass::Process,
            format!("codex exited with status {} ({stderr})", output.status),
        ));
    }

    let result = std::fs::read_to_string(&tmp).map_err(|e| {
        InvocationError::new(
            FailureClass::Io,
            format!("Failed to read codex output file {}: {e}", tmp.display()),
        )
    })?;
    let _ = std::fs::remove_file(&tmp);

    Ok(result)
}

/// Call the OpenRouter chat completions API.
///
/// Reads `OPENROUTER_API_KEY` from the environment. The model parameter is passed
/// directly (e.g., `anthropic/claude-sonnet-4`, `google/gemini-2.5-pro`).
fn invoke_openrouter(
    input: &str,
    model: &str,
    chunk_timeout: Duration,
) -> Result<String, InvocationError> {
    let api_key = std::env::var("OPENROUTER_API_KEY").map_err(|_| {
        InvocationError::new(
            FailureClass::Io,
            "OPENROUTER_API_KEY environment variable not set",
        )
    })?;

    invoke_chat_completions(
        "OpenRouter",
        "https://openrouter.ai/api/v1/chat/completions",
        Some(&api_key),
        input,
        model,
        chunk_timeout,
    )
}

/// Call a local or self-hosted OpenAI-compatible chat-completions endpoint.
///
/// `base_url` may be either a base URL such as `http://127.0.0.1:18173/v1` or
/// the full `/chat/completions` endpoint. If `OPENAI_API_KEY` is set, cassio
/// sends it as a bearer token; local llama.cpp servers typically do not require one.
fn invoke_openai_compatible(
    input: &str,
    model: &str,
    base_url: Option<&str>,
    chunk_timeout: Duration,
) -> Result<String, InvocationError> {
    let base_url = base_url.ok_or_else(|| {
        InvocationError::new(
            FailureClass::Io,
            "provider=openai requires base_url (set `base_url` in config or pass `--base-url`)",
        )
    })?;
    let endpoint = chat_completions_endpoint(base_url);
    let api_key = std::env::var("OPENAI_API_KEY").ok();
    invoke_chat_completions(
        "OpenAI-compatible provider",
        &endpoint,
        api_key.as_deref(),
        input,
        model,
        chunk_timeout,
    )
}

fn chat_completions_endpoint(provider: &str) -> String {
    let trimmed = provider.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

fn invoke_chat_completions(
    label: &str,
    endpoint: &str,
    bearer_token: Option<&str>,
    input: &str,
    model: &str,
    chunk_timeout: Duration,
) -> Result<String, InvocationError> {
    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "user", "content": input }
        ]
    });

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(chunk_timeout))
        .timeout_per_call(Some(chunk_timeout))
        .build()
        .into();

    let mut request = agent
        .post(endpoint)
        .header("Content-Type", "application/json");
    let auth_header;
    if let Some(token) = bearer_token {
        auth_header = format!("Bearer {token}");
        request = request.header("Authorization", &auth_header);
    }

    let raw_response = request
        .send_json(&body)
        .map_err(|err| map_chat_completions_error(label, err, chunk_timeout))?
        .body_mut()
        .read_to_string()
        .map_err(|e| {
            InvocationError::new(
                FailureClass::Transport,
                format!("{label} response read failed: {e}"),
            )
        })?;

    let response: serde_json::Value = serde_json::from_str(&raw_response).map_err(|e| {
        InvocationError::new(
            FailureClass::ParseError,
            format!("{label} response parse failed: {e}"),
        )
        .with_raw_response_body(raw_response.clone())
    })?;

    // Extract the assistant message content from the chat completions response.
    let content = response
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            InvocationError::new(
                FailureClass::ParseError,
                format!(
                    "{label} response missing choices[0].message.content: {}",
                    truncate_for_error(&serde_json::to_string(&response).unwrap_or_default())
                ),
            )
            .with_raw_response_body(raw_response.clone())
        })?;

    if content.is_empty() {
        return Err(InvocationError::new(
            FailureClass::EmptyResponse,
            format!(
                "{label} returned empty response: {}",
                truncate_for_error(&serde_json::to_string(&response).unwrap_or_default())
            ),
        ));
    }

    Ok(content)
}

fn wait_with_timeout(
    mut child: Child,
    cmd: &str,
    chunk_timeout: Duration,
) -> Result<Output, InvocationError> {
    match child.wait_timeout(chunk_timeout).map_err(|e| {
        InvocationError::new(FailureClass::Io, format!("Failed to wait for {cmd}: {e}"))
    })? {
        Some(_) => child.wait_with_output().map_err(|e| {
            InvocationError::new(
                FailureClass::Io,
                format!("Failed to read {cmd} output: {e}"),
            )
        }),
        None => {
            let _ = child.kill();
            let _ = child.wait();
            Err(InvocationError::new(
                FailureClass::Timeout,
                format!("{cmd} exceeded {}s timeout", chunk_timeout.as_secs()),
            ))
        }
    }
}

fn map_chat_completions_error(
    label: &str,
    err: ureq::Error,
    chunk_timeout: Duration,
) -> InvocationError {
    match err {
        ureq::Error::Timeout(_) => InvocationError::new(
            FailureClass::Timeout,
            format!(
                "{label} request timed out after {}s",
                chunk_timeout.as_secs()
            ),
        ),
        ureq::Error::StatusCode(code) => InvocationError::new(
            FailureClass::ProviderHttpError,
            format!("{label} returned HTTP {code}"),
        ),
        other => InvocationError::new(
            FailureClass::Transport,
            format!("{label} request failed: {other}"),
        ),
    }
}

fn truncate_for_error(input: &str) -> String {
    const LIMIT: usize = 240;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return "no stderr".to_string();
    }
    if trimmed.len() <= LIMIT {
        return trimmed.to_string();
    }
    format!("{}...", &trimmed[..LIMIT])
}

fn append_progress_event(path: &Path, event: &ProgressEvent<'_>) -> Result<(), CassioError> {
    let parent = path.parent().ok_or_else(|| {
        CassioError::Other(format!(
            "Cannot determine parent directory for {}",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(event)
        .map_err(|e| CassioError::Other(format!("Failed to serialize progress event: {e}")))?;
    writeln!(file, "{line}")?;
    Ok(())
}

fn progress_log_path(checkpoint_dir: &Path) -> PathBuf {
    checkpoint_dir.join("progress.jsonl")
}

fn write_parse_failure_artifact(
    checkpoint_dir: &Path,
    phase: &str,
    chunk_index: Option<usize>,
    err: &InvocationError,
) -> Result<(), CassioError> {
    let Some(raw) = &err.raw_response_body else {
        return Ok(());
    };
    let suffix = chunk_index
        .map(|index| format!("chunk-{index:04}"))
        .unwrap_or_else(|| "merge".to_string());
    let path = checkpoint_dir.join(format!("{phase}-{suffix}.raw.txt"));
    write_atomic(&path, raw)
}

fn day_status_err(day: &str) -> impl FnOnce(CassioError) -> DayFailure + '_ {
    move |e| DayFailure {
        detail: format!("{day}: failed to write checkpoint status: {e}"),
    }
}

/// Format a duration as `Xm YYs` (for ≥60s) or `Xs`.
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
    fn test_build_daily_input_structure() {
        let sessions = vec![
            ("s1.md".to_string(), "session one".to_string()),
            ("s2.md".to_string(), "session two".to_string()),
        ];
        let result = build_daily_input("PROMPT", "2025-01-15", &sessions);
        assert!(result.starts_with("PROMPT"));
        assert!(result.contains("2025-01-15"));
        assert!(result.contains("2 sessions"));
        assert!(result.contains("session one"));
        assert!(result.contains("session two"));
        assert!(result.contains("---BEGIN TRANSCRIPTS"));
        assert!(result.contains("---END TRANSCRIPTS---"));
    }

    #[test]
    fn test_plan_daily_compaction_single() {
        let sessions = vec![("a.md".to_string(), "small content".to_string())];
        let plan = plan_daily_compaction("2025-01-15", &sessions);
        match plan {
            DailyCompactionPlan::Single(input) => {
                assert!(input.contains("2025-01-15"));
                assert!(input.contains("small content"));
            }
            DailyCompactionPlan::Chunked(_) => panic!("expected single-pass plan"),
        }
    }

    #[test]
    fn test_plan_daily_compaction_chunked() {
        let big = "x".repeat(100_000);
        let sessions = vec![("a.md".to_string(), big.clone()), ("b.md".to_string(), big)];
        let plan = plan_daily_compaction("2025-01-15", &sessions);
        match plan {
            DailyCompactionPlan::Chunked(chunk_inputs) => {
                assert!(chunk_inputs.len() >= 2);
                assert!(chunk_inputs[0].contains("2025-01-15"));
            }
            DailyCompactionPlan::Single(_) => panic!("expected chunked plan"),
        }
    }

    #[test]
    fn test_build_daily_merge_input_structure() {
        let chunks = vec![
            ("chunk-1".to_string(), "partial one".to_string()),
            ("chunk-2".to_string(), "partial two".to_string()),
        ];
        let result = build_daily_merge_input("2025-01-15", &chunks);
        assert!(result.starts_with(DAILY_MERGE_PROMPT));
        assert!(result.contains("2025-01-15"));
        assert!(result.contains("2 chunks"));
        assert!(result.contains("partial one"));
        assert!(result.contains("partial two"));
        assert!(result.contains("---BEGIN DAILY PARTIALS"));
        assert!(result.contains("---END DAILY PARTIALS (2025-01-15)---"));
    }

    #[test]
    fn test_build_chunks_single_chunk() {
        let contents = vec![("a".to_string(), "small content".to_string())];
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
        let path = dir.join("test.md");
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

    #[test]
    fn test_daily_chunk_checkpoint_path_format() {
        let dir = Path::new("/tmp/checkpoints");
        let path = daily_chunk_checkpoint_path(dir, 0);
        assert_eq!(path, Path::new("/tmp/checkpoints/chunk-0001.md"));
    }

    #[test]
    fn test_read_cached_chunk_summary_missing_and_empty() {
        let dir =
            std::env::temp_dir().join(format!("cassio_test_chunk_cache_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let missing = dir.join("missing.md");
        assert!(read_cached_chunk_summary(&missing).unwrap().is_none());

        let empty = dir.join("empty.md");
        std::fs::write(&empty, "   \n").unwrap();
        assert!(read_cached_chunk_summary(&empty).unwrap().is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_read_cached_chunk_summary_returns_content() {
        let dir = std::env::temp_dir().join(format!(
            "cassio_test_chunk_cache_hit_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("chunk-0001.md");
        std::fs::write(&path, "saved partial").unwrap();

        assert_eq!(
            read_cached_chunk_summary(&path).unwrap().as_deref(),
            Some("saved partial")
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_chat_completions_endpoint_normalizes_base_url() {
        assert_eq!(
            chat_completions_endpoint("http://127.0.0.1:18173/v1"),
            "http://127.0.0.1:18173/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_endpoint("http://127.0.0.1:18173/v1/"),
            "http://127.0.0.1:18173/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_endpoint("http://127.0.0.1:18173/v1/chat/completions"),
            "http://127.0.0.1:18173/v1/chat/completions"
        );
    }

    #[test]
    fn test_openai_provider_requires_base_url() {
        let err = invoke_llm_once(
            "hello",
            "local",
            "openai",
            None,
            std::time::Duration::from_secs(1),
        )
        .unwrap_err();
        assert_eq!(err.class, FailureClass::Io);
        assert!(err.detail.contains("requires base_url"));
    }
}
