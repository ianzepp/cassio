//! Deterministic session metrics rails for case-study pipelines.
//!
//! Reads formatted session transcripts (no LLM) and emits stable JSON
//! for a calendar day or ISO week.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate};
use serde::Serialize;
use walkdir::WalkDir;

use crate::error::CassioError;
use crate::pricing;

const KNOWN_TOOL_SUFFIXES: &[&str] = &[
    "claude-chat",
    "claude",
    "codex",
    "hermes",
    "opencode",
    "pi",
    "grok",
    "cursor",
    "kimi",
];

#[derive(Debug, Clone, Serialize)]
pub struct DayMetrics {
    pub period: String,
    pub period_kind: &'static str,
    pub sessions: u32,
    pub user_msgs: u32,
    pub asst_msgs: u32,
    pub tool_ok: u32,
    pub tool_fail: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub by_tool: BTreeMap<String, AgentBucket>,
    pub by_model: BTreeMap<String, AgentBucket>,
    pub by_project: BTreeMap<String, u32>,
    pub session_files: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AgentBucket {
    pub sessions: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WeekMetrics {
    pub period: String,
    pub period_kind: &'static str,
    pub days: Vec<String>,
    pub sessions: u32,
    pub user_msgs: u32,
    pub asst_msgs: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub by_tool: BTreeMap<String, AgentBucket>,
    pub by_model: BTreeMap<String, AgentBucket>,
}

struct SessionRow {
    date: String,
    tool: String,
    project: String,
    model: Option<String>,
    user_msgs: u32,
    asst_msgs: u32,
    tool_ok: u32,
    tool_fail: u32,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    file_name: String,
}

/// Write day metrics JSON under `output_dir/YYYY-MM/YYYY-MM-DD.metrics.json`.
pub fn write_day_metrics(input_dir: &Path, output_dir: &Path, day: &str) -> Result<PathBuf, CassioError> {
    let metrics = collect_day_metrics(input_dir, day)?;
    let month = day.get(..7).unwrap_or("unknown");
    let out_dir = output_dir.join(month);
    std::fs::create_dir_all(&out_dir)?;
    let path = out_dir.join(format!("{day}.metrics.json"));
    let body = serde_json::to_string_pretty(&metrics)
        .map_err(|e| CassioError::Other(format!("metrics serialize: {e}")))?;
    std::fs::write(&path, body)?;
    Ok(path)
}

/// Write week metrics JSON under `output_dir/YYYY-MM/YYYY-Www.metrics.json`
/// (month folder = Monday's month).
pub fn write_week_metrics(
    input_dir: &Path,
    output_dir: &Path,
    iso_week: &str,
) -> Result<PathBuf, CassioError> {
    let metrics = collect_week_metrics(input_dir, iso_week)?;
    let monday = iso_week_monday(iso_week)?;
    let month = monday.format("%Y-%m").to_string();
    let out_dir = output_dir.join(month);
    std::fs::create_dir_all(&out_dir)?;
    let path = out_dir.join(format!("{iso_week}.metrics.json"));
    let body = serde_json::to_string_pretty(&metrics)
        .map_err(|e| CassioError::Other(format!("metrics serialize: {e}")))?;
    std::fs::write(&path, body)?;
    Ok(path)
}

pub fn collect_day_metrics(input_dir: &Path, day: &str) -> Result<DayMetrics, CassioError> {
    validate_day(day)?;
    let rows = scan_sessions(input_dir)?
        .into_iter()
        .filter(|r| r.date == day)
        .collect::<Vec<_>>();
    Ok(aggregate_day(day, rows))
}

pub fn collect_week_metrics(input_dir: &Path, iso_week: &str) -> Result<WeekMetrics, CassioError> {
    let days = iso_week_days(iso_week)?;
    let day_set: std::collections::BTreeSet<_> = days.iter().cloned().collect();
    let rows = scan_sessions(input_dir)?
        .into_iter()
        .filter(|r| day_set.contains(&r.date))
        .collect::<Vec<_>>();

    let mut by_tool: BTreeMap<String, AgentBucket> = BTreeMap::new();
    let mut by_model: BTreeMap<String, AgentBucket> = BTreeMap::new();
    let mut sessions = 0u32;
    let mut user_msgs = 0u32;
    let mut asst_msgs = 0u32;
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut cost_usd = 0.0;

    for r in &rows {
        sessions += 1;
        user_msgs += r.user_msgs;
        asst_msgs += r.asst_msgs;
        input_tokens += r.input_tokens;
        output_tokens += r.output_tokens;
        cost_usd += r.cost_usd;
        bump_bucket(
            by_tool.entry(r.tool.clone()).or_default(),
            r.input_tokens,
            r.output_tokens,
            r.cost_usd,
        );
        if let Some(m) = &r.model {
            bump_bucket(
                by_model.entry(m.clone()).or_default(),
                r.input_tokens,
                r.output_tokens,
                r.cost_usd,
            );
        }
    }

    Ok(WeekMetrics {
        period: iso_week.to_string(),
        period_kind: "week",
        days,
        sessions,
        user_msgs,
        asst_msgs,
        input_tokens,
        output_tokens,
        cost_usd,
        by_tool,
        by_model,
    })
}

fn aggregate_day(day: &str, rows: Vec<SessionRow>) -> DayMetrics {
    let mut by_tool: BTreeMap<String, AgentBucket> = BTreeMap::new();
    let mut by_model: BTreeMap<String, AgentBucket> = BTreeMap::new();
    let mut by_project: BTreeMap<String, u32> = BTreeMap::new();
    let mut session_files = Vec::new();
    let mut sessions = 0u32;
    let mut user_msgs = 0u32;
    let mut asst_msgs = 0u32;
    let mut tool_ok = 0u32;
    let mut tool_fail = 0u32;
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut cost_usd = 0.0;

    for r in rows {
        sessions += 1;
        user_msgs += r.user_msgs;
        asst_msgs += r.asst_msgs;
        tool_ok += r.tool_ok;
        tool_fail += r.tool_fail;
        input_tokens += r.input_tokens;
        output_tokens += r.output_tokens;
        cost_usd += r.cost_usd;
        *by_project.entry(r.project.clone()).or_default() += 1;
        session_files.push(r.file_name.clone());
        bump_bucket(
            by_tool.entry(r.tool.clone()).or_default(),
            r.input_tokens,
            r.output_tokens,
            r.cost_usd,
        );
        if let Some(m) = &r.model {
            bump_bucket(
                by_model.entry(m.clone()).or_default(),
                r.input_tokens,
                r.output_tokens,
                r.cost_usd,
            );
        }
    }

    DayMetrics {
        period: day.to_string(),
        period_kind: "day",
        sessions,
        user_msgs,
        asst_msgs,
        tool_ok,
        tool_fail,
        input_tokens,
        output_tokens,
        cost_usd,
        by_tool,
        by_model,
        by_project,
        session_files,
    }
}

fn bump_bucket(b: &mut AgentBucket, tin: u64, tout: u64, cost: f64) {
    b.sessions += 1;
    b.input_tokens += tin;
    b.output_tokens += tout;
    b.cost_usd += cost;
}

fn scan_sessions(input_dir: &Path) -> Result<Vec<SessionRow>, CassioError> {
    let mut rows = Vec::new();
    for entry in WalkDir::new(input_dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((date, tool)) = parse_session_filename(name) else {
            continue;
        };
        match parse_session(path, &date, &tool, name) {
            Ok(row) => rows.push(row),
            Err(_) => continue,
        }
    }
    Ok(rows)
}

fn parse_session_filename(name: &str) -> Option<(String, String)> {
    let stem = name.strip_suffix(".md")?;
    if stem.len() < 15 {
        return None;
    }
    let date = stem.get(..10)?;
    if date.as_bytes().get(4) != Some(&b'-') || date.as_bytes().get(7) != Some(&b'-') {
        return None;
    }
    // Session files contain T timestamp
    if !stem.contains('T') {
        return None;
    }
    for suffix in KNOWN_TOOL_SUFFIXES {
        let marker = format!("-{suffix}");
        if stem.ends_with(&marker) {
            return Some((date.to_string(), (*suffix).to_string()));
        }
    }
    None
}

fn parse_session(
    path: &Path,
    date: &str,
    tool: &str,
    file_name: &str,
) -> Result<SessionRow, CassioError> {
    let content = std::fs::read_to_string(path)?;
    let mut project = String::new();
    let mut model = None;
    let mut user_msgs = 0u32;
    let mut asst_msgs = 0u32;
    let mut tool_ok = 0u32;
    let mut tool_fail = 0u32;
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut cost_usd = 0.0;

    // Header for project/model; footer summary for tokens/messages/cost when present
    // (mid-session token lines would otherwise dominate).
    let summary_idx = content
        .find("📋 --- Summary ---")
        .or_else(|| content.find("--- Summary ---"));
    let (header, footer) = match summary_idx {
        Some(i) => (&content[..i], &content[i..]),
        None => (content.as_str(), content.as_str()),
    };

    for line in header.lines() {
        let Some(rest) = line.strip_prefix('📋') else {
            continue;
        };
        if let Some(val) = rest.strip_prefix(" Project: ") {
            project = val.to_string();
        } else if let Some(val) = rest.strip_prefix(" Model: ") {
            model = Some(val.to_string());
        }
    }

    for line in footer.lines() {
        let Some(rest) = line.strip_prefix('📋') else {
            continue;
        };
        if let Some(val) = rest.strip_prefix(" Model: ") {
            model = Some(val.to_string());
        } else if let Some(val) = rest.strip_prefix(" Messages: ") {
            let parts: Vec<&str> = val.split(", ").collect();
            if let Some(u) = parts.first() {
                user_msgs = u
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
            }
            if let Some(a) = parts.get(1) {
                asst_msgs = a
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
            }
        } else if let Some(val) = rest
            .strip_prefix(" Tool calls: ")
            .or_else(|| rest.strip_prefix(" Function calls: "))
        {
            let parts: Vec<&str> = val.split(", ").collect();
            let total: u32 = parts
                .first()
                .and_then(|s| s.split_whitespace().next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            let failed: u32 = parts
                .get(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            tool_ok = total.saturating_sub(failed);
            tool_fail = failed;
        } else if let Some(val) = rest.strip_prefix(" Tokens: ") {
            for part in val.split(", ") {
                let mut it = part.split_whitespace();
                let amount = it.next().unwrap_or("0");
                let label = it.next().unwrap_or("");
                match label {
                    "in" => input_tokens = parse_token_value(amount),
                    "out" => output_tokens = parse_token_value(amount),
                    _ => {}
                }
            }
        } else if let Some(val) = rest.strip_prefix(" Cost: ") {
            let cleaned = val.trim().trim_start_matches('$');
            if let Ok(v) = cleaned.parse::<f64>() {
                cost_usd = v;
            }
        }
    }

    if cost_usd == 0.0 {
        if let Some(price) =
            pricing::estimate_cost(model.as_deref(), input_tokens, output_tokens, 0, 0, None)
        {
            cost_usd = price;
        }
    }

    Ok(SessionRow {
        date: date.to_string(),
        tool: tool.to_string(),
        project,
        model,
        user_msgs,
        asst_msgs,
        tool_ok,
        tool_fail,
        input_tokens,
        output_tokens,
        cost_usd,
        file_name: file_name.to_string(),
    })
}

fn parse_token_value(s: &str) -> u64 {
    let s = s.trim();
    if let Some(num) = s.strip_suffix('K').or_else(|| s.strip_suffix('k')) {
        return (num.parse::<f64>().unwrap_or(0.0) * 1000.0) as u64;
    }
    if let Some(num) = s.strip_suffix('M').or_else(|| s.strip_suffix('m')) {
        return (num.parse::<f64>().unwrap_or(0.0) * 1_000_000.0) as u64;
    }
    s.parse().unwrap_or(0)
}

fn validate_day(day: &str) -> Result<(), CassioError> {
    if day.len() != 10 || day.as_bytes()[4] != b'-' || day.as_bytes()[7] != b'-' {
        return Err(CassioError::Other(format!(
            "Invalid day format: {day} (expected YYYY-MM-DD)"
        )));
    }
    NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .map_err(|e| CassioError::Other(format!("Invalid day {day}: {e}")))?;
    Ok(())
}

/// ISO week id like `2026-W24`.
pub fn iso_week_id(date: NaiveDate) -> String {
    let iso = date.iso_week();
    format!("{:04}-W{:02}", iso.year(), iso.week())
}

pub fn iso_week_monday_public(iso_week: &str) -> Result<NaiveDate, CassioError> {
    iso_week_monday(iso_week)
}

fn iso_week_monday(iso_week: &str) -> Result<NaiveDate, CassioError> {
    // Parse YYYY-Www
    let parts: Vec<&str> = iso_week.split('-').collect();
    if parts.len() != 2 || !parts[1].starts_with('W') {
        return Err(CassioError::Other(format!(
            "Invalid ISO week: {iso_week} (expected YYYY-Www)"
        )));
    }
    let year: i32 = parts[0]
        .parse()
        .map_err(|_| CassioError::Other(format!("Invalid ISO week year: {iso_week}")))?;
    let week: u32 = parts[1][1..]
        .parse()
        .map_err(|_| CassioError::Other(format!("Invalid ISO week number: {iso_week}")))?;
    NaiveDate::from_isoywd_opt(year, week, chrono::Weekday::Mon).ok_or_else(|| {
        CassioError::Other(format!("Invalid ISO week calendar: {iso_week}"))
    })
}

fn iso_week_days(iso_week: &str) -> Result<Vec<String>, CassioError> {
    let monday = iso_week_monday(iso_week)?;
    Ok((0..7)
        .map(|i| (monday + chrono::Duration::days(i)).format("%Y-%m-%d").to_string())
        .collect())
}

#[cfg(test)]
#[path = "metrics_test.rs"]
mod metrics_test;
