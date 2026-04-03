use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("inference failed: {0}")]
    InferenceFailed(String),

    #[error("model not available: {0}")]
    ModelNotAvailable(String),

    #[error("tool execution failed: {0}")]
    ToolFailed(String),

    #[error("max iterations exceeded")]
    MaxIterationsExceeded,

    #[error("task timed out")]
    Timeout,

    #[error("transport error: {0}")]
    Transport(#[from] glorfindel_transport::TransportError),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
}
