use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Generic message envelope wrapping any payload with routing metadata.
///
/// Every message in the Glorfindel system is wrapped in an envelope that
/// provides tracing, correlation, and source identification. This is the
/// fundamental unit of communication across both the DDS control plane
/// and ZMQ data plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEnvelope<T> {
    /// Unique identifier for this message.
    pub id: Uuid,
    /// Timestamp when the message was created.
    pub timestamp: DateTime<Utc>,
    /// Correlation ID linking related messages (e.g., a ToolResult back to its ToolCall).
    pub correlation_id: Option<Uuid>,
    /// Identifier of the system component that produced this message.
    pub source: String,
    /// The actual message payload.
    pub payload: T,
}

impl<T: Serialize> MessageEnvelope<T>
where
    T: Serialize,
{
    /// Create a new envelope wrapping the given payload.
    pub fn new(source: impl Into<String>, payload: T) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            correlation_id: None,
            source: source.into(),
            payload,
        }
    }

    /// Create an envelope that correlates to an existing message.
    pub fn reply_to(original_id: Uuid, source: impl Into<String>, payload: T) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            correlation_id: Some(original_id),
            source: source.into(),
            payload,
        }
    }
}
