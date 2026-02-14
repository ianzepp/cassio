use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use walkdir::WalkDir;

use crate::error::CassioError;

/// Stats parsed from a single transcript .txt file.
#[derive(Default)]
struct TranscriptStats {
    tool_name: String,
    date: String,       // YYYY-MM-DD
    project: String,
    user_msgs: u32,
    asst_msgs: u32,
    tool_ok: u32,
    tool_fail: u32,
    input_tokens: u64,
    output_tokens: u64,
    duration_secs: i64,
}

/// Aggregated stats for a group (month×tool or project).
#[derive(Default)]
struct Aggregate {
    sessions: u32,
    user_msgs: u32,
    asst_msgs: u32,
    tool_ok: u32,
    tool_fail: u32,
    input_tokens: u64,
    output_tokens: u64,
    duration_secs: i64,
}

impl Aggregate {
    fn add(&mut self, s: &TranscriptStats) {
        self.sessions += 1;
        self.user_msgs += s.user_msgs;
        self.asst_msgs += s.asst_msgs;
        self.tool_ok += s.tool_ok;
        self.tool_fail += s.tool_fail;
        self.input_tokens += s.input_tokens;
        self.output_tokens += s.output_tokens;
        self.duration_secs += s.duration_secs;
    }

    fn add_agg(&mut self, other: &Aggregate) {
        self.sessions += other.sessions;
        self.user_msgs += other.user_msgs;
        self.asst_msgs += other.asst_msgs;
        self.tool_ok += other.tool_ok;
        self.tool_fail += other.tool_fail;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.duration_secs += other.duration_secs;
    }
}

/// Run summary: regular (month×tool) or detailed (per-project).
pub fn run_summary(dir: &Path, detailed: bool) -> Result<(), CassioError> {
    let stats = collect_stats(dir)?;

    if stats.is_empty() {
        eprintln!("No transcript files found in {}", dir.display());
        return Ok(());
    }

    eprintln!("Scanned {} transcript files", stats.len());

    if detailed {
        print_detailed(&stats);
    } else {
        print_regular(&stats);
    }

    Ok(())
}

fn collect_stats(dir: &Path) -> Result<Vec<TranscriptStats>, CassioError> {
    let mut results = Vec::new();

    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        // Match YYYY-MM-DDT...-tool.txt pattern
        if !name.ends_with(".txt") || name.len() < 15 {
            continue;
        }
        let date = &name[..10];
        if date.as_bytes().get(4) != Some(&b'-') || date.as_bytes().get(7) != Some(&b'-') {
            continue;
        }
        // Tool name: last segment before .txt after the last hyphen
        let stem = &name[..name.len() - 4]; // strip .txt
        let tool_name = stem.rsplit('-').next().unwrap_or("unknown").to_string();

        match parse_transcript_stats(path, date, &tool_name) {
            Ok(s) => results.push(s),
            Err(_) => continue,
        }
    }

    Ok(results)
}

fn parse_transcript_stats(
    path: &Path,
    date: &str,
    tool_name: &str,
) -> Result<TranscriptStats, CassioError> {
    let content = std::fs::read_to_string(path)?;
    let mut stats = TranscriptStats {
        date: date.to_string(),
        tool_name: tool_name.to_string(),
        ..Default::default()
    };

    for line in content.lines() {
        if let Some(rest) = strip_emoji_prefix(line, "📋") {
            if let Some(val) = rest.strip_prefix(" Project: ") {
                stats.project = val.to_string();
            } else if let Some(val) = rest.strip_prefix(" Duration: ") {
                stats.duration_secs = parse_duration(val);
            } else if let Some(val) = rest.strip_prefix(" Messages: ") {
                // "N user, M assistant"
                let parts: Vec<&str> = val.split(", ").collect();
                if let Some(u) = parts.first() {
                    stats.user_msgs = u.split_whitespace().next()
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0);
                }
                if let Some(a) = parts.get(1) {
                    stats.asst_msgs = a.split_whitespace().next()
                        .and_then(|n| n.parse().ok())
                        .unwrap_or(0);
                }
            } else if let Some(val) = rest.strip_prefix(" Tool calls: ")
                .or_else(|| rest.strip_prefix(" Function calls: "))
            {
                // "N total, M failed"
                let parts: Vec<&str> = val.split(", ").collect();
                let total: u32 = parts.first()
                    .and_then(|s| s.split_whitespace().next())
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                let failed: u32 = parts.get(1)
                    .and_then(|s| s.split_whitespace().next())
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                stats.tool_ok = total.saturating_sub(failed);
                stats.tool_fail = failed;
            } else if let Some(val) = rest.strip_prefix(" Tokens: ") {
                // "1.2K in, 4.5K out"
                let parts: Vec<&str> = val.split(", ").collect();
                if let Some(inp) = parts.first() {
                    stats.input_tokens = parse_token_value(
                        inp.split_whitespace().next().unwrap_or("0"),
                    );
                }
                if let Some(out) = parts.get(1) {
                    stats.output_tokens = parse_token_value(
                        out.split_whitespace().next().unwrap_or("0"),
                    );
                }
            }
        }
    }

    Ok(stats)
}

/// Strip the emoji prefix (multi-byte) and return the rest of the line.
fn strip_emoji_prefix<'a>(line: &'a str, emoji: &str) -> Option<&'a str> {
    line.strip_prefix(emoji)
}

// --- Regular mode: month × tool ---

