//! Keyword, regex, and semantic search over cassio transcript artifacts.
//!
//! Text search walks monthly summaries, daily compactions, session transcripts,
//! and optional training JSON. Results are newest-first by default so unconstrained
//! searches surface recent material instead of the oldest months in the archive.
//! Filters narrow the walk by date range (`--from`/`--to`), agent (`--tool`),
//! project header (`--project`), and speaker role (`--speaker`). Semantic search
//! reuses the SQLite index built by `cassio index` and ranks chunks by cosine
//! similarity to the query embedding.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use regex::RegexBuilder;
use rusqlite::{Connection, params};
use serde::Serialize;
use walkdir::WalkDir;

use crate::ast::{SESSION_TOOL_SUFFIXES, session_tool_suffix};
use crate::error::CassioError;
use crate::formatter::emoji_text::{
    EMOJI_ASSISTANT, EMOJI_FAILURE, EMOJI_META, EMOJI_QUEUE, EMOJI_SUCCESS, EMOJI_USER,
};
use crate::index;

#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Earliest date to search (`YYYY-MM` or `YYYY-MM-DD`), inclusive.
    pub from: Option<String>,
    /// Latest date to search (`YYYY-MM` or `YYYY-MM-DD`), inclusive.
    pub to: Option<String>,
    /// Restrict to sessions from one agent (`codex`, `grok`, `pi`, ...).
    pub tool: Option<String>,
    /// Restrict to sessions whose `📋 Project:` header contains this substring.
    pub project: Option<String>,
    /// Match only lines spoken by this role in session transcripts.
    pub speaker: Option<Speaker>,
    pub limit: usize,
    pub summaries_only: bool,
    pub include_training: bool,
    pub include_paths: bool,
    pub json: bool,
    pub regex: bool,
    pub case_sensitive: bool,
    /// Context lines to show around each match (0 = none).
    pub context: usize,
    pub files_with_matches: bool,
    pub count: bool,
    /// Walk oldest files first (default is newest first).
    pub oldest_first: bool,
    pub semantic: Option<SemanticSearchOptions>,
    /// Separate root for `*.training.json` when not co-located under `root`.
    pub training_root: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    User,
    Assistant,
    Tool,
}

impl std::str::FromStr for Speaker {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            "tool" => Ok(Self::Tool),
            other => Err(format!(
                "invalid speaker '{other}' (expected user, assistant, or tool)"
            )),
        }
    }
}

