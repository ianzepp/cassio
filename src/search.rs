use std::fs;
use std::path::{Path, PathBuf};

use regex::RegexBuilder;
use rusqlite::{Connection, params};
use serde::Serialize;
use walkdir::WalkDir;

use crate::error::CassioError;
use crate::index;

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub month: Option<String>,
    pub limit: usize,
    pub summaries_only: bool,
    pub include_training: bool,
    pub include_paths: bool,
    pub json: bool,
    pub regex: bool,
    pub case_sensitive: bool,
    pub semantic: Option<SemanticSearchOptions>,
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
        serde_json::to_writer_pretty(std::io::stdout(), &hits)?;
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

    if options.semantic.is_some() {
        if options.regex {
            return Err(CassioError::Other(
                "--semantic cannot be combined with --regex".into(),
            ));
        }
        return semantic_search(root, query, options);
    }

    let target = if let Some(month) = &options.month {
        root.join(month)
    } else {
        root.to_path_buf()
    };

    if !target.exists() {
        return Err(CassioError::Other(format!(
            "Search target does not exist: {}",
            target.display()
        )));
    }

    let matcher = Matcher::new(query, options.regex, options.case_sensitive)?;
    let mut hits = Vec::new();

    for artifact in artifact_order(options) {
        for path in files_for_artifact(&target, artifact) {
            search_file(&path, artifact, &matcher, options, &mut hits)?;
            if hits.len() >= options.limit {
                return Ok(hits);
            }
        }
    }

    Ok(hits)
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

fn files_for_artifact(root: &Path, artifact: SearchArtifact) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if artifact_for_path(path) == Some(artifact) {
            paths.push(path.to_path_buf());
        }
    }
    paths.sort();
    paths
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

fn search_file(
    path: &Path,
    artifact: SearchArtifact,
    matcher: &Matcher,
    options: &SearchOptions,
    hits: &mut Vec<SearchHit>,
) -> Result<(), CassioError> {
    let content = fs::read_to_string(path)?;
    for (index, line) in content.lines().enumerate() {
        let searchable = if options.include_paths {
            line.to_string()
        } else {
            strip_path_noise(line)
        };
        if !matcher.is_match(&searchable) {
            continue;
        }
        hits.push(SearchHit {
            artifact,
            path: path.to_path_buf(),
            line: index + 1,
            line_end: None,
            text: truncate_line(line.trim(), 280),
            score: None,
        });
        if hits.len() >= options.limit {
            break;
        }
    }
    Ok(())
}

