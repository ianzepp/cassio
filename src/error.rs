use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CassioError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error at {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },

    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("Unknown format for file: {0}")]
    UnknownFormat(PathBuf),

    #[error("Empty session: {0}")]
    EmptySession(PathBuf),

    #[error("{0}")]
    Other(String),
}