impl Speaker {
    fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }

    fn line_kind(self) -> LineSpeaker {
        match self {
            Self::User => LineSpeaker::User,
            Self::Assistant => LineSpeaker::Assistant,
            Self::Tool => LineSpeaker::Tool,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SemanticSearchOptions {
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchArtifact {
    Monthly,
    Daily,
    Session,
    Training,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub artifact: SearchArtifact,
    pub path: PathBuf,
    pub line: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_end: Option<usize>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Vec<SearchContextLine>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchContextLine {
    pub line: usize,
    pub text: String,
}

/// Inclusive `--from`/`--to` bounds, each `YYYY-MM` or `YYYY-MM-DD`.
///
/// Month bounds constrain every artifact via its `YYYY-MM/` directory. Day bounds
/// additionally constrain artifacts that carry a `YYYY-MM-DD` date prefix
/// (sessions, dailies, training). `YYYY-MM` and `YYYY-MM-DD` compare correctly
/// as plain strings.
#[derive(Debug, Clone, Default)]
struct DateBounds {
    from: Option<Bound>,
    to: Option<Bound>,
}

#[derive(Debug, Clone)]
struct Bound {
    /// `YYYY-MM`
    month: String,
    /// `YYYY-MM-DD`, when the caller gave a full date.
    day: Option<String>,
}

impl Bound {
    fn parse(s: &str) -> Result<Self, CassioError> {
        if is_month(s) {
            return Ok(Self {
                month: s.to_string(),
                day: None,
            });
        }
        if is_day(s) {
            return Ok(Self {
                month: s[..7].to_string(),
                day: Some(s.to_string()),
            });
        }
        Err(CassioError::Other(format!(
            "Invalid date '{s}' (expected YYYY-MM or YYYY-MM-DD)"
        )))
    }
}

impl DateBounds {
    fn parse(from: Option<&str>, to: Option<&str>) -> Result<Self, CassioError> {
        Ok(Self {
            from: from.map(Bound::parse).transpose()?,
            to: to.map(Bound::parse).transpose()?,
        })
    }

    /// Whether a `YYYY-MM` directory falls inside the month bounds.
    fn month_in_range(&self, month_dir: &str) -> bool {
        if let Some(from) = &self.from
            && month_dir < from.month.as_str()
        {
            return false;
        }
        if let Some(to) = &self.to
            && month_dir > to.month.as_str()
        {
            return false;
        }
        true
    }

    fn contains(&self, month_dir: &str, date: Option<&str>) -> bool {
        if !self.month_in_range(month_dir) {
            return false;
        }
        if let Some(from) = &self.from
            && let (Some(from_day), Some(date)) = (&from.day, date)
            && date < from_day.as_str()
        {
            return false;
        }
        if let Some(to) = &self.to
            && let (Some(to_day), Some(date)) = (&to.day, date)
            && date > to_day.as_str()
        {
            return false;
        }
        true
    }
}

fn is_month(s: &str) -> bool {
    s.len() == 7
        && s.as_bytes()[4] == b'-'
        && chrono::NaiveDate::parse_from_str(&format!("{s}-01"), "%Y-%m-%d").is_ok()
}

fn is_day(s: &str) -> bool {
    s.len() == 10 && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

#[derive(Debug)]
enum Matcher {
    Terms {
        terms: Vec<String>,
        case_sensitive: bool,
    },
    Regex(regex::Regex),
}

impl Matcher {
    fn new(query: &str, regex: bool, case_sensitive: bool) -> Result<Self, CassioError> {
        if regex {
            return RegexBuilder::new(query)
                .case_insensitive(!case_sensitive)
                .build()
                .map(Self::Regex)
                .map_err(|e| CassioError::Other(format!("Invalid search regex: {e}")));
        }

        let terms: Vec<_> = if case_sensitive {
            query.split_whitespace().map(str::to_string).collect()
        } else {
            query.split_whitespace().map(normalize_term).collect()
        };
        if terms.is_empty() {
            return Err(CassioError::Other("Search query cannot be empty".into()));
        }

        Ok(Self::Terms {
            terms,
            case_sensitive,
        })
    }

    fn is_match(&self, line: &str) -> bool {
        match self {
            Self::Terms {
                terms,
                case_sensitive,
            } => {
                let haystack = if *case_sensitive {
                    line.to_string()
                } else {
                    line.to_lowercase()
                };
                terms.iter().all(|term| haystack.contains(term))
            }
            Self::Regex(regex) => regex.is_match(line),
        }
    }
}

pub fn run_search(root: &Path, query: &str, options: SearchOptions) -> Result<(), CassioError> {
    let hits = search(root, query, &options)?;

    if options.json {
        if options.count {
            serde_json::to_writer_pretty(
                std::io::stdout(),
                &count_entries(root, &hits, options.limit),
            )?;
        } else if options.files_with_matches {
            serde_json::to_writer_pretty(
                std::io::stdout(),
                &matching_file_paths(root, &hits, options.limit),
            )?;
        } else {
            serde_json::to_writer_pretty(std::io::stdout(), &hits)?;
        }
        println!();
    } else {
        print_hits(root, query, &options, &hits);
    }

    Ok(())
}

pub fn search(
    root: &Path,
    query: &str,
    options: &SearchOptions,
) -> Result<Vec<SearchHit>, CassioError> {
    if options.limit == 0 {
        return Ok(Vec::new());
    }

    validate_options(options)?;
    let bounds = DateBounds::parse(options.from.as_deref(), options.to.as_deref())?;

    if options.semantic.is_some() {
        return semantic_search(root, query, options, &bounds);
    }

    // `--from X --to X` with a month-only X resolves to that month directory so a
    // missing month still produces the friendly "Search target does not exist"
    // error instead of an empty walk of the root.
    let single_month = options
        .from
        .as_deref()
        .filter(|f| options.to.as_deref() == Some(f) && is_month(f));
    let target = match single_month {
        Some(month) => root.join(month),
        None => root.to_path_buf(),
    };
    if !target.exists() {
        return Err(CassioError::Other(format!(
            "Search target does not exist: {}",
            target.display()
        )));
    }
    // Inside the single-month directory the bounds are already satisfied.
    let walk_bounds = if single_month.is_some() {
        DateBounds::default()
    } else {
        bounds
    };

    let matcher = Matcher::new(query, options.regex, options.case_sensitive)?;
    let mut hits = Vec::new();
    // --count and --files-with-matches report on every file, so the per-file
    // limit early-exit must not apply; the file list is capped at output time.
    let scan_all = options.count || options.files_with_matches;

    for artifact in artifact_order(options) {
        for path in files_for_artifact_with_options(root, &target, artifact, options, &walk_bounds)
        {
            search_file(&path, artifact, &matcher, options, &mut hits)?;
            if !scan_all && hits.len() >= options.limit {
                return Ok(hits);
            }
        }
    }

    Ok(hits)
}

fn validate_options(options: &SearchOptions) -> Result<(), CassioError> {
    if options.count && options.files_with_matches {
        return Err(CassioError::Other(
            "--count and --files-with-matches cannot be combined".into(),
        ));
    }
    if options.regex && options.semantic.is_some() {
        return Err(CassioError::Other(
            "--semantic cannot be combined with --regex".into(),
        ));
    }
    if let Some(tool) = options.tool.as_deref()
        && !SESSION_TOOL_SUFFIXES
            .iter()
            .any(|known| known.eq_ignore_ascii_case(tool))
    {
        return Err(CassioError::Other(format!(
            "Unknown tool '{tool}' (expected one of: {})",
            SESSION_TOOL_SUFFIXES.join(", ")
        )));
    }
    if options.tool.is_some() && options.summaries_only {
        return Err(CassioError::Other(
            "--tool cannot be combined with --summaries-only (summaries aggregate all tools)"
                .into(),
        ));
    }
    if options.project.is_some() {
        if options.summaries_only {
            return Err(CassioError::Other(
                "--project cannot be combined with --summaries-only (summaries aggregate projects)"
                    .into(),
            ));
        }
        if options.include_training {
            return Err(CassioError::Other(
                "--project cannot be combined with --include-training".into(),
            ));
        }
        if options.semantic.is_some() {
            return Err(CassioError::Other(
                "--project cannot be combined with --semantic (index chunks carry no project)"
                    .into(),
            ));
        }
    }
    if options.speaker.is_some() {
        if options.summaries_only {
            return Err(CassioError::Other(
                "--speaker cannot be combined with --summaries-only (summaries have no speaker lines)"
                    .into(),
            ));
        }
        if options.include_training {
            return Err(CassioError::Other(
                "--speaker cannot be combined with --include-training (training JSON has no speaker lines)"
                    .into(),
            ));
        }
        if options.semantic.is_some() {
            return Err(CassioError::Other(
                "--speaker cannot be combined with --semantic (index chunks carry no speaker)"
                    .into(),
            ));
        }
    }
    Ok(())
}

fn artifact_order(options: &SearchOptions) -> Vec<SearchArtifact> {
    let mut order = vec![SearchArtifact::Monthly, SearchArtifact::Daily];
    if !options.summaries_only {
        order.push(SearchArtifact::Session);
        if options.include_training {
            order.push(SearchArtifact::Training);
        }
    }
    order
}

fn files_for_artifact_with_options(
    root: &Path,
    target: &Path,
    artifact: SearchArtifact,
    options: &SearchOptions,
    bounds: &DateBounds,
) -> Vec<PathBuf> {
    let single_month = options
        .from
        .as_deref()
        .filter(|f| options.to.as_deref() == Some(f) && is_month(f));

    let base = if artifact == SearchArtifact::Training {
        if let Some(training_root) = &options.training_root {
            let training_target = match single_month {
                Some(month) => training_root.join(month),
                None => training_root.clone(),
            };
            if training_target.exists() {
                training_target
            } else {
                // Fall through to co-located training under the transcript tree
                // when the dedicated training root (or month slice) is missing.
                target.to_path_buf()
            }
        } else {
            target.to_path_buf()
        }
    } else {
        // Keep non-training walks on the transcript tree only so a separate
        // training_root is never scanned for markdown.
        let _ = root;
        target.to_path_buf()
    };

    let mut paths = files_for_artifact(&base, artifact, bounds);
    paths.retain(|path| artifact_in_walk_scope(path, &base, artifact, options, bounds));
    if options.oldest_first {
        paths.sort();
    } else {
        // Newest first: `YYYY-MM/YYYY-MM-DD...` prefixes sort descending
        // chronologically, so unconstrained searches surface recent material.
        paths.sort_by(|a, b| b.cmp(a));
    }
    paths
}

fn files_for_artifact(root: &Path, artifact: SearchArtifact, bounds: &DateBounds) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    // Prune out-of-range `YYYY-MM` directories during the walk so a range
    // search over a multi-year archive never descends into every month.
    let walker = WalkDir::new(root).into_iter().filter_entry(|entry| {
        if entry.file_type().is_dir()
            && let Some(name) = entry.file_name().to_str()
            && is_month(name)
        {
            return bounds.month_in_range(name);
        }
        true
    });
    for entry in walker.filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if artifact_for_path(path) == Some(artifact) {
            paths.push(path.to_path_buf());
        }
    }
    paths
}

/// File-level gate for the lexical walk: date bounds, `--tool`, `--project`.
fn artifact_in_walk_scope(
    path: &Path,
    base: &Path,
    artifact: SearchArtifact,
    options: &SearchOptions,
    bounds: &DateBounds,
) -> bool {
    // Monthly/daily summaries aggregate every tool, project, and speaker, so
    // where/what filters cannot apply to them. Training JSON has no speaker
    // lines and project filtering on it is not implemented.
    if matches!(artifact, SearchArtifact::Monthly | SearchArtifact::Daily)
        && (options.tool.is_some() || options.project.is_some() || options.speaker.is_some())
    {
        return false;
    }
    if artifact == SearchArtifact::Training
        && (options.project.is_some() || options.speaker.is_some())
    {
        return false;
    }

    if options.tool.is_some()
        && !matches!(artifact, SearchArtifact::Session | SearchArtifact::Training)
    {
        return false;
    }
    if let Some(tool) = options.tool.as_deref() {
        let stem = file_stem(path);
        if !session_tool_suffix(&stem).is_some_and(|suffix| suffix.eq_ignore_ascii_case(tool)) {
            return false;
        }
    }

    if let Some(project) = options.project.as_deref()
        && !project_header_matches(path, project)
    {
        return false;
    }

    match month_dir_of(path, base) {
        Some(month) => {
            if !bounds.contains(&month, file_date_of(path).as_deref()) {
                return false;
            }
        }
        None => {
            // Root-level files (e.g. compact-prompt.md) carry no month; keep them
            // only when no date bounds are active — they are not artifacts anyway.
            if bounds.from.is_some() || bounds.to.is_some() {
                return false;
            }
        }
    }
    true
}

fn file_stem(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| {
            n.strip_suffix(".md")
                .or_else(|| n.strip_suffix(".txt"))
                .or_else(|| n.strip_suffix(".json"))
                .unwrap_or(n)
        })
        .unwrap_or_default()
        .to_string()
}

/// `YYYY-MM` directory a file lives under, relative to its walk base.
fn month_dir_of(path: &Path, base: &Path) -> Option<String> {
    let rel = path.strip_prefix(base).ok()?;
    let first = rel.components().next()?.as_os_str().to_str()?;
    (is_month(first)).then(|| first.to_string())
}

/// `YYYY-MM-DD` prefix of a session/daily/training filename, if any.
fn file_date_of(path: &Path) -> Option<String> {
    let stem = file_stem(path);
    let date = stem.get(..10)?;
    (is_day(date)).then(|| date.to_string())
}

/// Substring (case-insensitive) match against the session's `📋 Project:` header.
///
/// The metadata block sits at the top of a transcript, so a bounded read keeps
/// project filtering cheap without loading whole sessions.
fn project_header_matches(path: &Path, needle: &str) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut reader = std::io::BufReader::new(file);
    let mut header = String::new();
    let _ = std::io::Read::by_ref(&mut reader)
        .take(8192)
        .read_to_string(&mut header);
    let needle = needle.to_lowercase();
    header.lines().any(|line| {
        line.starts_with(EMOJI_META)
            && line.contains("Project:")
            && line.to_lowercase().contains(&needle)
    })
}

pub(crate) fn artifact_for_path(path: &Path) -> Option<SearchArtifact> {
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".monthly.md") {
        Some(SearchArtifact::Monthly)
    } else if name.ends_with(".daily.md") || name.ends_with(".compaction.md") {
        Some(SearchArtifact::Daily)
    } else if name.ends_with(".training.json") {
        Some(SearchArtifact::Training)
    } else if is_session_markdown_name(name) {
        Some(SearchArtifact::Session)
    } else {
        None
    }
}

