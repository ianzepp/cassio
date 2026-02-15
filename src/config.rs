use std::path::PathBuf;

use serde::Deserialize;
use toml_edit::DocumentMut;

use crate::error::CassioError;

/// Git integration options from `[git]` table.
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
#[derive(Debug, Default, Deserialize)]
pub struct SourcesConfig {
    pub claude: Option<String>,
    pub codex: Option<String>,
    pub opencode: Option<String>,
}

/// Top-level config loaded from `~/.config/cassio/config.toml`.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    pub output: Option<String>,
    pub format: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    #[serde(default)]
    pub git: GitConfig,
    pub sources: Option<SourcesConfig>,
}

impl Config {
    /// Load config from `~/.config/cassio/config.toml`.
    /// Returns `Config::default()` if the file is missing.
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

    /// Resolve the output path with tilde expansion.
    pub fn output_path(&self) -> Option<PathBuf> {
        self.output.as_deref().map(expand_tilde)
    }
}

/// Get a config value by dotted key (e.g. "sources.claude" or "output").
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

/// Set a config value by dotted key (e.g. "git.autocommit true").
/// Creates the file and any intermediate tables as needed.
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

/// Unset (remove) a config value by dotted key.
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

/// List all config values.
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

/// Create a default config file with all options commented out.
/// Errors if the file already exists.
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

// --- helpers ---

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

/// Split "git.autocommit" into (["git"], "autocommit") or "output" into ([], "output").
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

/// Resolve a dotted key to an item in the document.
fn resolve_key<'a>(doc: &'a DocumentMut, key: &str) -> Option<&'a toml_edit::Item> {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current: &toml_edit::Item = doc.as_item();
    for part in &parts {
        current = current.as_table_like()?.get(part)?;
    }
    Some(current)
}

/// Format a toml_edit::Item for display.
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

/// Recursively collect "key = value" entries for display.
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

/// Format a TOML value for clean display (no decoration whitespace).
fn format_value(v: &toml_edit::Value) -> String {
    match v {
        toml_edit::Value::String(s) => format!("\"{}\"", s.value()),
        toml_edit::Value::Integer(i) => i.value().to_string(),
        toml_edit::Value::Float(f) => f.value().to_string(),
        toml_edit::Value::Boolean(b) => b.value().to_string(),
        other => other.to_string(),
    }
}

/// Infer a TOML value type from a string input.
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
    pub fn claude_path(&self) -> Option<PathBuf> {
        self.claude.as_deref().map(expand_tilde)
    }

    pub fn codex_path(&self) -> Option<PathBuf> {
        self.codex.as_deref().map(expand_tilde)
    }

    pub fn opencode_path(&self) -> Option<PathBuf> {
        self.opencode.as_deref().map(expand_tilde)
    }
}

/// Expand a leading `~` to the user's home directory.
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
