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
    let v = infer_value("2.5");
    let f = v.as_float().unwrap();
    assert!((f - 2.5).abs() < 0.001);
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
training_output = "~/training"
format = "emoji-text"
provider = "openai"
base_url = "http://127.0.0.1:18173/v1"
max_input_bytes = 102400
chunk_timeout_secs = 300
max_retries = 3

[embedding]
auto_index = true
provider = "ollama"
model = "cassio-embedding"
base_url = "http://127.0.0.1:11434"
include_training = true
include_paths = false
batch_size = 8
timeout_secs = 30

[git]
commit = true
push = false

[sources]
claude = "~/.claude/projects"
pi = "~/.pi/agent/sessions"
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.output.as_deref(), Some("~/transcripts"));
    assert_eq!(config.training_output.as_deref(), Some("~/training"));
    assert_eq!(config.format.as_deref(), Some("emoji-text"));
    assert_eq!(config.provider.as_deref(), Some("openai"));
    assert_eq!(
        config.base_url.as_deref(),
        Some("http://127.0.0.1:18173/v1")
    );
    assert_eq!(config.max_input_bytes, Some(102400));
    assert_eq!(config.chunk_timeout_secs, Some(300));
    assert_eq!(config.max_retries, Some(3));
    let embedding = config.embedding.as_ref().unwrap();
    assert!(embedding.auto_index);
    assert_eq!(embedding.provider.as_deref(), Some("ollama"));
    assert_eq!(embedding.model.as_deref(), Some("cassio-embedding"));
    assert_eq!(
        embedding.base_url.as_deref(),
        Some("http://127.0.0.1:11434")
    );
    assert!(embedding.include_training);
    assert!(!embedding.include_paths);
    assert_eq!(embedding.batch_size, Some(8));
    assert_eq!(embedding.timeout_secs, Some(30));
    assert!(config.git.commit);
    assert!(!config.git.push);
    assert_eq!(
        config.sources.as_ref().unwrap().claude.as_deref(),
        Some("~/.claude/projects")
    );
    assert_eq!(
        config.sources.as_ref().unwrap().pi.as_deref(),
        Some("~/.pi/agent/sessions")
    );
}

#[test]
fn test_config_default() {
    let config = Config::default();
    assert!(config.output.is_none());
    assert!(!config.git.commit);
    assert!(!config.git.push);
    assert!(config.max_input_bytes.is_none());
    assert!(config.chunk_timeout_secs.is_none());
    assert!(config.max_retries.is_none());
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

#[test]
fn test_config_training_output_path_expands_tilde() {
    let config = Config {
        training_output: Some("~/training".to_string()),
        ..Default::default()
    };
    let path = config.training_output_path().unwrap();
    let home = dirs::home_dir().unwrap();
    assert_eq!(path, home.join("training"));
}

#[test]
fn test_config_training_output_path_none_when_unset() {
    let config = Config::default();
    assert!(config.training_output_path().is_none());
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