fn is_session_markdown_name(name: &str) -> bool {
    let stem = name
        .strip_suffix(".md")
        .or_else(|| name.strip_suffix(".txt"));
    let Some(stem) = stem else {
        return false;
    };

    if stem.starts_with("unknown-") {
        return true;
    }

    stem.len() >= 20
        && stem.as_bytes().get(4) == Some(&b'-')
        && stem.as_bytes().get(7) == Some(&b'-')
        && stem.as_bytes().get(10) == Some(&b'T')
}

/// The speaker a transcript line belongs to, per the emoji prefix.
///
/// Only the first line of a message carries its prefix; continuation lines have
/// none and inherit the enclosing block's speaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineSpeaker {
    User,
    Assistant,
    Tool,
    Other,
}

fn line_speaker(line: &str) -> Option<LineSpeaker> {
    if line.starts_with(EMOJI_USER) {
        Some(LineSpeaker::User)
    } else if line.starts_with(EMOJI_ASSISTANT) {
        Some(LineSpeaker::Assistant)
    } else if line.starts_with(EMOJI_SUCCESS) || line.starts_with(EMOJI_FAILURE) {
        Some(LineSpeaker::Tool)
    } else if line.starts_with(EMOJI_QUEUE) || line.starts_with(EMOJI_META) {
        Some(LineSpeaker::Other)
    } else {
        None
    }
}

