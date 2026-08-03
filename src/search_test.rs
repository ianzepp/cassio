use super::*;

fn test_options() -> SearchOptions {
    SearchOptions {
        from: None,
        to: None,
        tool: None,
        project: None,
        speaker: None,
        limit: 50,
        summaries_only: false,
        include_training: false,
        include_paths: false,
        json: false,
        regex: false,
        case_sensitive: false,
        context: 0,
        files_with_matches: false,
        count: false,
        oldest_first: false,
        semantic: None,
        training_root: None,
    }
}

/// Write a session transcript under `root/YYYY-MM/` with the given content.
fn write_session(root: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let month = &name[..7];
    let dir = root.join(month);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

fn temp_root(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("cassio_search_{label}_{}", std::process::id()))
}

#[test]
fn literal_query_matches_all_terms_case_insensitively() {
    let matcher = Matcher::new("Skill Author", false, false).unwrap();
    assert!(matcher.is_match("updated the skill-author workflow"));
    assert!(!matcher.is_match("updated the skill workflow"));
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
    let root = temp_root("scrub");
    let path = write_session(
        &root,
        "2026-04-24T14-24-33-codex.md",
        r#"✅ Read: file="/Users/ianzepp/github/gauntlet/ghostfolio/get_holdings.json"
👤 should I use Zepp Equity or Zepp Holdings as the name?
"#,
    );
    let hits = search(&root, "zepp holdings", &test_options()).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line, 2);
    assert_eq!(hits[0].path, path);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn include_paths_allows_path_matches() {
    let root = temp_root("paths");
    write_session(
        &root,
        "2026-04-24T14-24-33-codex.md",
        r#"✅ Read: file="/Users/ianzepp/github/gauntlet/ghostfolio/get_holdings.json"
"#,
    );
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

// ---------------------------------------------------------------------------
// Ordering
// ---------------------------------------------------------------------------

#[test]
fn default_search_is_newest_first() {
    let root = temp_root("newest");
    write_session(&root, "2026-03-10T10-00-00-codex.md", "👤 hello zepp\n");
    write_session(&root, "2026-06-15T10-00-00-codex.md", "👤 hello zepp\n");
    let hits = search(&root, "zepp", &test_options()).unwrap();
    assert_eq!(hits.len(), 2);
    assert!(
        hits[0].path.to_string_lossy().contains("2026-06"),
        "newest month first: {}",
        hits[0].path.display()
    );
    assert!(hits[1].path.to_string_lossy().contains("2026-03"));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn oldest_first_flag_reverses_ordering() {
    let root = temp_root("oldest");
    write_session(&root, "2026-03-10T10-00-00-codex.md", "👤 hello zepp\n");
    write_session(&root, "2026-06-15T10-00-00-codex.md", "👤 hello zepp\n");
    let mut options = test_options();
    options.oldest_first = true;
    let hits = search(&root, "zepp", &options).unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits[0].path.to_string_lossy().contains("2026-03"));

    std::fs::remove_dir_all(&root).ok();
}

// ---------------------------------------------------------------------------
// When bounds
// ---------------------------------------------------------------------------

#[test]
fn from_to_month_range_filters_months() {
    let root = temp_root("months");
    write_session(&root, "2026-03-10T10-00-00-codex.md", "👤 hello zepp\n");
    write_session(&root, "2026-04-10T10-00-00-codex.md", "👤 hello zepp\n");
    write_session(&root, "2026-05-10T10-00-00-codex.md", "👤 hello zepp\n");
    let mut options = test_options();
    options.from = Some("2026-04".to_string());
    options.to = Some("2026-05".to_string());
    let hits = search(&root, "zepp", &options).unwrap();
    let months: Vec<String> = hits
        .iter()
        .map(|h| month_dir_of(&h.path, &root).expect("hit lives in a month dir"))
        .collect();
    assert_eq!(months, vec!["2026-05".to_string(), "2026-04".to_string()]);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn from_to_day_range_filters_sessions_within_a_month() {
    let root = temp_root("days");
    write_session(&root, "2026-04-10T10-00-00-codex.md", "👤 hello zepp\n");
    write_session(&root, "2026-04-20T10-00-00-codex.md", "👤 hello zepp\n");
    let mut options = test_options();
    options.from = Some("2026-04-15".to_string());
    let hits = search(&root, "zepp", &options).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].path.to_string_lossy().contains("2026-04-20"));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn month_flag_missing_directory_errors() {
    let root = temp_root("missing");
    std::fs::create_dir_all(&root).unwrap();
    let mut options = test_options();
    options.from = Some("2026-09".to_string());
    options.to = Some("2026-09".to_string());
    let err = search(&root, "zepp", &options).unwrap_err();
    assert!(err.to_string().contains("Search target does not exist"));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn invalid_date_format_errors() {
    let root = temp_root("baddate");
    std::fs::create_dir_all(&root).unwrap();
    for bad in ["2026-13", "2026-04-32", "2026/04", "april"] {
        let mut options = test_options();
        options.from = Some(bad.to_string());
        let err = search(&root, "zepp", &options).unwrap_err();
        assert!(
            err.to_string().contains("Invalid date"),
            "expected Invalid date for {bad}, got: {err}"
        );
    }
    std::fs::remove_dir_all(&root).ok();
}

// ---------------------------------------------------------------------------
// Where: --tool and --project
// ---------------------------------------------------------------------------

#[test]
fn tool_filter_restricts_sessions_and_skips_summaries() {
    let root = temp_root("tool");
    write_session(&root, "2026-04-10T10-00-00-codex.md", "👤 hello zepp\n");
    write_session(&root, "2026-04-11T10-00-00-grok.md", "👤 hello zepp\n");
    write_session(&root, "2026-04-28.daily.md", "hello zepp\n");
    write_session(&root, "2026-04.monthly.md", "hello zepp\n");

    let mut options = test_options();
    options.tool = Some("codex".to_string());
    let hits = search(&root, "zepp", &options).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].path.to_string_lossy().ends_with("-codex.md"));

    // Case-insensitive, and the session artifact is picked regardless of case.
    options.tool = Some("GroK".to_string());
    let hits = search(&root, "zepp", &options).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].path.to_string_lossy().ends_with("-grok.md"));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn tool_filter_rejects_unknown_tools_and_summaries_only() {
    let root = temp_root("toolbad");
    std::fs::create_dir_all(&root).unwrap();
    let mut options = test_options();
    options.tool = Some("bogus".to_string());
    let err = search(&root, "zepp", &options).unwrap_err();
    assert!(err.to_string().contains("Unknown tool 'bogus'"));

    options.tool = Some("codex".to_string());
    options.summaries_only = true;
    let err = search(&root, "zepp", &options).unwrap_err();
    assert!(
        err.to_string()
            .contains("--tool cannot be combined with --summaries-only")
    );

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn project_filter_matches_header_and_excludes_missing() {
    let root = temp_root("project");
    write_session(
        &root,
        "2026-04-10T10-00-00-codex.md",
        "📋 Session: abc\n📋 Project: /Users/ianzepp/work/ianzepp/faber\n👤 hello zepp\n",
    );
    write_session(
        &root,
        "2026-04-11T10-00-00-grok.md",
        "📋 Session: def\n📋 Project: /Users/ianzepp/work/ianzepp/cassio\n👤 hello zepp\n",
    );
    // Old sessions may lack a Project header entirely.
    write_session(&root, "2026-04-12T10-00-00-pi.md", "👤 hello zepp\n");

    let mut options = test_options();
    options.project = Some("faber".to_string());
    let hits = search(&root, "zepp", &options).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].path.to_string_lossy().ends_with("-codex.md"));

    options.project = Some("nope".to_string());
    assert!(search(&root, "zepp", &options).unwrap().is_empty());

    std::fs::remove_dir_all(&root).ok();
}

