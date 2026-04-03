use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("DDS error: {0}")]
    Dds(String),

    #[error("ZMQ error: {0}")]
    Zmq(String),

    #[error("channel closed")]
    ChannelClosed,

    #[error("connection failed: {0}")]
    Connection(String),

    #[error("timeout waiting for message")]
    Timeout,
}