fn block_speakers(lines: &[&str]) -> Vec<LineSpeaker> {
    let mut current = LineSpeaker::Other;
    lines
        .iter()
        .map(|line| {
            if let Some(kind) = line_speaker(line) {
                current = kind;
            }
            current
        })
        .collect()
}

fn search_file(
    path: &Path,
    artifact: SearchArtifact,
    matcher: &Matcher,
    options: &SearchOptions,
    hits: &mut Vec<SearchHit>,
) -> Result<(), CassioError> {
    let content = fs::read_to_string(path)?;
    let lines: Vec<&str> = content.lines().collect();

    let speakers = options.speaker.map(|speaker| {
        let wanted = speaker.line_kind();
        block_speakers(&lines)
            .into_iter()
            .map(|kind| kind == wanted)
            .collect::<Vec<_>>()
    });

    let mut match_lines = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some(ref wanted) = speakers
            && !wanted[index]
        {
            continue;
        }
        let searchable = if options.include_paths {
            (*line).to_string()
        } else {
            strip_path_noise(line)
        };
        if matcher.is_match(&searchable) {
            match_lines.push(index);
        }
    }

    if match_lines.is_empty() {
        return Ok(());
    }

    // Each context line is attached to the first hit whose window covers it, so
    // overlapping windows between nearby hits do not repeat lines.
    let mut is_context = if options.context > 0 {
        let mut flags = vec![false; lines.len()];
        for index in &match_lines {
            let lo = index.saturating_sub(options.context);
            let hi = (index + options.context + 1).min(lines.len());
            flags[lo..hi].fill(true);
        }
        for index in &match_lines {
            flags[*index] = false;
        }
        flags
    } else {
        Vec::new()
    };

    for index in match_lines {
        let mut context = Vec::new();
        if options.context > 0 {
            let lo = index.saturating_sub(options.context);
            let hi = (index + options.context + 1).min(lines.len());
            for li in lo..hi {
                if is_context[li] {
                    is_context[li] = false;
                    context.push(SearchContextLine {
                        line: li + 1,
                        text: truncate_line(lines[li].trim(), 200),
                    });
                }
            }
        }
        hits.push(SearchHit {
            artifact,
            path: path.to_path_buf(),
            line: index + 1,
            line_end: None,
            text: truncate_line(lines[index].trim(), 280),
            score: None,
            context: (!context.is_empty()).then_some(context),
        });
    }
    Ok(())
}

