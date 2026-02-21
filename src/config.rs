//! Configuration loading and management for cassio.
//!
//! # Architecture overview
//!
//! Configuration lives in `~/.config/cassio/config.toml` and is purely optional.
//! When absent, every option falls back to a sensible default so that cassio works
//! out of the box without any setup.
//!
//! Config values flow into the rest of the system through two paths:
//! 1. **`Config::load()`** — used at runtime by the CLI to fill in defaults before
//!    processing sessions.
//! 2. **`get_value` / `set_value` / `unset_value`** — used by `cassio get/set/unset`
//!    subcommands to read and write individual keys from the live file.
//!
//! # Design philosophy
//!
//! CLI flags always override config values. The `run()` function in `main.rs` merges
//! them after loading config, so this module only needs to expose the raw config data
//! and the mutation helpers — it does not need to know about the CLI at all.
//!
//! # TRADE-OFFS
//!
//! - `toml_edit` is used instead of plain `toml` for the mutation helpers because it
//!   preserves comments and formatting in the user's config file. This adds a second
//!   TOML dependency but is worth it to avoid silently destroying hand-written comments.
//! - Source paths are stored as `Option<String>` rather than `Option<PathBuf>` so
//!   that tilde expansion happens at point-of-use rather than at parse time, making
//!   serialization round-trips lossless.

use std::path::PathBuf;

use serde::Deserialize;
use toml_edit::DocumentMut;

use crate::error::CassioError;

/// Git integration options from `[git]` table.
///
/// WHY: Isolating git options in a sub-struct keeps the top-level `Config` flat and
/// maps cleanly to the `[git]` TOML table, making the config file readable.
#[derive(Debug, Deserialize)]
pub struct GitConfig {
    /// Auto-commit output files after processing.
    #[serde(default)]
    pub commit: bool,
    /// Auto-push after committing.
    #[serde(default)]
    pub push: bool,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            commit: false,
            push: false,
        }
    }
}

/// Per-tool source path overrides from `[sources]` table.
///
/// WHY: Users who keep their AI tool data in non-standard locations (e.g., a
/// different drive or a shared network directory) need a way to point cassio at
/// the right place without patching the binary. Each field is `Option<String>`
/// so that an absent key means "use the default path" rather than "no path at all".
#[derive(Debug, Default, Deserialize)]
pub struct SourcesConfig {
    pub claude: Option<String>,
    pub claude_desktop: Option<String>,
    pub codex: Option<String>,
    pub opencode: Option<String>,
}

/// Top-level config deserialized from `~/.config/cassio/config.toml`.
///
/// All fields are optional. Missing fields fall back to built-in defaults, so a
/// config file with a single key is valid and common. The `Config::load()` method
/// returns `Config::default()` when the file is absent rather than erroring, so
/// cassio is always runnable without any configuration.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    /// Default output directory for transcripts and compaction files.
    pub output: Option<String>,
    /// Default output format: `"emoji-text"` or `"jsonl"`.
    pub format: Option<String>,
    /// Default model name passed to the LLM provider during compaction.
    pub model: Option<String>,
    /// LLM provider for compaction: `"ollama"`, `"claude"`, or `"codex"`.
    pub provider: Option<String>,
    #[serde(default)]
    pub git: GitConfig,
    pub sources: Option<SourcesConfig>,
}

impl Config {
    /// Load config from `~/.config/cassio/config.toml`.
    ///
    /// Returns `Config::default()` silently when the file is absent — cassio is
    /// designed to be zero-config, so a missing file is never an error. Parse
    /// failures also fall back to default to avoid breaking normal usage when a
    /// user has written an invalid value.
    pub fn load() -> Config {
        let Some(home) = dirs::home_dir() else {
            return Config::default();
        };
        let path = home.join(".config/cassio/config.toml");
        let Ok(content) = std::fs::read_to_string(&path) else {
            return Config::default();
        };
        toml::from_str(&content).unwrap_or_default()
    }