fn print_regular(stats: &[TranscriptStats]) {
    // Discover all tools
    let mut tools: BTreeSet<String> = BTreeSet::new();
    for s in stats {
        tools.insert(s.tool_name.clone());
    }
    let tools: Vec<String> = tools.into_iter().collect();

    // Aggregate by (month, tool)
    let mut by_month_tool: BTreeMap<(String, String), Aggregate> = BTreeMap::new();
    for s in stats {
        let month = &s.date[..7];
        by_month_tool
            .entry((month.to_string(), s.tool_name.clone()))
            .or_default()
            .add(s);
    }

    // Collect months
    let mut months: BTreeSet<String> = BTreeSet::new();
    for (key, _) in &by_month_tool {
        months.insert(key.0.clone());
    }
    let months: Vec<String> = months.into_iter().collect();

    // Header
    print!("| Month |");
    for tool in &tools {
        print!(" {tool} |");
    }
    println!(" Total | Tokens | Duration |");

    print!("|-------|");
    for _ in &tools {
        print!(" ---: |");
    }
    println!(" ---: | ---: | ---: |");

    // Rows
    let mut tool_totals: BTreeMap<String, Aggregate> = BTreeMap::new();
    let mut grand_total = Aggregate::default();

    for month in &months {
        print!("| {month} |");
        let mut month_sessions: u32 = 0;
        let mut month_tokens: u64 = 0;
        let mut month_duration: i64 = 0;

        for tool in &tools {
            let key = (month.clone(), tool.clone());
            if let Some(agg) = by_month_tool.get(&key) {
                print!(" {} |", agg.sessions);
                month_sessions += agg.sessions;
                month_tokens += agg.input_tokens + agg.output_tokens;
                month_duration += agg.duration_secs;
                tool_totals.entry(tool.clone()).or_default().add_agg(agg);
            } else {
                print!(" - |");
            }
        }

        grand_total.sessions += month_sessions;
        grand_total.input_tokens += month_tokens;
        grand_total.duration_secs += month_duration;

        println!(
            " {} | {} | {} |",
            month_sessions,
            format_tokens(month_tokens),
            format_duration(month_duration),
        );
    }

    // Totals row
    print!("| **Total** |");
    for tool in &tools {
        if let Some(agg) = tool_totals.get(tool) {
            print!(" **{}** |", agg.sessions);
        } else {
            print!(" - |");
        }
    }
    println!(
        " **{}** | **{}** | **{}** |",
        grand_total.sessions,
        format_tokens(grand_total.input_tokens),
        format_duration(grand_total.duration_secs),
    );
}

// --- Detailed mode: per-project ---

fn print_detailed(stats: &[TranscriptStats]) {
    let mut by_project: BTreeMap<String, Aggregate> = BTreeMap::new();
    for s in stats {
        let key = if s.project.is_empty() {
            "unknown".to_string()
        } else {
            shorten_project(&s.project)
        };
        by_project.entry(key).or_default().add(s);
    }

    println!("| Project | Sessions | User | Asst | Tools (ok/fail) | Tokens (in/out) | Duration |");
    println!("|---------|----------|------|------|-----------------|-----------------|----------|");

    let mut total = Aggregate::default();

    for (project, agg) in &by_project {
        println!(
            "| {} | {} | {} | {} | {}/{} | {}/{} | {} |",
            project,
            agg.sessions,
            agg.user_msgs,
            agg.asst_msgs,
            agg.tool_ok,
            agg.tool_fail,
            format_tokens(agg.input_tokens),
            format_tokens(agg.output_tokens),
            format_duration(agg.duration_secs),
        );
        total.add_agg(agg);
    }

    println!(
        "| **Total** | **{}** | **{}** | **{}** | **{}/{}** | **{}/{}** | **{}** |",
        total.sessions,
        total.user_msgs,
        total.asst_msgs,
        total.tool_ok,
        total.tool_fail,
        format_tokens(total.input_tokens),
        format_tokens(total.output_tokens),
        format_duration(total.duration_secs),
    );
}

/// Shorten a project path: keep last 3 components (or fewer).
fn shorten_project(path: &str) -> String {
    let normalized = path
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts.len() <= 3 {
        return normalized;
    }
    parts[parts.len() - 3..].join("/")
}

// --- formatting helpers ---

fn parse_duration(s: &str) -> i64 {
    let mut secs: i64 = 0;
    let s = s.trim();
    // "1h 4m" or "12m" or "45s"
    for part in s.split_whitespace() {
        if let Some(h) = part.strip_suffix('h') {
            secs += h.parse::<i64>().unwrap_or(0) * 3600;
        } else if let Some(m) = part.strip_suffix('m') {
            secs += m.parse::<i64>().unwrap_or(0) * 60;
        } else if let Some(s_val) = part.strip_suffix('s') {
            secs += s_val.parse::<i64>().unwrap_or(0);
        }
    }
    secs
}

fn parse_token_value(s: &str) -> u64 {
    let s = s.trim();
    if let Some(m) = s.strip_suffix('M') {
        (m.parse::<f64>().unwrap_or(0.0) * 1_000_000.0) as u64
    } else if let Some(k) = s.strip_suffix('K') {
        (k.parse::<f64>().unwrap_or(0.0) * 1_000.0) as u64
    } else {
        s.parse().unwrap_or(0)
    }
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn format_duration(seconds: i64) -> String {
    if seconds <= 0 {
        return "-".to_string();
    }
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}
