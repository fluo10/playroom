use thiserror::Error;

#[derive(Debug, Error)]
pub enum NetworkError {
    #[error("iroh error: {0}")]
    Iroh(#[from] anyhow::Error),
    #[error("framing error: {0}")]
    Framing(String),
    #[error("connection closed")]
    Closed,
    #[error("write error: {0}")]
    Write(#[from] iroh::endpoint::WriteError),
    #[error("read error: {0}")]
    ReadExact(#[from] iroh::endpoint::ReadExactError),
}