// ---------------------------------------------------------------------------
// What: --speaker
// ---------------------------------------------------------------------------

const SPEAKER_SAMPLE: &str = "📋 Session: abc\n\
     👤 user said hello zepp\n\
     🤖 assistant replied hello zepp\n\
     ✅ Read: file=\"/tmp/zepp.txt\" hello zepp\n\
     ❌ Grep failed on zepp\n";

#[test]
fn speaker_user_matches_only_user_blocks() {
    let root = temp_root("spkuser");
    write_session(&root, "2026-04-10T10-00-00-codex.md", SPEAKER_SAMPLE);
    let mut options = test_options();
    options.speaker = Some(Speaker::User);
    let hits = search(&root, "zepp", &options).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line, 2);
    assert!(hits[0].text.starts_with("👤"));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn speaker_user_continuation_lines_inherit_speaker() {
    let root = temp_root("spkcont");
    write_session(
        &root,
        "2026-04-10T10-00-00-codex.md",
        "👤 user said hello\ncontinuation line with world\n🤖 assistant said world\n",
    );
    let mut options = test_options();
    options.speaker = Some(Speaker::User);
    let hits = search(&root, "world", &options).unwrap();
    assert_eq!(hits.len(), 1, "continuation inherits the user block");
    // The continuation line (line 2) matches; the assistant line (line 3) does not.
    assert_eq!(hits[0].line, 2);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn speaker_assistant_and_tool_match_their_blocks() {
    let root = temp_root("spkrest");
    write_session(&root, "2026-04-10T10-00-00-codex.md", SPEAKER_SAMPLE);

    let mut options = test_options();
    options.speaker = Some(Speaker::Assistant);
    let hits = search(&root, "zepp", &options).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line, 3);

    // Both ✅ and ❌ lines carry the tool speaker; "hello zepp" only appears in
    // the successful read line.
    options.speaker = Some(Speaker::Tool);
    let hits = search(&root, "hello zepp", &options).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line, 4);

    options.speaker = Some(Speaker::Tool);
    let hits = search(&root, "failed", &options).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line, 5);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn speaker_rejects_summaries_only_and_training() {
    let root = temp_root("spkbad");
    std::fs::create_dir_all(&root).unwrap();
    let mut options = test_options();
    options.speaker = Some(Speaker::User);
    options.summaries_only = true;
    assert!(search(&root, "zepp", &options).is_err());

    options.summaries_only = false;
    options.include_training = true;
    assert!(search(&root, "zepp", &options).is_err());
    std::fs::remove_dir_all(&root).ok();
}

