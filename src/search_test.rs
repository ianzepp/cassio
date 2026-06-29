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
