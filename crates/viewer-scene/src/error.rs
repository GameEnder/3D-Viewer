// crates/viewer-scene/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("{0} files are not supported by this viewer")]
    UnsupportedFormat(String),
    #[error("{format} parse error: {detail}")]
    Parse {
        format: &'static str,
        detail: String,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