// ---------------------------------------------------------------------------
// Presentation: context, count, files-with-matches
// ---------------------------------------------------------------------------

#[test]
fn context_lines_are_attached_once() {
    let root = temp_root("context");
    write_session(
        &root,
        "2026-04-10T10-00-00-codex.md",
        "alpha one\ntwo\nalpha three\nfour\nalpha five\nsix\n",
    );
    let mut options = test_options();
    options.context = 1;
    let hits = search(&root, "alpha", &options).unwrap();
    assert_eq!(hits.len(), 3);
    let context_lines: Vec<usize> = hits
        .iter()
        .flat_map(|h| h.context.iter().flat_map(|c| c.iter().map(|l| l.line)))
        .collect();
    let mut sorted = context_lines.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), context_lines.len(), "no duplicated context");
    // Line 4 sits in the windows of hits 3 and 5 but is attached exactly once.
    assert_eq!(sorted, vec![2, 4, 6]);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn count_mode_reports_per_file_totals() {
    let root = temp_root("count");
    write_session(
        &root,
        "2026-04-10T10-00-00-codex.md",
        "👤 zepp one\n👤 zepp two\n",
    );
    write_session(&root, "2026-04-11T10-00-00-grok.md", "👤 zepp three\n");
    let mut options = test_options();
    options.count = true;
    let hits = search(&root, "zepp", &options).unwrap();
    assert_eq!(hits.len(), 3, "count mode scans every file");

    let entries = count_entries(&root, &hits, 50);
    assert_eq!(entries.len(), 2);
    let mut counts: Vec<usize> = entries.iter().map(|e| e.count).collect();
    counts.sort_unstable();
    assert_eq!(counts, vec![1, 2], "one file with 2 matches, one with 1");

    // --limit caps the *file* list, not the scan.
    let entries = count_entries(&root, &hits, 1);
    assert_eq!(entries.len(), 1);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn files_with_matches_lists_each_file_once() {
    let root = temp_root("files");
    write_session(
        &root,
        "2026-04-10T10-00-00-codex.md",
        "👤 zepp one\n👤 zepp two\n",
    );
    write_session(&root, "2026-04-11T10-00-00-grok.md", "👤 zepp three\n");
    let mut options = test_options();
    options.files_with_matches = true;
    let hits = search(&root, "zepp", &options).unwrap();
    let paths = matching_file_paths(&root, &hits, 50);
    assert_eq!(paths.len(), 2);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn count_and_files_with_matches_conflict() {
    let root = temp_root("conflict");
    std::fs::create_dir_all(&root).unwrap();
    let mut options = test_options();
    options.count = true;
    options.files_with_matches = true;
    let err = search(&root, "zepp", &options).unwrap_err();
    assert!(err.to_string().contains("--count and --files-with-matches"));
    std::fs::remove_dir_all(&root).ok();
}

// ---------------------------------------------------------------------------
// Semantic scope
// ---------------------------------------------------------------------------

fn bounds_for(options: &SearchOptions) -> DateBounds {
    DateBounds::parse(options.from.as_deref(), options.to.as_deref()).unwrap()
}

#[test]
fn semantic_scope_filters_by_range_tool_and_artifact_options() {
    let mut options = test_options();
    options.from = Some("2026-04".to_string());
    options.to = Some("2026-05".to_string());
    options.summaries_only = true;
    let bounds = bounds_for(&options);

    assert!(artifact_in_scope(
        SearchArtifact::Daily,
        "2026-04/2026-04-30.daily.md",
        &options,
        &bounds
    ));
    assert!(!artifact_in_scope(
        SearchArtifact::Session,
        "2026-04/2026-04-30T10-00-00-codex.md",
        &options,
        &bounds
    ));
    assert!(!artifact_in_scope(
        SearchArtifact::Daily,
        "2026-03/2026-03-30.daily.md",
        &options,
        &bounds
    ));
    assert!(artifact_in_scope(
        SearchArtifact::Daily,
        "2026-05/2026-05-30.daily.md",
        &options,
        &bounds
    ));

    options.summaries_only = false;
    options.tool = Some("codex".to_string());
    assert!(artifact_in_scope(
        SearchArtifact::Session,
        "2026-04/2026-04-30T10-00-00-codex.md",
        &options,
        &bounds
    ));
    assert!(!artifact_in_scope(
        SearchArtifact::Session,
        "2026-04/2026-04-30T10-00-00-grok.md",
        &options,
        &bounds
    ));
    assert!(!artifact_in_scope(
        SearchArtifact::Daily,
        "2026-04/2026-04-30.daily.md",
        &options,
        &bounds
    ));
}

#[test]
fn semantic_day_bounds_constrain_sessions() {
    let mut options = test_options();
    options.from = Some("2026-04-15".to_string());
    let bounds = bounds_for(&options);
    assert!(!artifact_in_scope(
        SearchArtifact::Session,
        "2026-04/2026-04-10T10-00-00-codex.md",
        &options,
        &bounds
    ));
    assert!(artifact_in_scope(
        SearchArtifact::Session,
        "2026-04/2026-04-20T10-00-00-codex.md",
        &options,
        &bounds
    ));
}

#[test]
fn speaker_parse_rejects_unknown_values() {
    assert_eq!("user".parse::<Speaker>().unwrap(), Speaker::User);
    assert_eq!("ASSISTANT".parse::<Speaker>().unwrap(), Speaker::Assistant);
    assert!("model".parse::<Speaker>().is_err());
}

#[test]
fn date_bounds_validate_calendar_dates() {
    assert!(is_month("2026-04"));
    assert!(!is_month("2026-13"));
    assert!(is_day("2026-04-15"));
    assert!(!is_day("2026-04-32"));
}
