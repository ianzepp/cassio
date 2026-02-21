//! CLI entry point for cassio.
//!
//! # Architecture overview
//!
//! `main.rs` is the thin coordination layer between the CLI surface and the
//! library crate. Its responsibilities are:
//!
//! 1. Parse CLI arguments (via `clap`)
//! 2. Load and merge configuration from `~/.config/cassio/config.toml`
//! 3. Dispatch to the appropriate processing mode:
//!    - **Subcommands** (`init`, `get`, `set`, `unset`, `docs`, `summary`, `compact`)
//!      are handled inline before the process mode logic.
//!    - **Process mode** routes to `run_single_file`, `run_stdin`, `run_batch_mode`,
//!      or `run_all_mode` based on the presence and type of the `PATH` argument.
//!
//! # Configuration merging
//!
//! CLI flags take precedence over config file values. The merge happens in `run()`
//! after subcommands are handled:
//! - `--output` overrides `config.output`
//! - `--format` is only overridden by config when the CLI value is still the default
//!   `"emoji-text"` — this prevents a config `format` from being silently ignored
//!   when the user explicitly passes `--format` on the command line.
//!
//! # Error handling
//!
//! All functions return `Result<(), CassioError>`. `main()` catches errors and
//! prints them to stderr before exiting with code 1. This keeps error reporting
//! consistent regardless of which path through `run()` failed.

use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

use chrono::{Datelike, Local, Timelike, TimeZone, Utc};
use clap::{Parser as ClapParser, Subcommand};

use cassio::ast::Tool;
use cassio::config::{self, Config};
use cassio::discover;
use cassio::error::CassioError;
use cassio::formatter::{Formatter, OutputFormat};
use cassio::parser::Parser;

#[derive(ClapParser)]
#[command(name = "cassio", about = "AI transcript processor")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Input file or directory (omit for stdin)
    path: Option<PathBuf>,

    /// Output directory for batch mode
    #[arg(short, long, global = true)]
    output: Option<PathBuf>,

    /// Output format
    #[arg(short, long, default_value = "emoji-text", global = true)]
    format: String,

    /// Discover and process all tools' default paths
    #[arg(long, global = true)]
    all: bool,

    /// Regenerate even if output is newer than input
    #[arg(long, global = true)]
    force: bool,

    /// Ignore config file; all options must be explicit
    #[arg(long, global = true)]
    detached: bool,

    /// Only process sessions whose working directory is under this path
    #[arg(long, global = true)]
    filter_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Create a default config file
    Init,
    /// Get a config value (e.g. `cassio get output`)
    Get {
        /// Dotted config key (e.g. "sources.claude", "git.autocommit")
        key: Option<String>,
    },
    /// Set a config value (e.g. `cassio set git.autocommit true`)
    Set {
        /// Dotted config key
        key: String,
        /// Value to set
        value: String,
    },
    /// Remove a config value (e.g. `cassio unset git.autocommit`)
    Unset {
        /// Dotted config key
        key: String,
    },
    /// Show full documentation
    Docs,
    /// Show summary statistics for transcripts
    Summary {
        /// Show per-project detailed stats instead of month×tool overview
        #[arg(long)]
        detailed: bool,
    },
    /// Compact transcripts into daily/monthly analysis
    Compact {
        #[command(subcommand)]
        action: CompactAction,
    },
}

