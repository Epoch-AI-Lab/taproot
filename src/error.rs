use thiserror::Error;

#[derive(Debug, Error)]
pub enum TaprootError {
    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("invalid hash: expected {expected}, got {got}")]
    HashMismatch { expected: String, got: String },

    #[error("signature verification failed")]
    InvalidSignature,

    #[error("invalid key: {0}")]
    InvalidKey(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