    /// Resolve the configured output path, expanding a leading `~` to the home directory.
    ///
    /// Returns `None` when no output path is configured, signalling to callers that
    /// they should require the user to supply `--output` on the command line.
    pub fn output_path(&self) -> Option<PathBuf> {
        self.output.as_deref().map(expand_tilde)
    }
}

/// Print a config value by dotted key (e.g. `"sources.claude"` or `"output"`).
///
/// Reads the live config file so that comments and formatting written by the user
/// are not disturbed. Errors if the key does not exist.
pub fn get_value(key: &str) -> Result<(), CassioError> {
    let content = read_config_file()?;
    let doc: DocumentMut = content
        .parse()
        .map_err(|e| CassioError::Other(format!("Failed to parse config: {e}")))?;

    let value = resolve_key(&doc, key);
    match value {
        Some(item) => {
            println!("{}", format_item(item));
            Ok(())
        }
        None => Err(CassioError::Other(format!("Key not found: {key}"))),
    }
}

/// Write a config value by dotted key (e.g. `cassio set git.commit true`).
///
/// Creates the config file and any intermediate TOML tables as needed. Values are
/// type-inferred from their string representation — `"true"` / `"false"` become
/// booleans, numeric strings become integers or floats, and everything else becomes
/// a string. This matches the most common user expectation without requiring type
/// annotations.
pub fn set_value(key: &str, value: &str) -> Result<(), CassioError> {
    let content = read_config_file().unwrap_or_default();
    let mut doc: DocumentMut = content
        .parse()
        .map_err(|e| CassioError::Other(format!("Failed to parse config: {e}")))?;

    let (table_path, field) = split_key(key)?;

    // Navigate/create intermediate tables
    let mut table = doc.as_table_mut();
    for segment in &table_path {
        if !table.contains_key(segment) {
            table.insert(segment, toml_edit::Item::Table(toml_edit::Table::new()));
        }
        table = table[segment]
            .as_table_mut()
            .ok_or_else(|| CassioError::Other(format!("'{segment}' is not a table")))?;
    }

    // Set the value with type inference
    let toml_value = infer_value(value);
    table.insert(&field, toml_edit::Item::Value(toml_value));

    write_config_file(&doc.to_string())?;
    Ok(())
}

/// Remove a config value by dotted key (`cassio unset <key>`).
///
/// Errors if the key does not exist, so the command gives clear feedback rather
/// than silently succeeding on a typo.
pub fn unset_value(key: &str) -> Result<(), CassioError> {
    let content = read_config_file()?;
    let mut doc: DocumentMut = content
        .parse()
        .map_err(|e| CassioError::Other(format!("Failed to parse config: {e}")))?;

    let (table_path, field) = split_key(key)?;

    let mut table = doc.as_table_mut();
    for segment in &table_path {
        table = table
            .get_mut(segment)
            .and_then(|item| item.as_table_mut())
            .ok_or_else(|| CassioError::Other(format!("Key not found: {key}")))?;
    }

    if table.remove(&field).is_none() {
        return Err(CassioError::Other(format!("Key not found: {key}")));
    }

    write_config_file(&doc.to_string())?;
    Ok(())
}

/// Print all config values in `key = value` format (`cassio get`).
///
/// Lists every leaf key in the config file using dotted notation so that the
/// output can be copy-pasted directly into `cassio set` commands.
pub fn list_values() -> Result<(), CassioError> {
    let content = read_config_file()?;
    let doc: DocumentMut = content
        .parse()
        .map_err(|e| CassioError::Other(format!("Failed to parse config: {e}")))?;

    let mut entries = Vec::new();
    collect_entries(doc.as_table(), "", &mut entries);

    if entries.is_empty() {
        eprintln!("No config values set.");
    } else {
        for (key, value) in entries {
            println!("{key} = {value}");
        }
    }
    Ok(())
}

