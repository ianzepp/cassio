//! CaseStudyEvidence: parse, validate, and merge structured case-study blocks
//! embedded in daily/weekly compaction markdown.
//!
//! Schema: `docs/case-study-evidence.md`

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::CassioError;

const EVIDENCE_HEADING: &str = "## CaseStudyEvidence";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CaseStudyEvidence {
    pub period: String,
    #[serde(default = "default_period_kind")]
    pub period_kind: String,
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default)]
    pub volume: Volume,
    #[serde(default)]
    pub agents_used: Vec<AgentUse>,
    #[serde(default)]
    pub instruction_deltas: Vec<InstructionDelta>,
    #[serde(default)]
    pub corrections: Vec<Correction>,
    #[serde(default)]
    pub outcomes: Vec<Outcome>,
    #[serde(default)]
    pub process_invocations: Vec<String>,
    #[serde(default)]
    pub open_threads: Vec<String>,
    #[serde(default)]
    pub case_study_quotes: Vec<String>,
    #[serde(default)]
    pub metrics_ref: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

fn default_period_kind() -> String {
    "day".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Volume {
    #[serde(default)]
    pub sessions: u32,
    #[serde(default)]
    pub user_turns: Option<u32>,
    #[serde(default)]
    pub corrections: u32,
    #[serde(default)]
    pub decisions: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentUse {
    pub tool: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub sessions: u32,
    #[serde(default)]
    pub tokens_in: Option<u64>,
    #[serde(default)]
    pub tokens_out: Option<u64>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct InstructionDelta {
    pub summary: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub quote: Option<String>,
    #[serde(default)]
    pub codified: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Correction {
    #[serde(rename = "type")]
    pub kind: String,
    pub quote: String,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub codified_into: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Outcome {
    pub unit: String,
    pub result: String,
    #[serde(default)]
    pub sessions: Option<u32>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// Extract the YAML body of the CaseStudyEvidence section from markdown.
pub fn extract_yaml_block(markdown: &str) -> Option<String> {
    let heading_idx = markdown.find(EVIDENCE_HEADING)?;
    let after = &markdown[heading_idx + EVIDENCE_HEADING.len()..];
    // Find fenced yaml block
    let re = Regex::new(r"(?s)```(?:yaml|yml)\s*\n(.*?)```").ok()?;
    re.captures(after)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

/// Parse CaseStudyEvidence from a daily/weekly markdown document.
pub fn parse_from_markdown(markdown: &str) -> Result<Option<CaseStudyEvidence>, CassioError> {
    let Some(yaml) = extract_yaml_block(markdown) else {
        return Ok(None);
    };
    let evidence: CaseStudyEvidence = serde_yaml::from_str(&yaml)
        .map_err(|e| CassioError::Other(format!("CaseStudyEvidence YAML parse failed: {e}")))?;
    Ok(Some(evidence))
}

/// Parse YAML body directly.
pub fn parse_yaml(yaml: &str) -> Result<CaseStudyEvidence, CassioError> {
    serde_yaml::from_str(yaml)
        .map_err(|e| CassioError::Other(format!("CaseStudyEvidence YAML parse failed: {e}")))
}

/// Serialize evidence to a markdown section.
pub fn to_markdown_section(evidence: &CaseStudyEvidence) -> Result<String, CassioError> {
    let yaml = serde_yaml::to_string(evidence)
        .map_err(|e| CassioError::Other(format!("CaseStudyEvidence YAML emit failed: {e}")))?;
    Ok(format!("## CaseStudyEvidence\n\n```yaml\n{}```\n", yaml))
}

/// Soft validation warnings (empty if OK).
pub fn validate_warnings(evidence: &CaseStudyEvidence) -> Vec<String> {
    let mut warnings = Vec::new();
    if evidence.period.trim().is_empty() {
        warnings.push("period is empty".into());
    }
    if evidence.period_kind != "day" && evidence.period_kind != "week" {
        warnings.push(format!(
            "period_kind should be day|week, got {}",
            evidence.period_kind
        ));
    }
    let n = evidence.corrections.len() as u32;
    if evidence.volume.corrections != n && n > 0 {
        warnings.push(format!(
            "volume.corrections={} disagrees with corrections.len()={}",
            evidence.volume.corrections, n
        ));
    }
    if evidence.case_study_quotes.len() > 10 {
        warnings.push(format!(
            "case_study_quotes has {} entries (cap 10)",
            evidence.case_study_quotes.len()
        ));
    }
    let allowed_corr = [
        "over_abstraction",
        "ignored_clean_break",
        "wrong_boundary",
        "tool_misuse",
        "ignored_instruction",
        "over_hedging",
        "terminology",
        "other",
    ];
    for c in &evidence.corrections {
        if !allowed_corr.contains(&c.kind.as_str()) {
            warnings.push(format!("unknown correction type: {}", c.kind));
        }
        if c.quote.trim().is_empty() {
            warnings.push("correction with empty quote".into());
        }
    }
    let allowed_out = [
        "autonomous_success",
        "first_pass",
        "rework",
        "hardening",
        "deferred",
        "abandoned",
        "unknown",
    ];
    for o in &evidence.outcomes {
        if !allowed_out.contains(&o.result.as_str()) {
            warnings.push(format!("unknown outcome result: {}", o.result));
        }
    }
    warnings
}

fn outcome_rank(result: &str) -> u8 {
    match result {
        "abandoned" => 6,
        "deferred" => 5,
        "hardening" => 4,
        "rework" => 3,
        "first_pass" => 2,
        "autonomous_success" => 1,
        _ => 0,
    }
}

fn sum_opt_u32(a: Option<u32>, b: Option<u32>) -> Option<u32> {
    match (a, b) {
        (None, None) => None,
        (Some(x), None) | (None, Some(x)) => Some(x),
        (Some(x), Some(y)) => Some(x.saturating_add(y)),
    }
}

fn sum_opt_u64(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, None) => None,
        (Some(x), None) | (None, Some(x)) => Some(x),
        (Some(x), Some(y)) => Some(x.saturating_add(y)),
    }
}

fn sum_opt_f64(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (None, None) => None,
        (Some(x), None) | (None, Some(x)) => Some(x),
        (Some(x), Some(y)) => Some(x + y),
    }
}

/// Merge multiple evidence blocks (chunk partials or daily→weekly).
pub fn merge_evidence(
    period: &str,
    period_kind: &str,
    parts: &[CaseStudyEvidence],
) -> CaseStudyEvidence {
    let mut out = CaseStudyEvidence {
        period: period.to_string(),
        period_kind: period_kind.to_string(),
        ..Default::default()
    };

    let mut projects: BTreeSet<String> = BTreeSet::new();
    let mut agents: BTreeMap<(String, String), AgentUse> = BTreeMap::new();
    let mut deltas: BTreeMap<String, InstructionDelta> = BTreeMap::new();
    let mut corrections: BTreeMap<String, Correction> = BTreeMap::new();
    let mut outcomes: BTreeMap<String, Outcome> = BTreeMap::new();
    let mut process: BTreeSet<String> = BTreeSet::new();
    let mut threads: BTreeSet<String> = BTreeSet::new();
    let mut quotes: Vec<String> = Vec::new();
    let mut quote_set: BTreeSet<String> = BTreeSet::new();
    let mut metrics_ref: Option<String> = None;
    let mut notes: Vec<String> = Vec::new();

    for part in parts {
        out.volume.sessions = out.volume.sessions.saturating_add(part.volume.sessions);
        out.volume.user_turns = sum_opt_u32(out.volume.user_turns, part.volume.user_turns);
        out.volume.decisions = out.volume.decisions.saturating_add(part.volume.decisions);
        // corrections volume recomputed from list at end

        for p in &part.projects {
            if !p.trim().is_empty() {
                projects.insert(p.clone());
            }
        }
        for a in &part.agents_used {
            let key = (a.tool.clone(), a.model.clone().unwrap_or_default());
            let entry = agents.entry(key).or_insert_with(|| AgentUse {
                tool: a.tool.clone(),
                model: a.model.clone(),
                ..Default::default()
            });
            entry.sessions = entry.sessions.saturating_add(a.sessions);
            entry.tokens_in = sum_opt_u64(entry.tokens_in, a.tokens_in);
            entry.tokens_out = sum_opt_u64(entry.tokens_out, a.tokens_out);
            entry.cost_usd = sum_opt_f64(entry.cost_usd, a.cost_usd);
        }
        for d in &part.instruction_deltas {
            let key = d.summary.trim().to_lowercase();
            deltas.entry(key).or_insert_with(|| d.clone());
        }
        for c in &part.corrections {
            corrections
                .entry(c.quote.clone())
                .or_insert_with(|| c.clone());
        }
        for o in &part.outcomes {
            let key = o.unit.trim().to_lowercase();
            outcomes
                .entry(key)
                .and_modify(|existing| {
                    if outcome_rank(&o.result) > outcome_rank(&existing.result) {
                        existing.result = o.result.clone();
                    }
                    existing.sessions = sum_opt_u32(existing.sessions, o.sessions);
                    if existing.notes.is_none() {
                        existing.notes = o.notes.clone();
                    }
                })
                .or_insert_with(|| o.clone());
        }
        for p in &part.process_invocations {
            if !p.trim().is_empty() {
                process.insert(p.clone());
            }
        }
        for t in &part.open_threads {
            if !t.trim().is_empty() {
                threads.insert(t.clone());
            }
        }
        for q in &part.case_study_quotes {
            if quote_set.insert(q.clone()) {
                quotes.push(q.clone());
            }
        }
        if metrics_ref.is_none() {
            if let Some(r) = &part.metrics_ref {
                if !r.is_empty() {
                    metrics_ref = Some(r.clone());
                }
            }
        }
        if let Some(n) = &part.notes {
            if !n.trim().is_empty() {
                notes.push(n.clone());
            }
        }
    }

    // Prefer correction quotes in the capped quote list
    let mut ordered_quotes: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for c in corrections.values() {
        if seen.insert(c.quote.clone()) {
            ordered_quotes.push(c.quote.clone());
        }
    }
    for q in quotes {
        if seen.insert(q.clone()) {
            ordered_quotes.push(q);
        }
    }
    ordered_quotes.truncate(10);

    out.projects = projects.into_iter().collect();
    out.agents_used = agents.into_values().collect();
    out.instruction_deltas = deltas.into_values().collect();
    out.corrections = corrections.into_values().collect();
    out.volume.corrections = out.corrections.len() as u32;
    out.outcomes = outcomes.into_values().collect();
    out.process_invocations = process.into_iter().collect();
    out.open_threads = threads.into_iter().collect();
    out.case_study_quotes = ordered_quotes;
    out.metrics_ref = metrics_ref;
    out.notes = if notes.is_empty() {
        None
    } else {
        Some(notes.join(" | "))
    };
    out
}

/// Loss-audit helper: quotes from `expected` missing in `actual`.
pub fn missing_quotes(expected: &[String], actual: &[String]) -> Vec<String> {
    let set: BTreeSet<&str> = actual.iter().map(|s| s.as_str()).collect();
    expected
        .iter()
        .filter(|q| !set.contains(q.as_str()))
        .cloned()
        .collect()
}

/// Loss-audit helper: instruction summaries missing (case-insensitive).
pub fn missing_instruction_summaries(
    expected: &[InstructionDelta],
    actual: &[InstructionDelta],
) -> Vec<String> {
    let set: BTreeSet<String> = actual
        .iter()
        .map(|d| d.summary.trim().to_lowercase())
        .collect();
    expected
        .iter()
        .filter(|d| !set.contains(&d.summary.trim().to_lowercase()))
        .map(|d| d.summary.clone())
        .collect()
}

#[cfg(test)]
#[path = "evidence_test.rs"]
mod evidence_test;