fn print_hits(root: &Path, query: &str, options: &SearchOptions, hits: &[SearchHit]) {
    if options.count {
        print_counts(root, hits, options.limit);
        return;
    }
    if options.files_with_matches {
        print_matching_files(root, hits, options.limit);
        return;
    }

    let scope = scope_label(options);
    println!(
        "cassio search: {:?} in {} ({})",
        query,
        root.display(),
        scope
    );

    if hits.is_empty() {
        println!("No matches.");
        return;
    }

    if options.semantic.is_some() {
        println!("\n== semantic matches ==");
        for hit in hits {
            print_hit(root, hit);
        }
        return;
    }

    let mut last_artifact = None;
    for hit in hits {
        if last_artifact != Some(hit.artifact) {
            println!("\n== {} ==", artifact_label(hit.artifact));
            last_artifact = Some(hit.artifact);
        }
        print_hit(root, hit);
    }
}

fn scope_label(options: &SearchOptions) -> String {
    let when = match (&options.from, &options.to) {
        (Some(from), Some(to)) if from == to => from.clone(),
        (Some(from), Some(to)) => format!("{from}..{to}"),
        (Some(from), None) => format!("{from}.."),
        (None, Some(to)) => format!("..{to}"),
        (None, None) => "all months".to_string(),
    };
    let mut parts = vec![when];
    if let Some(tool) = &options.tool {
        parts.push(format!("tool={tool}"));
    }
    if let Some(project) = &options.project {
        parts.push(format!("project={project}"));
    }
    if let Some(speaker) = options.speaker {
        parts.push(format!("speaker={}", speaker.name()));
    }
    parts.join(", ")
}

