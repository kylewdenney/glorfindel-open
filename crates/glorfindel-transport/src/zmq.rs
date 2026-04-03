use async_trait::async_trait;
use glorfindel_schemas::{MessageEnvelope, ToolCall, ToolResult};
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use crate::error::TransportError;
use crate::traits::DataPlane;

/// ZeroMQ-backed data plane implementation using tmq.
///
/// Uses PUSH/PULL for directed tool call dispatch and PUB/SUB for
/// result broadcasting. Optimized for throughput over reliability.
pub struct ZmqDataPlane {
    /// ZMQ endpoint for tool calls (PUSH/PULL).
    tool_call_endpoint: String,
    /// ZMQ endpoint for tool results (PUB/SUB).
    tool_result_endpoint: String,
}

impl ZmqDataPlane {
    pub fn new(tool_call_endpoint: impl Into<String>, tool_result_endpoint: impl Into<String>) -> Self {
        let tool_call_endpoint = tool_call_endpoint.into();
        let tool_result_endpoint = tool_result_endpoint.into();
        info!(
            tool_call_endpoint = %tool_call_endpoint,
            tool_result_endpoint = %tool_result_endpoint,
            "Creating ZMQ data plane"
        );
        Self {
            tool_call_endpoint,
            tool_result_endpoint,
        }
    }

    fn serialize<T: serde::Serialize>(msg: &T) -> Result<Vec<u8>, TransportError> {
        rmp_serde::to_vec(msg).map_err(|e| TransportError::Serialization(e.to_string()))
    }

    fn deserialize<T: for<'de> serde::Deserialize<'de>>(data: &[u8]) -> Result<T, TransportError> {
        rmp_serde::from_slice(data).map_err(|e| TransportError::Serialization(e.to_string()))
    }
}

#[async_trait]
impl DataPlane for ZmqDataPlane {
    async fn send_tool_call(
        &self,
        call: MessageEnvelope<ToolCall>,
    ) -> Result<(), TransportError> {
        let data = Self::serialize(&call)?;
        let context = tmq::Context::new();
        let socket = tmq::push(&context)
            .connect(&self.tool_call_endpoint)
            .map_err(|e| TransportError::Zmq(e.to_string()))?;

        let mut multipart = tmq::Multipart::default();
        multipart.push_back(tmq::Message::from(data.as_slice()));
        use futures::SinkExt;
        let mut socket = socket;
        socket
            .send(multipart)
            .await
            .map_err(|e| TransportError::Zmq(e.to_string()))?;

        debug!(tool = %call.payload.tool_name, "Sent tool call via ZMQ");
        Ok(())
    }

    async fn receive_tool_calls(
        &self,
    ) -> Result<mpsc::Receiver<MessageEnvelope<ToolCall>>, TransportError> {
        let (tx, rx) = mpsc::channel(256);
        let endpoint = self.tool_call_endpoint.clone();

        tokio::spawn(async move {
            let context = tmq::Context::new();
            let socket = match tmq::pull(&context).bind(&endpoint) {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to bind ZMQ PULL socket: {e}");
                    return;
                }
            };

            use futures::StreamExt;
            let mut socket = socket;
            while let Some(result) = socket.next().await {
                match result {
                    Ok(multipart) => {
                        for msg in multipart.iter() {
                            match ZmqDataPlane::deserialize::<MessageEnvelope<ToolCall>>(msg) {
                                Ok(envelope) => {
                                    if tx.send(envelope).await.is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to deserialize tool call: {e}");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("ZMQ receive error: {e}");
                    }
                }
            }
        });

        Ok(rx)
    }

    async fn send_tool_result(
        &self,
        result: MessageEnvelope<ToolResult>,
    ) -> Result<(), TransportError> {
        let data = Self::serialize(&result)?;
        let context = tmq::Context::new();
        let socket = tmq::publish(&context)
            .connect(&self.tool_result_endpoint)
            .map_err(|e| TransportError::Zmq(e.to_string()))?;

        // Topic prefix for filtering
        let topic = format!("result.{}", result.payload.task_id);
        let mut multipart = tmq::Multipart::default();
        multipart.push_back(tmq::Message::from(topic.as_bytes()));
        multipart.push_back(tmq::Message::from(data.as_slice()));

        use futures::SinkExt;
        let mut socket = socket;
        socket
            .send(multipart)
            .await
            .map_err(|e| TransportError::Zmq(e.to_string()))?;

        debug!(tool = %result.payload.tool_name, "Sent tool result via ZMQ");
        Ok(())
    }

    async fn receive_tool_results(
        &self,
    ) -> Result<mpsc::Receiver<MessageEnvelope<ToolResult>>, TransportError> {
        let (tx, rx) = mpsc::channel(256);
        let endpoint = self.tool_result_endpoint.clone();

        tokio::spawn(async move {
            let context = tmq::Context::new();
            let socket = match tmq::subscribe(&context)
                .connect(&endpoint)
            {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to connect ZMQ SUB socket: {e}");
                    return;
                }
            };

            let socket = match socket.subscribe(b"result.") {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to subscribe to result topic: {e}");
                    return;
                }
            };

            use futures::StreamExt;
            let mut socket = socket;
            while let Some(result) = socket.next().await {
                match result {
                    Ok(multipart) => {
                        // Skip topic frame (index 0), data is in frame 1
                        if let Some(data_frame) = multipart.iter().nth(1) {
                            match ZmqDataPlane::deserialize::<MessageEnvelope<ToolResult>>(
                                data_frame,
                            ) {
                                Ok(envelope) => {
                                    if tx.send(envelope).await.is_err() {
                                        return;
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to deserialize tool result: {e}");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("ZMQ receive error: {e}");
                    }
                }
            }
        });

        Ok(rx)
    }
}
