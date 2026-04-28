use async_trait::async_trait;
use glorfindel_schemas::{MessageEnvelope, ToolCall, ToolResult};
use tokio::sync::{mpsc, Mutex};
use std::sync::Arc;
use tracing::debug;

use crate::error::TransportError;
use crate::traits::DataPlane;

/// In-process `DataPlane` backed by tokio MPSC channels.
///
/// Identical interface to `ZmqDataPlane` but works within a single process.
/// Useful for same-process agent-executor pairs and integration testing.
/// Swap for `ZmqDataPlane` when splitting into separate processes.
///
/// Usage: create one `ChannelDataPlane`, share it via `Arc` between the agent
/// and its tool executor. The agent calls `receive_tool_results()` once to
/// take the result receiver; the executor calls `receive_tool_calls()` once
/// to take the call receiver. Both sides use `send_*` through the shared Arc.
pub struct ChannelDataPlane {
    tool_call_tx: mpsc::Sender<MessageEnvelope<ToolCall>>,
    tool_call_rx: Arc<Mutex<Option<mpsc::Receiver<MessageEnvelope<ToolCall>>>>>,
    tool_result_tx: mpsc::Sender<MessageEnvelope<ToolResult>>,
    tool_result_rx: Arc<Mutex<Option<mpsc::Receiver<MessageEnvelope<ToolResult>>>>>,
}

impl ChannelDataPlane {
    pub fn new() -> Self {
        let (tool_call_tx, tool_call_rx) = mpsc::channel(256);
        let (tool_result_tx, tool_result_rx) = mpsc::channel(256);
        Self {
            tool_call_tx,
            tool_call_rx: Arc::new(Mutex::new(Some(tool_call_rx))),
            tool_result_tx,
            tool_result_rx: Arc::new(Mutex::new(Some(tool_result_rx))),
        }
    }
}

#[async_trait]
impl DataPlane for ChannelDataPlane {
    async fn send_tool_call(
        &self,
        call: MessageEnvelope<ToolCall>,
    ) -> Result<(), TransportError> {
        debug!(tool = %call.payload.tool_name, "Channel: send_tool_call");
        self.tool_call_tx
            .send(call)
            .await
            .map_err(|_| TransportError::ChannelClosed)
    }

    async fn receive_tool_calls(
        &self,
    ) -> Result<mpsc::Receiver<MessageEnvelope<ToolCall>>, TransportError> {
        self.tool_call_rx
            .lock()
            .await
            .take()
            .ok_or(TransportError::ChannelClosed)
    }

    async fn send_tool_result(
        &self,
        result: MessageEnvelope<ToolResult>,
    ) -> Result<(), TransportError> {
        debug!(tool = %result.payload.tool_name, "Channel: send_tool_result");
        self.tool_result_tx
            .send(result)
            .await
            .map_err(|_| TransportError::ChannelClosed)
    }

    async fn receive_tool_results(
        &self,
    ) -> Result<mpsc::Receiver<MessageEnvelope<ToolResult>>, TransportError> {
        self.tool_result_rx
            .lock()
            .await
            .take()
            .ok_or(TransportError::ChannelClosed)
    }
}