#[derive(Subcommand)]
enum CompactAction {
    /// Run full pipeline: sessions → dailies → monthlies
    All {
        /// Model name passed to the selected provider
        #[arg(short, long)]
        model: Option<String>,
        /// LLM provider: ollama, claude, or codex
        #[arg(short, long)]
        provider: Option<String>,
    },
    /// Compact daily session transcripts into daily summaries
    Dailies {
        /// Input directory containing session transcripts
        #[arg(short, long)]
        input: Option<PathBuf>,
        /// Maximum number of days to process
        #[arg(short, long)]
        limit: Option<usize>,
        /// Model name passed to the selected provider
        #[arg(short, long)]
        model: Option<String>,
        /// LLM provider: ollama, claude, or codex
        #[arg(short, long)]
        provider: Option<String>,
    },
    /// Synthesize daily compactions into a monthly summary
    Monthly {
        /// Month to process (YYYY-MM format, e.g. 2025-12)
        #[arg(short, long)]
        input: String,
        /// Model name passed to the selected provider
        #[arg(short, long)]
        model: Option<String>,
        /// LLM provider: ollama, claude, or codex
        #[arg(short, long)]
        provider: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn run(mut cli: Cli) -> Result<(), CassioError> {
    // Handle config subcommands
    match cli.command {
        Some(Command::Init) => {
            return config::init();
        }
        Some(Command::Get { key }) => {
            return match key {
                Some(k) => config::get_value(&k),
                None => config::list_values(),
            };
        }
        Some(Command::Set { key, value }) => {
            return config::set_value(&key, &value);
        }
        Some(Command::Unset { key }) => {
            return config::unset_value(&key);
        }
        Some(Command::Docs) => {
            print!("{}", include_str!("../README.md"));
            return Ok(());
        }
        Some(Command::Summary { detailed }) => {
            let config = if cli.detached { Config::default() } else { Config::load() };
            let dir = cli
                .output
                .clone()
                .or_else(|| config.output_path())
                .ok_or_else(|| {
                    CassioError::Other(
                        "--output is required (or set via `cassio set output <path>`)".into(),
                    )
                })?;
            return cassio::summary::run_summary(&dir, detailed);
        }
        Some(Command::Compact { action }) => {
            let config = if cli.detached { Config::default() } else { Config::load() };
            let config_output = config.output_path();
            let default_model = config.model.clone().unwrap_or_else(|| "llama3.1".to_string());
            let default_provider = config.provider.clone().unwrap_or_else(|| "ollama".to_string());
            match action {
                CompactAction::All { model, provider } => {
                    let model = model.unwrap_or_else(|| default_model.clone());
                    let provider = provider.unwrap_or_else(|| default_provider.clone());
                    let output_dir = cli
                        .output
                        .clone()
                        .or_else(|| config_output.clone())
                        .ok_or_else(|| {
                            CassioError::Other(
                                "--output is required (or set via `cassio set output <path>`)".into(),
                            )
                        })?;

                    // Step 1: sessions → transcripts
                    eprintln!("=== Step 1: Processing sessions ===\n");
                    let format: OutputFormat = cli
                        .format
                        .parse()
                        .map_err(|e: String| CassioError::Other(e))?;
                    let formatter = format.formatter();
                    let sources =
                        discover::discover_all_sources_with_config(&config.sources);
                    if sources.is_empty() {
                        eprintln!("No session sources found, skipping.");
                    } else {
                        let source_names: Vec<_> =
                            sources.iter().map(|(t, _)| t.to_string()).collect();
                        eprintln!(
                            "Found {} source(s): {}",
                            sources.len(),
                            source_names.join(", ")
                        );
                        for (tool, path) in &sources {
                            eprintln!("\nProcessing {} ({})...", tool, path.display());
                            let files = discover::find_session_files(path, Some(*tool));
                            eprintln!("Found {} session files", files.len());
                            process_file_list(&files, &output_dir, cli.force, &*formatter, cli.filter_dir.as_deref())?;
                        }
                    }

                    // Step 2: transcripts → dailies
                    eprintln!("\n=== Step 2: Compacting dailies ===\n");
                    cassio::compact::run_dailies(&output_dir, &output_dir, None, &model, &provider)?;

                    // Step 3: dailies → monthlies
                    eprintln!("\n=== Step 3: Compacting monthlies ===\n");
                    cassio::compact::run_pending_monthlies(&output_dir, &model, &provider)?;

                    cassio::git::auto_commit_and_push(
                        &output_dir,
                        &format!("cassio compact all ({})", Local::now().format("%Y-%m-%d")),
                        &config.git,
                    )?;

                    return Ok(());
                }
                CompactAction::Dailies {
                    input,
                    limit,
                    model,
                    provider,
                } => {
                    let model = model.unwrap_or_else(|| default_model.clone());
                    let provider = provider.unwrap_or_else(|| default_provider.clone());
                    let input_dir = input
                        .or_else(|| cli.output.clone())
                        .or_else(|| config_output.clone())
                        .ok_or_else(|| {
                            CassioError::Other(
                                "--input is required (or set via `cassio set output <path>`)".into(),
                            )
                        })?;
                    let output_dir = cli
                        .output
                        .clone()
                        .or(config_output)
                        .unwrap_or_else(|| input_dir.clone());
                    cassio::compact::run_dailies(&input_dir, &output_dir, limit, &model, &provider)?;
                    cassio::git::auto_commit_and_push(
                        &output_dir,
                        &format!("cassio compact dailies ({})", Local::now().format("%Y-%m-%d")),
                        &config.git,
                    )?;
                    return Ok(());
                }
                CompactAction::Monthly { input, model, provider } => {
                    let model = model.unwrap_or(default_model);
                    let provider = provider.unwrap_or(default_provider);
                    let dir = cli
                        .output
                        .clone()
                        .or_else(|| config_output.clone())
                        .ok_or_else(|| {
                            CassioError::Other(
                                "--output is required (or set via `cassio set output <path>`)".into(),
                            )
                        })?;
                    cassio::compact::run_monthly(&dir, &input, &model, &provider)?;
                    cassio::git::auto_commit_and_push(
                        &dir,
                        &format!("cassio compact monthly {input}"),
                        &config.git,
                    )?;
                    return Ok(());
                }
            }
        }
        None => {}
    }

    // Process mode — load config and merge
    let config = if cli.detached { Config::default() } else { Config::load() };

    // Merge output: CLI arg → config value
    if cli.output.is_none() {
        cli.output = config.output_path();
    }

    // Merge format: CLI arg (if not default) → config value → "emoji-text"
    if cli.format == "emoji-text" {
        if let Some(ref fmt) = config.format {
            cli.format = fmt.clone();
        }
    }

    let format: OutputFormat = cli
        .format
        .parse()
        .map_err(|e: String| CassioError::Other(e))?;
    let formatter = format.formatter();

    if cli.all {
        return run_all_mode(&cli, &config, &*formatter);
    }

    match cli.path {
        Some(ref path) if path.is_dir() => run_batch_mode(path, &cli, &config, &*formatter),
        Some(ref path) if path.is_file() => run_single_file(path, &*formatter, cli.filter_dir.as_deref()),
        Some(ref path) => Err(CassioError::Other(format!(
            "Path not found: {}",
            path.display()
        ))),
        None => run_stdin(&*formatter, cli.filter_dir.as_deref()),
    }
}

/// Parse and format a single session file, writing output to stdout.
fn run_single_file(path: &Path, formatter: &dyn Formatter, filter_dir: Option<&Path>) -> Result<(), CassioError> {
    let parser = cassio::parser::detect_parser(path)?;
    let session = parser.parse_session(path)?;

    if let Some(filter) = filter_dir {
        let filter_str = filter.to_string_lossy();
        if !session.metadata.project_path.starts_with(filter_str.as_ref()) {
            eprintln!("Skipping: session project path '{}' does not match --filter-dir", session.metadata.project_path);
            return Ok(());
        }
    }

    let stdout = io::stdout();
    let mut writer = stdout.lock();
    formatter.format(&session, &mut writer)?;
    Ok(())
}

/// Read all of stdin, detect the log format from the first non-empty line, and format.
///
/// WHY: Buffering all lines before parsing is necessary because format detection
/// requires peeking at the first line, but the parser needs all lines. An
/// alternative would be a two-pass approach, but that would require the input
/// to be seekable (stdin is not).
fn run_stdin(formatter: &dyn Formatter, filter_dir: Option<&Path>) -> Result<(), CassioError> {
    let stdin = io::stdin();
    let reader = stdin.lock();
    let lines: Vec<String> = reader.lines().map(|l| l.unwrap_or_default()).collect();

    if lines.is_empty() {
        return Err(CassioError::Other("No input on stdin".into()));
    }

    // Detect format from first non-empty line
    let first_line = lines
        .iter()
        .find(|l| !l.trim().is_empty())
        .cloned()
        .unwrap_or_default();

    let session = if first_line.contains("\"session_meta\"")
        || first_line.contains("\"response_item\"")
    {
        cassio::parser::codex::CodexParser::parse_from_lines(lines.into_iter())?
    } else {
        cassio::parser::claude::ClaudeParser::parse_from_lines(lines.into_iter())?
    };

    if let Some(filter) = filter_dir {
        let filter_str = filter.to_string_lossy();
        if !session.metadata.project_path.starts_with(filter_str.as_ref()) {
            eprintln!("Skipping: session project path '{}' does not match --filter-dir", session.metadata.project_path);
            return Ok(());
        }
    }

    let stdout = io::stdout();
    let mut writer = stdout.lock();
    formatter.format(&session, &mut writer)?;
    Ok(())
}

/// Process all session files in a directory, writing formatted output to `--output`.
///
/// The directory is auto-detected for tool type (Claude, Codex, or OpenCode)
/// based on its path. Files whose output is already newer than the input are
/// skipped unless `--force` is set.
fn run_batch_mode(
    dir: &Path,
    cli: &Cli,
    config: &Config,
    formatter: &dyn Formatter,
) -> Result<(), CassioError> {
    let output_dir = cli
        .output
        .as_ref()
        .ok_or_else(|| CassioError::Other("--output is required for batch mode".into()))?;

    let files = discover::find_session_files(dir, None);
    let total = files.len();
    eprintln!("Found {total} session files");

    process_file_list(&files, output_dir, cli.force, formatter, cli.filter_dir.as_deref())?;

    cassio::git::auto_commit_and_push(
        output_dir,
        &format!("cassio batch ({})", Local::now().format("%Y-%m-%d")),
        &config.git,
    )?;

    Ok(())
}

/// Discover all installed tool sources and batch-process them into `--output`.
///
/// Uses `discover::discover_all_sources_with_config` to find source directories,
/// then calls `process_file_list` for each tool. Sources that don't exist on this
/// machine are silently skipped. Errors if no sources are found at all.
fn run_all_mode(cli: &Cli, config: &Config, formatter: &dyn Formatter) -> Result<(), CassioError> {
    let output_dir = cli
        .output
        .as_ref()
        .ok_or_else(|| CassioError::Other("--output is required for --all mode".into()))?;

    let sources = discover::discover_all_sources_with_config(&config.sources);
    if sources.is_empty() {
        eprintln!("No session directories found. Checked:");
        eprintln!("  Claude:         ~/.claude/projects");
        eprintln!("  Claude Desktop: ~/Library/Application Support/Claude/local-agent-mode-sessions");
        eprintln!("  Codex:          ~/.codex/sessions");
        eprintln!("  OpenCode:       ~/.local/share/opencode/storage");
        return Err(CassioError::Other("No sources found".into()));
    }

    let source_names: Vec<_> = sources.iter().map(|(t, _)| t.to_string()).collect();
    eprintln!(
        "Found {} source(s): {}",
        sources.len(),
        source_names.join(", ")
    );

    for (tool, path) in &sources {
        eprintln!("\nProcessing {} ({})...", tool, path.display());

        let files = discover::find_session_files(path, Some(*tool));
        let total = files.len();
        eprintln!("Found {total} session files");

        process_file_list(&files, output_dir, cli.force, formatter, cli.filter_dir.as_deref())?;
    }

    cassio::git::auto_commit_and_push(
        output_dir,
        &format!("cassio --all ({})", Local::now().format("%Y-%m-%d")),
        &config.git,
    )?;

    eprintln!("\nAll done.");
    Ok(())
}

/// Process a list of `(Tool, path)` pairs and write formatted transcripts to `output_dir`.
///
/// PHASE 1: PRE-FLIGHT CHECKS
/// Skip empty files (zero bytes) and files whose output is already up-to-date,
/// unless `force` is true.
///
/// PHASE 2: OUTPUT PATH DERIVATION
/// Compute the `YYYY-MM/filename.txt` path within `output_dir` using
/// `derive_output_path_for`. Create parent directories as needed.
///
/// PHASE 3: PARSING AND WRITING
/// Parse the session with the appropriate tool parser and write formatted output.
/// Sessions with no user or assistant messages are skipped (they contain only
/// system events and produce empty transcripts).
/// Parse failures are logged as warnings but do not abort the batch.
///
/// Progress is reported to stderr with a rolling counter every 100 files.
fn process_file_list(
    files: &[(Tool, PathBuf)],
    output_dir: &Path,
    force: bool,
    formatter: &dyn Formatter,
    filter_dir: Option<&Path>,
) -> Result<(), CassioError> {
    let total = files.len();
    let mut processed = 0u32;
    let mut skipped = 0u32;
    let mut up_to_date = 0u32;

    for (i, (tool, path)) in files.iter().enumerate() {
        if (i + 1) == 1 || (i + 1) % 100 == 0 {
            eprint!("\r  Processing {}/{}...", i + 1, total);
        }

        // Skip empty files
        if path.is_file() {
            if let Ok(meta) = fs::metadata(path) {
                if meta.len() == 0 {
                    skipped += 1;
                    continue;
                }
            }
        }

        let (folder, filename) = derive_output_path_for(*tool, path)?;
        let out_path = output_dir.join(&folder).join(&filename);

        if !force && is_up_to_date(path, &out_path) {
            up_to_date += 1;
            continue;
        }

        let parser: Box<dyn Parser> = match tool {
            Tool::Claude | Tool::ClaudeDesktop => Box::new(cassio::parser::claude::ClaudeParser),
            Tool::Codex => Box::new(cassio::parser::codex::CodexParser),
            Tool::OpenCode => Box::new(cassio::parser::opencode::OpenCodeParser),
        };

        match parser.parse_session(path) {
            Ok(session) => {
                if session.stats.user_messages == 0 && session.stats.assistant_messages == 0 {
                    skipped += 1;
                    continue;
                }

                if let Some(filter) = filter_dir {
                    let filter_str = filter.to_string_lossy();
                    if !session.metadata.project_path.starts_with(filter_str.as_ref()) {
                        skipped += 1;
                        continue;
                    }
                }

                if let Some(parent) = out_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut file = fs::File::create(&out_path)?;
                formatter.format(&session, &mut file)?;
                processed += 1;
            }
            Err(e) => {
                eprintln!("\r  warning: skipping {}: {e}", path.display());
                skipped += 1;
            }
        }
    }

    eprintln!(
        "\r  Done: {processed} processed, {skipped} skipped, {up_to_date} up-to-date     "
    );
    Ok(())
}

/// Compute the `(year-month-folder, filename)` output path for a session file.
///
/// OpenCode requires reading the session JSON to get a timestamp (since its session
/// IDs are opaque), so this function handles that case directly. Other tools delegate
/// to `discover::derive_output_path`.
fn derive_output_path_for(tool: Tool, path: &Path) -> Result<(String, String), CassioError> {
    match tool {
        Tool::OpenCode => {
            let session_id = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            let storage_dir = path.parent().and_then(|p| p.parent()).unwrap_or(path);

            let session_dir = storage_dir.join("session");
            if session_dir.is_dir() {
                if let Ok(entries) = fs::read_dir(&session_dir) {
                    for entry in entries.filter_map(|e| e.ok()) {
                        let session_file = entry.path().join(format!("{session_id}.json"));
                        if session_file.exists() {
                            if let Ok(content) = fs::read_to_string(&session_file) {
                                if let Ok(val) =
                                    serde_json::from_str::<serde_json::Value>(&content)
                                {
                                    if let Some(created) = val
                                        .get("time")
                                        .and_then(|t| t.get("created"))
                                        .and_then(|c| c.as_f64())
                                    {
                                        let dt = Utc
                                            .timestamp_opt(created as i64 / 1000, 0)
                                            .single()
                                            .unwrap_or_else(Utc::now);
                                        let folder =
                                            format!("{:04}-{:02}", dt.year(), dt.month());
                                        let ts = format!(
                                            "{:04}-{:02}-{:02}T{:02}-{:02}-{:02}",
                                            dt.year(),
                                            dt.month(),
                                            dt.day(),
                                            dt.hour(),
                                            dt.minute(),
                                            dt.second()
                                        );
                                        return Ok((folder, format!("{ts}-opencode.txt")));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Ok(("unknown".to_string(), format!("{session_id}-opencode.txt")))
        }
        _ => Ok(discover::derive_output_path(tool, path)),
    }
}

/// Return `true` when the output file is newer than (or the same age as) the input.
///
/// WHY: Comparing modification times lets batch mode skip already-processed files
/// without storing any external state. Returns `false` when either file is missing
/// or when modification times are unavailable (some filesystems do not support mtime).
fn is_up_to_date(input: &Path, output: &Path) -> bool {
    let input_meta = match fs::metadata(input) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let output_meta = match fs::metadata(output) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if let (Ok(input_modified), Ok(output_modified)) =
        (input_meta.modified(), output_meta.modified())
    {
        output_modified >= input_modified
    } else {
        false
    }
}
