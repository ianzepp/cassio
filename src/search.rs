use std::fs;
use std::path::{Path, PathBuf};

use regex::RegexBuilder;
use serde::Serialize;
use walkdir::WalkDir;

use crate::error::CassioError;

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub month: Option<String>,
    pub limit: usize,
    pub summaries_only: bool,
    pub include_training: bool,
    pub json: bool,
    pub regex: bool,
    pub case_sensitive: bool,
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
    pub text: String,
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
            search_file(&path, artifact, &matcher, options.limit, &mut hits)?;
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

fn artifact_for_path(path: &Path) -> Option<SearchArtifact> {
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
    limit: usize,
    hits: &mut Vec<SearchHit>,
) -> Result<(), CassioError> {
    let content = fs::read_to_string(path)?;
    for (index, line) in content.lines().enumerate() {
        if !matcher.is_match(line) {
            continue;
        }
        hits.push(SearchHit {
            artifact,
            path: path.to_path_buf(),
            line: index + 1,
            text: truncate_line(line.trim(), 280),
        });
        if hits.len() >= limit {
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

    let mut last_artifact = None;
    for hit in hits {
        if last_artifact != Some(hit.artifact) {
            println!("\n== {} ==", artifact_label(hit.artifact));
            last_artifact = Some(hit.artifact);
        }
        let display_path = hit.path.strip_prefix(root).unwrap_or(&hit.path);
        println!("{}:{}: {}", display_path.display(), hit.line, hit.text);
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

fn normalize_term(term: &str) -> String {
    term.to_lowercase()
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
}
