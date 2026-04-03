use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("missing required parameter: {0}")]
    MissingParameter(String),

    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    #[error("permission denied: requires {0:?}")]
    PermissionDenied(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