fn print_hit(root: &Path, hit: &SearchHit) {
    let display_path = hit.path.strip_prefix(root).unwrap_or(&hit.path);
    let line = match hit.line_end {
        Some(end) if end > hit.line => format!("{}-{}", hit.line, end),
        _ => hit.line.to_string(),
    };
    if let Some(score) = hit.score {
        println!(
            "{}:{} [{score:.3}]: {}",
            display_path.display(),
            line,
            hit.text
        );
    } else {
        println!("{}:{}: {}", display_path.display(), line, hit.text);
    }
    if let Some(context) = &hit.context {
        for ctx in context {
            println!("  {}| {}", ctx.line, ctx.text);
        }
    }
}

fn print_counts(root: &Path, hits: &[SearchHit], limit: usize) {
    for entry in count_entries(root, hits, limit) {
        println!("{}: {}", entry.path, entry.count);
    }
}

fn print_matching_files(root: &Path, hits: &[SearchHit], limit: usize) {
    for path in matching_file_paths(root, hits, limit) {
        println!("{path}");
    }
}

#[derive(Debug, Serialize)]
struct CountEntry {
    path: String,
    count: usize,
}

fn count_entries(root: &Path, hits: &[SearchHit], limit: usize) -> Vec<CountEntry> {
    let mut entries: Vec<CountEntry> = Vec::new();
    for hit in hits {
        let path = hit.path.strip_prefix(root).unwrap_or(&hit.path);
        let path = path.display().to_string();
        match entries.iter_mut().find(|e| e.path == path) {
            Some(entry) => entry.count += 1,
            None => entries.push(CountEntry { path, count: 1 }),
        }
    }
    entries.truncate(limit);
    entries
}