/// Write the default config template to `~/.config/cassio/config.toml`.
///
/// All options are commented out so that the file documents what is available
/// without actually changing any behavior. Errors if the file already exists to
/// avoid silently overwriting user customizations.
pub fn init() -> Result<(), CassioError> {
    let path = config_path()?;
    if path.exists() {
        return Err(CassioError::Other(format!(
            "Config file already exists: {}",
            path.display()
        )));
    }

    let template = r#"# Cassio configuration
# See: cassio docs

# Default output directory for transcripts, dailies, and monthlies
# output = "~/transcripts"

# Default output format: "emoji-text" or "jsonl"
# format = "emoji-text"

# LLM provider for compaction: "ollama", "claude", or "codex"
# provider = "ollama"

# Default model name (passed to the selected provider)
# model = "llama3.1"

[git]
# Auto-commit output files after processing
# commit = false

# Auto-push after committing
# push = false

[sources]
# Override default log paths (leave commented to use defaults)
# claude = "~/.claude/projects"
# claude_desktop = "~/Library/Application Support/Claude/local-agent-mode-sessions"
# codex = "~/.codex/sessions"
# opencode = "~/.local/share/opencode/storage"
"#;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, template)?;

    eprintln!("Created config file: {}", path.display());
    eprintln!();
    eprintln!("Edit it directly, or use:");
    eprintln!("  cassio set output ~/transcripts");
    eprintln!("  cassio set git.commit true");
    eprintln!("  cassio get");

    Ok(())
}

// ── Private helpers ───────────────────────────────────────────────────────────
//
// These functions handle the low-level mechanics of reading, writing, and
// navigating the TOML document. They are kept private because callers should use
// the public API above rather than manipulating the document directly.

fn config_path() -> Result<PathBuf, CassioError> {
    let home =
        dirs::home_dir().ok_or_else(|| CassioError::Other("Cannot determine home directory".into()))?;
    Ok(home.join(".config/cassio/config.toml"))
}

fn read_config_file() -> Result<String, CassioError> {
    let path = config_path()?;
    std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CassioError::Other(format!("Config file not found: {}", path.display()))
        } else {
            CassioError::Io(e)
        }
    })
}

fn write_config_file(content: &str) -> Result<(), CassioError> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, content)?;
    Ok(())
}

/// Parse a dotted key into a table path and a leaf field name.
///
/// `"git.commit"` → `(["git"], "commit")`
/// `"output"` → `([], "output")`
///
/// WHY: Separating path traversal from leaf assignment makes `set_value` and
/// `unset_value` simpler — they navigate to the right subtable, then operate on
/// the leaf without extra string manipulation.
fn split_key(key: &str) -> Result<(Vec<String>, String), CassioError> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() || parts.iter().any(|p| p.is_empty()) {
        return Err(CassioError::Other(format!("Invalid key: {key}")));
    }
    let field = parts.last().unwrap().to_string();
    let table_path = parts[..parts.len() - 1]
        .iter()
        .map(|s| s.to_string())
        .collect();
    Ok((table_path, field))
}

/// Walk a dotted key path in a `toml_edit` document and return the matching item.
///
/// Returns `None` if any segment in the path is missing, making it safe to call
/// without prior existence checks.
fn resolve_key<'a>(doc: &'a DocumentMut, key: &str) -> Option<&'a toml_edit::Item> {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current: &toml_edit::Item = doc.as_item();
    for part in &parts {
        current = current.as_table_like()?.get(part)?;
    }
    Some(current)
}

