use super::*;

fn sample(period: &str, sessions: u32, quote: &str) -> CaseStudyEvidence {
    CaseStudyEvidence {
        period: period.into(),
        period_kind: "day".into(),
        projects: vec!["proj-a".into()],
        volume: Volume {
            sessions,
            user_turns: Some(3),
            corrections: 1,
            decisions: 1,
        },
        agents_used: vec![AgentUse {
            tool: "codex".into(),
            model: Some("gpt-5.5".into()),
            sessions,
            tokens_in: Some(100),
            tokens_out: Some(50),
            cost_usd: Some(0.01),
        }],
        instruction_deltas: vec![InstructionDelta {
            summary: "PokerFace optional".into(),
            target: Some("skills/factory".into()),
            quote: None,
            codified: true,
        }],
        corrections: vec![Correction {
            kind: "terminology".into(),
            quote: quote.into(),
            context: Some("vocab".into()),
            codified_into: None,
        }],
        outcomes: vec![Outcome {
            unit: "colony phases".into(),
            result: "first_pass".into(),
            sessions: Some(sessions),
            notes: None,
        }],
        process_invocations: vec!["factory".into()],
        open_threads: vec!["hygiene budgets".into()],
        case_study_quotes: vec![quote.into()],
        metrics_ref: None,
        notes: None,
    }
}

#[test]
fn merge_sums_volume_and_unions_quotes() {
    let a = sample("2026-06-11", 2, "quote one");
    let mut b = sample("2026-06-11", 3, "quote two");
    b.instruction_deltas[0].summary = "Other rule".into();
    b.outcomes[0].result = "hardening".into();

    let merged = merge_evidence("2026-06-11", "day", &[a, b]);
    assert_eq!(merged.volume.sessions, 5);
    assert_eq!(merged.volume.user_turns, Some(6));
    assert_eq!(merged.corrections.len(), 2);
    assert_eq!(merged.volume.corrections, 2);
    assert_eq!(merged.instruction_deltas.len(), 2);
    assert_eq!(merged.agents_used[0].sessions, 5);
    assert_eq!(merged.outcomes[0].result, "hardening");
    assert!(merged.case_study_quotes.len() <= 10);
}

#[test]
fn parse_from_markdown_roundtrip() {
    let ev = sample("2026-06-11", 1, "exact quote");
    let md = to_markdown_section(&ev).unwrap();
    let full = format!("# Daily\n\n## Arc\nhello\n\n{md}");
    let parsed = parse_from_markdown(&full).unwrap().unwrap();
    assert_eq!(parsed.period, "2026-06-11");
    assert_eq!(parsed.corrections[0].quote, "exact quote");
}

#[test]
fn validate_warns_on_count_mismatch() {
    let mut ev = sample("2026-06-11", 1, "q");
    ev.volume.corrections = 99;
    let w = validate_warnings(&ev);
    assert!(w.iter().any(|x| x.contains("volume.corrections")));
}

#[test]
fn missing_quotes_reports_absent() {
    let missing = missing_quotes(&["a".into(), "b".into()], &["a".into()]);
    assert_eq!(missing, vec!["b".to_string()]);
}