fn matching_file_paths(root: &Path, hits: &[SearchHit], limit: usize) -> Vec<String> {
    let mut paths = Vec::new();
    for hit in hits {
        let path = hit.path.strip_prefix(root).unwrap_or(&hit.path);
        let path = path.display().to_string();
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths.truncate(limit);
    paths
}

fn artifact_label(artifact: SearchArtifact) -> &'static str {
    match artifact {
        SearchArtifact::Monthly => "monthly summaries",
        SearchArtifact::Daily => "daily compactions",
        SearchArtifact::Session => "session transcripts",
        SearchArtifact::Training => "training metadata",
    }
}

fn semantic_search(
    root: &Path,
    query: &str,
    options: &SearchOptions,
    bounds: &DateBounds,
) -> Result<Vec<SearchHit>, CassioError> {
    let Some(semantic) = &options.semantic else {
        return Ok(Vec::new());
    };
    if query.split_whitespace().next().is_none() {
        return Err(CassioError::Other("Search query cannot be empty".into()));
    }
    if !root.exists() {
        return Err(CassioError::Other(format!(
            "Search target does not exist: {}",
            root.display()
        )));
    }

    let index_path = index::index_path_for(root, &semantic.provider, &semantic.model);
    if !index_path.exists() {
        return Err(CassioError::Other(format!(
            "Semantic index not found: {} (run `cassio index` first)",
            index_path.display()
        )));
    }

    let query_embeddings = index::embed_texts(
        &semantic.provider,
        &semantic.base_url,
        &semantic.model,
        &[query],
        semantic.timeout_secs,
    )?;
    let Some(query_embedding) = query_embeddings.first() else {
        return Err(CassioError::Other(
            "Embedding provider returned no query embedding".into(),
        ));
    };

    let conn = Connection::open(&index_path)
        .map_err(|e| CassioError::Other(format!("Failed to open semantic index: {e}")))?;
    let mut stmt = conn
        .prepare(
            r#"
            SELECT source_path, artifact, line_start, line_end, chunk_text, embedding
            FROM chunks
            "#,
        )
        .map_err(|e| CassioError::Other(format!("Failed to query semantic index: {e}")))?;
    let rows = stmt
        .query_map(params![], |row| {
            Ok(IndexedChunkRow {
                source_path: row.get(0)?,
                artifact: row.get(1)?,
                line_start: row.get::<_, i64>(2)? as usize,
                line_end: row.get::<_, i64>(3)? as usize,
                chunk_text: row.get(4)?,
                embedding: row.get(5)?,
            })
        })
        .map_err(|e| CassioError::Other(format!("Failed to read semantic index: {e}")))?;

    let mut hits = Vec::new();
    for row in rows {
        let row =
            row.map_err(|e| CassioError::Other(format!("Failed to read indexed chunk: {e}")))?;
        let Some(artifact) = artifact_from_index_name(&row.artifact) else {
            continue;
        };
        if !artifact_in_scope(artifact, &row.source_path, options, bounds) {
            continue;
        }
        let embedding = index::decode_embedding(&row.embedding)?;
        let Some(score) = cosine_similarity(query_embedding, &embedding) else {
            continue;
        };
        hits.push(SearchHit {
            artifact,
            path: root.join(row.source_path),
            line: row.line_start,
            line_end: Some(row.line_end),
            text: truncate_line(&row.chunk_text.replace('\n', " / "), 500),
            score: Some(score),
            context: None,
        });
    }

    hits.sort_by(|a, b| {
        b.score
            .unwrap_or(f32::NEG_INFINITY)
            .total_cmp(&a.score.unwrap_or(f32::NEG_INFINITY))
    });
    hits.truncate(options.limit);
    Ok(hits)
}

struct IndexedChunkRow {
    source_path: String,
    artifact: String,
    line_start: usize,
    line_end: usize,
    chunk_text: String,
    embedding: Vec<u8>,
}

