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
            let config = Config::load();
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
            let config = Config::load();
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
                            process_file_list(&files, &output_dir, cli.force, &*formatter)?;
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
    let config = Config::load();

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
        Some(ref path) if path.is_file() => run_single_file(path, &*formatter),
        Some(ref path) => Err(CassioError::Other(format!(
            "Path not found: {}",
            path.display()
        ))),
        None => run_stdin(&*formatter),
    }
}

fn run_single_file(path: &Path, formatter: &dyn Formatter) -> Result<(), CassioError> {
    let parser = cassio::parser::detect_parser(path)?;
    let session = parser.parse_session(path)?;
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    formatter.format(&session, &mut writer)?;
    Ok(())
}

fn run_stdin(formatter: &dyn Formatter) -> Result<(), CassioError> {
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

    let stdout = io::stdout();
    let mut writer = stdout.lock();
    formatter.format(&session, &mut writer)?;
    Ok(())
}

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

    process_file_list(&files, output_dir, cli.force, formatter)?;

    cassio::git::auto_commit_and_push(
        output_dir,
        &format!("cassio batch ({})", Local::now().format("%Y-%m-%d")),
        &config.git,
    )?;

    Ok(())
}

fn run_all_mode(cli: &Cli, config: &Config, formatter: &dyn Formatter) -> Result<(), CassioError> {
    let output_dir = cli
        .output
        .as_ref()
        .ok_or_else(|| CassioError::Other("--output is required for --all mode".into()))?;

    let sources = discover::discover_all_sources_with_config(&config.sources);
    if sources.is_empty() {
        eprintln!("No session directories found. Checked:");
        eprintln!("  Claude:   ~/.claude/projects");
        eprintln!("  Codex:    ~/.codex/sessions");
        eprintln!("  OpenCode: ~/.local/share/opencode/storage");
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

        process_file_list(&files, output_dir, cli.force, formatter)?;
    }

    cassio::git::auto_commit_and_push(
        output_dir,
        &format!("cassio --all ({})", Local::now().format("%Y-%m-%d")),
        &config.git,
    )?;

    eprintln!("\nAll done.");
    Ok(())
}

fn process_file_list(
    files: &[(Tool, PathBuf)],
    output_dir: &Path,
    force: bool,
    formatter: &dyn Formatter,
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
            Tool::Claude => Box::new(cassio::parser::claude::ClaudeParser),
            Tool::Codex => Box::new(cassio::parser::codex::CodexParser),
            Tool::OpenCode => Box::new(cassio::parser::opencode::OpenCodeParser),
        };

        match parser.parse_session(path) {
            Ok(session) => {
                if session.stats.user_messages == 0 && session.stats.assistant_messages == 0 {
                    skipped += 1;
                    continue;
                }

                fs::create_dir_all(out_path.parent().unwrap())?;
                let mut file = fs::File::create(&out_path)?;
                formatter.format(&session, &mut file)?;
                processed += 1;
            }
            Err(_) => {
                skipped += 1;
            }
        }
    }

    eprintln!(
        "\r  Done: {processed} processed, {skipped} skipped, {up_to_date} up-to-date     "
    );
    Ok(())
}

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