/// Render a `toml_edit::Item` as a clean user-facing string.
///
/// Tables are expanded into `key = value` lines. This supports `cassio get git`
/// displaying all keys under the `[git]` table rather than requiring the user to
/// know each leaf key name.
fn format_item(item: &toml_edit::Item) -> String {
    match item {
        toml_edit::Item::Value(v) => match v {
            toml_edit::Value::String(s) => s.value().clone(),
            toml_edit::Value::Integer(i) => i.value().to_string(),
            toml_edit::Value::Float(f) => f.value().to_string(),
            toml_edit::Value::Boolean(b) => b.value().to_string(),
            other => other.to_string(),
        },
        toml_edit::Item::Table(t) => {
            let mut entries = Vec::new();
            collect_entries(t, "", &mut entries);
            entries
                .iter()
                .map(|(k, v)| format!("{k} = {v}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
        other => other.to_string(),
    }
}

/// Recursively walk a TOML table and collect all leaf values as `(key, value)` pairs.
///
/// WHY: Flattening the tree into dotted-key entries makes the `cassio get` output
/// immediately usable as `cassio set` commands, giving users a round-trippable view
/// of their config.
fn collect_entries(table: &toml_edit::Table, prefix: &str, out: &mut Vec<(String, String)>) {
    for (key, item) in table.iter() {
        let full_key = if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{prefix}.{key}")
        };
        match item {
            toml_edit::Item::Value(v) => {
                let display = format_value(v);
                out.push((full_key, display));
            }
            toml_edit::Item::Table(t) => {
                collect_entries(t, &full_key, out);
            }
            _ => {}
        }
    }
}

/// Render a TOML scalar value as a clean string without `toml_edit` decoration.
///
/// WHY: `toml_edit` preserves whitespace decoration around values (e.g., ` "hello" `).
/// This helper strips that so display output looks like a normal TOML literal.
fn format_value(v: &toml_edit::Value) -> String {
    match v {
        toml_edit::Value::String(s) => format!("\"{}\"", s.value()),
        toml_edit::Value::Integer(i) => i.value().to_string(),
        toml_edit::Value::Float(f) => f.value().to_string(),
        toml_edit::Value::Boolean(b) => b.value().to_string(),
        other => other.to_string(),
    }
}

/// Infer a TOML value type from a CLI string argument.
///
/// Precedence: boolean → integer → float (only when the string contains `.`) → string.
///
/// WHY: Users run `cassio set git.commit true` and expect the stored value to be a
/// boolean, not the string `"true"`. Explicit type annotation would be more correct
/// but is far less ergonomic for a config CLI.
///
/// EDGE: `"3.0"` without a decimal point is treated as integer `3`, not float.
/// This matches the most common user expectation for version-like values.
fn infer_value(s: &str) -> toml_edit::Value {
    if s == "true" {
        return toml_edit::Value::from(true);
    }
    if s == "false" {
        return toml_edit::Value::from(false);
    }
    if let Ok(n) = s.parse::<i64>() {
        return toml_edit::Value::from(n);
    }
    if let Ok(f) = s.parse::<f64>() {
        if s.contains('.') {
            return toml_edit::Value::from(f);
        }
    }
    toml_edit::Value::from(s)
}

impl SourcesConfig {
    /// Resolve the configured Claude Code source path, expanding `~`.
    pub fn claude_path(&self) -> Option<PathBuf> {
        self.claude.as_deref().map(expand_tilde)
    }

    /// Resolve the configured Codex source path, expanding `~`.
    pub fn codex_path(&self) -> Option<PathBuf> {
        self.codex.as_deref().map(expand_tilde)
    }

    /// Resolve the configured Claude Desktop source path, expanding `~`.
    pub fn claude_desktop_path(&self) -> Option<PathBuf> {
        self.claude_desktop.as_deref().map(expand_tilde)
    }

    /// Resolve the configured OpenCode source path, expanding `~`.
    pub fn opencode_path(&self) -> Option<PathBuf> {
        self.opencode.as_deref().map(expand_tilde)
    }
}

/// Expand a leading `~` or `~/` prefix to the user's home directory.
///
/// WHY: TOML config files written by users naturally contain `~/path` notation.
/// Storing paths as strings and expanding at point-of-use means the config file
/// remains human-readable and survives being copied between machines.
///
/// EDGE: A bare `"~"` (no trailing slash) is expanded to the home directory itself.
/// Paths without a leading `~` are returned unchanged, so absolute and relative
/// paths both work.
pub(crate) fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- split_key tests ---

    #[test]
    fn test_split_key_simple() {
        let (table, field) = split_key("output").unwrap();
        assert!(table.is_empty());
        assert_eq!(field, "output");
    }

    #[test]
    fn test_split_key_dotted() {
        let (table, field) = split_key("git.commit").unwrap();
        assert_eq!(table, vec!["git"]);
        assert_eq!(field, "commit");
    }

    #[test]
    fn test_split_key_deeply_nested() {
        let (table, field) = split_key("a.b.c").unwrap();
        assert_eq!(table, vec!["a", "b"]);
        assert_eq!(field, "c");
    }

    #[test]
    fn test_split_key_empty_segment_errors() {
        assert!(split_key("a..b").is_err());
        assert!(split_key(".a").is_err());
        assert!(split_key("a.").is_err());
    }

    // --- infer_value tests ---

    #[test]
    fn test_infer_value_true() {
        let v = infer_value("true");
        assert_eq!(v.as_bool(), Some(true));
    }

    #[test]
    fn test_infer_value_false() {
        let v = infer_value("false");
        assert_eq!(v.as_bool(), Some(false));
    }

    #[test]
    fn test_infer_value_integer() {
        let v = infer_value("42");
        assert_eq!(v.as_integer(), Some(42));
    }

    #[test]
    fn test_infer_value_float() {
        let v = infer_value("3.14");
        let f = v.as_float().unwrap();
        assert!((f - 3.14).abs() < 0.001);
    }

    #[test]
    fn test_infer_value_string() {
        let v = infer_value("hello world");
        assert_eq!(v.as_str(), Some("hello world"));
    }

    // --- expand_tilde tests ---

    #[test]
    fn test_expand_tilde_with_path() {
        let result = expand_tilde("~/projects");
        let home = dirs::home_dir().unwrap();
        assert_eq!(result, home.join("projects"));
    }

    #[test]
    fn test_expand_tilde_bare() {
        let result = expand_tilde("~");
        let home = dirs::home_dir().unwrap();
        assert_eq!(result, home);
    }

    #[test]
    fn test_expand_tilde_absolute_unchanged() {
        let result = expand_tilde("/absolute/path");
        assert_eq!(result, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_expand_tilde_relative_unchanged() {
        let result = expand_tilde("relative/path");
        assert_eq!(result, PathBuf::from("relative/path"));
    }

    // --- Config deserialization ---

    #[test]
    fn test_config_deserialize() {
        let toml_str = r#"
output = "~/transcripts"
format = "emoji-text"

[git]
commit = true
push = false

[sources]
claude = "~/.claude/projects"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.output.as_deref(), Some("~/transcripts"));
        assert_eq!(config.format.as_deref(), Some("emoji-text"));
        assert!(config.git.commit);
        assert!(!config.git.push);
        assert_eq!(config.sources.as_ref().unwrap().claude.as_deref(), Some("~/.claude/projects"));
    }

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert!(config.output.is_none());
        assert!(!config.git.commit);
        assert!(!config.git.push);
    }

    #[test]
    fn test_config_output_path_expands_tilde() {
        let config = Config {
            output: Some("~/transcripts".to_string()),
            ..Default::default()
        };
        let path = config.output_path().unwrap();
        let home = dirs::home_dir().unwrap();
        assert_eq!(path, home.join("transcripts"));
    }

    // --- resolve_key tests ---

    #[test]
    fn test_resolve_key_top_level() {
        let doc: toml_edit::DocumentMut = "output = \"test\"".parse().unwrap();
        let item = resolve_key(&doc, "output");
        assert!(item.is_some());
    }

    #[test]
    fn test_resolve_key_nested() {
        let doc: toml_edit::DocumentMut = "[git]\ncommit = true".parse().unwrap();
        let item = resolve_key(&doc, "git.commit");
        assert!(item.is_some());
    }

    #[test]
    fn test_resolve_key_missing() {
        let doc: toml_edit::DocumentMut = "output = \"test\"".parse().unwrap();
        let item = resolve_key(&doc, "nonexistent");
        assert!(item.is_none());
    }
}