fn artifact_in_scope(
    artifact: SearchArtifact,
    source_path: &str,
    options: &SearchOptions,
    bounds: &DateBounds,
) -> bool {
    // source_path is root-relative like `2026-04/2026-04-30.daily.md`.
    let month = source_path
        .split('/')
        .next()
        .filter(|month| is_month(month));
    match month {
        Some(month) => {
            if !bounds.contains(month, file_date_of(Path::new(source_path)).as_deref()) {
                return false;
            }
        }
        None => {
            if bounds.from.is_some() || bounds.to.is_some() {
                return false;
            }
        }
    }
    if let Some(tool) = options.tool.as_deref() {
        if !matches!(artifact, SearchArtifact::Session | SearchArtifact::Training) {
            return false;
        }
        let stem = file_stem(Path::new(source_path));
        if !session_tool_suffix(&stem).is_some_and(|suffix| suffix.eq_ignore_ascii_case(tool)) {
            return false;
        }
    }
    if options.summaries_only
        && !matches!(artifact, SearchArtifact::Monthly | SearchArtifact::Daily)
    {
        return false;
    }
    if artifact == SearchArtifact::Training && !options.include_training {
        return false;
    }
    true
}

fn artifact_from_index_name(name: &str) -> Option<SearchArtifact> {
    match name {
        "monthly" => Some(SearchArtifact::Monthly),
        "daily" => Some(SearchArtifact::Daily),
        "session" => Some(SearchArtifact::Session),
        "training" => Some(SearchArtifact::Training),
        _ => None,
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let mut dot = 0.0f32;
    let mut a_norm = 0.0f32;
    let mut b_norm = 0.0f32;
    for (left, right) in a.iter().zip(b) {
        dot += left * right;
        a_norm += left * left;
        b_norm += right * right;
    }
    if a_norm == 0.0 || b_norm == 0.0 {
        return None;
    }
    Some(dot / (a_norm.sqrt() * b_norm.sqrt()))
}

fn normalize_term(term: &str) -> String {
    term.to_lowercase()
}

pub(crate) fn strip_path_noise(line: &str) -> String {
    let without_markdown_targets = strip_markdown_link_targets(line);
    let mut scrubbed = String::with_capacity(without_markdown_targets.len());
    for token in without_markdown_targets.split_whitespace() {
        if looks_like_path_token(token) {
            continue;
        }
        if !scrubbed.is_empty() {
            scrubbed.push(' ');
        }
        scrubbed.push_str(&strip_embedded_paths(token));
    }
    scrubbed
}

fn strip_markdown_link_targets(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find("](") {
        let (before, after_start) = rest.split_at(start);
        out.push_str(before);
        let after_start = &after_start[2..];
        if let Some(end) = after_start.find(')') {
            rest = &after_start[end + 1..];
        } else {
            out.push_str("](");
            rest = after_start;
            break;
        }
    }
    out.push_str(rest);
    out
}

fn looks_like_path_token(token: &str) -> bool {
    let trimmed = token.trim_matches(path_boundary_char);
    trimmed.starts_with('/')
        || trimmed.starts_with("~/")
        || trimmed.starts_with("./")
        || trimmed.starts_with("../")
}

const EMBEDDED_PATH_MARKERS: &[&str] = &[
    "/Users/",
    "/Volumes/",
    "/var/",
    "/tmp/",
    "/opt/",
    "/usr/",
    "~/",
    "./",
    "../",
];

fn strip_embedded_paths(token: &str) -> String {
    let mut out = token.to_string();
    for marker in EMBEDDED_PATH_MARKERS {
        while let Some(start) = out.find(marker) {
            let end = out[start..]
                .find(path_terminal_char)
                .map(|offset| start + offset)
                .unwrap_or(out.len());
            out.replace_range(start..end, "");
        }
    }
    out
}

fn path_boundary_char(ch: char) -> bool {
    matches!(
        ch,
        '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | ':'
    )
}

fn path_terminal_char(ch: char) -> bool {
    matches!(
        ch,
        '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
    ) || ch.is_whitespace()
}

fn truncate_line(line: &str, max_chars: usize) -> String {
    let mut chars = line.chars();
    let mut out: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
#[path = "search_test.rs"]
mod tests;