fn print_hits(root: &Path, query: &str, options: &SearchOptions, hits: &[SearchHit]) {
    let scope = options.month.as_deref().unwrap_or("all months");
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
        if !artifact_in_scope(artifact, &row.source_path, options) {
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

fn artifact_in_scope(artifact: SearchArtifact, source_path: &str, options: &SearchOptions) -> bool {
    if let Some(month) = &options.month
        && !source_path.starts_with(&format!("{month}/"))
    {
        return false;
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

fn strip_embedded_paths(token: &str) -> String {
    let mut out = token.to_string();
    for marker in [
        "/Users/",
        "/Volumes/",
        "/var/",
        "/tmp/",
        "/opt/",
        "/usr/",
        "~/",
        "./",
        "../",
    ] {
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
mod tests {
    use super::*;

    #[test]
    fn literal_query_matches_all_terms_case_insensitively() {
        let matcher = Matcher::new("Skill Author", false, false).unwrap();
        assert!(matcher.is_match("updated the skill-author workflow"));
        assert!(!matcher.is_match("updated the skill workflow"));
    }

    fn test_options() -> SearchOptions {
        SearchOptions {
            month: None,
            limit: 50,
            summaries_only: false,
            include_training: false,
            include_paths: false,
            json: false,
            regex: false,
            case_sensitive: false,
            semantic: None,
        }
    }

    #[test]
    fn default_search_ignores_absolute_path_terms() {
        let matcher = Matcher::new("zepp holdings", false, false).unwrap();
        let line = r#"✅ Read: file="/Users/ianzepp/github/gauntlet/ghostfolio/get_holdings.json""#;
        assert!(!matcher.is_match(&strip_path_noise(line)));
        assert!(matcher.is_match("👤 should I use Zepp Equity or Zepp Holdings as the name?"));
    }

    #[test]
    fn include_paths_preserves_old_raw_line_matching() {
        let matcher = Matcher::new("zepp holdings", false, false).unwrap();
        let line = r#"✅ Read: file="/Users/ianzepp/github/gauntlet/ghostfolio/get_holdings.json""#;
        assert!(matcher.is_match(line));
    }

    #[test]
    fn search_uses_path_scrubbed_lines_by_default() {
        let root = std::env::temp_dir().join(format!("cassio_search_test_{}", std::process::id()));
        let month_dir = root.join("2026-04");
        std::fs::create_dir_all(&month_dir).unwrap();
        let path = month_dir.join("2026-04-24T14-24-33-codex.md");
        std::fs::write(
            &path,
            r#"✅ Read: file="/Users/ianzepp/github/gauntlet/ghostfolio/get_holdings.json"
👤 should I use Zepp Equity or Zepp Holdings as the name?
"#,
        )
        .unwrap();

        let hits = search(&root, "zepp holdings", &test_options()).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 2);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn include_paths_allows_path_matches() {
        let root =
            std::env::temp_dir().join(format!("cassio_search_paths_test_{}", std::process::id()));
        let month_dir = root.join("2026-04");
        std::fs::create_dir_all(&month_dir).unwrap();
        let path = month_dir.join("2026-04-24T14-24-33-codex.md");
        std::fs::write(
            &path,
            r#"✅ Read: file="/Users/ianzepp/github/gauntlet/ghostfolio/get_holdings.json"
"#,
        )
        .unwrap();

        let mut options = test_options();
        options.include_paths = true;
        let hits = search(&root, "zepp holdings", &options).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].line, 1);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn artifact_detection_prioritizes_summaries() {
        assert_eq!(
            artifact_for_path(Path::new("2026-04/2026-04.monthly.md")),
            Some(SearchArtifact::Monthly)
        );
        assert_eq!(
            artifact_for_path(Path::new("2026-04/2026-04-28.daily.md")),
            Some(SearchArtifact::Daily)
        );
        assert_eq!(
            artifact_for_path(Path::new("2026-04/2026-04-28T09-29-00-codex.md")),
            Some(SearchArtifact::Session)
        );
        assert_eq!(
            artifact_for_path(Path::new("2026-04/2026-04-28T09-29-00-codex.training.json")),
            Some(SearchArtifact::Training)
        );
    }

    #[test]
    fn root_prompts_are_not_session_markdown() {
        assert!(!is_session_markdown_name("compact-prompt.md"));
        assert!(!is_session_markdown_name("monthly-prompt.md"));
        assert!(is_session_markdown_name("unknown-claude.md"));
    }

    #[test]
    fn cosine_similarity_scores_identical_vectors_highest() {
        let same = cosine_similarity(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]).unwrap();
        let different = cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).unwrap();
        assert!((same - 1.0).abs() < 0.0001);
        assert_eq!(different, 0.0);
    }

    #[test]
    fn semantic_scope_filters_by_month_and_artifact_options() {
        let mut options = test_options();
        options.month = Some("2026-04".to_string());
        options.summaries_only = true;

        assert!(artifact_in_scope(
            SearchArtifact::Daily,
            "2026-04/2026-04-30.daily.md",
            &options
        ));
        assert!(!artifact_in_scope(
            SearchArtifact::Session,
            "2026-04/2026-04-30T10-00-00-codex.md",
            &options
        ));
        assert!(!artifact_in_scope(
            SearchArtifact::Daily,
            "2026-03/2026-03-30.daily.md",
            &options
        ));
    }
}
