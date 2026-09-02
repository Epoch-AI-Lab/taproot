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

    #[error("mount failed: {0}")]
    Mount(String),

    #[error("baseline not found: {0}")]
    BaselineMissing(String),

    #[error("drift detected: {breaking} breaking, {warning} warning")]
    Drift { breaking: usize, warning: usize },

    #[error("invalid hash: {0}")]
    InvalidHash(String),

    #[error("object not found: {0}")]
    ObjectNotFound(String),

    #[error("ref not found: {repo}/{branch}")]
    RefNotFound { repo: String, branch: String },
}
