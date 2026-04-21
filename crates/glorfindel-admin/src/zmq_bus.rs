/// Publishes campaign write events to a ZMQ PUB socket.
///
/// Any subscriber on tcp://127.0.0.1:5558 will receive JSON messages whenever
/// an agent writes a campaign file. Topic prefix is "campaign".
///
/// Message format: `campaign {"filename":"npcs.md","content":"...","agent":"..."}`
use futures::SinkExt;
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::state::{TaskEvent, TaskEventKind};

pub const ZMQ_CAMPAIGN_ADDR: &str = "tcp://127.0.0.1:5558";

pub async fn run_publisher(mut rx: broadcast::Receiver<TaskEvent>) {
    let ctx = tmq::Context::new();
    let mut sock = match tmq::publish(&ctx).bind(ZMQ_CAMPAIGN_ADDR) {
        Ok(s) => {
            info!(addr = ZMQ_CAMPAIGN_ADDR, "ZMQ campaign bus listening");
            s
        }
        Err(e) => {
            warn!(error = %e, "Failed to bind ZMQ campaign bus — campaign events won't be published");
            return;
        }
    };

    loop {
        let event = match rx.recv().await {
            Ok(e) => e,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        };

        match &event.kind {
            TaskEventKind::Complete { response } => {
                for action in &response.actions_taken {
                    if action.tool_call.tool_name == "campaign.write" {
                        let filename = action.tool_call.parameters
                            .get("filename")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let content = action.tool_call.parameters
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let payload = serde_json::json!({
                            "task_id": event.task_id,
                            "filename": filename,
                            "content": content,
                            "status": action.tool_result.status,
                        });
                        let payload_str = payload.to_string();
                        if let Err(e) = sock.send(vec!["campaign", payload_str.as_str()]).await {
                            warn!(error = %e, "Failed to publish campaign event to ZMQ");
                        } else {
                            info!(filename, "Published campaign write to ZMQ bus");
                        }
                    }
                }
            }
            TaskEventKind::PipelineStep { step, body } => {
                let payload = serde_json::json!({
                    "task_id": event.task_id,
                    "step": step,
                    "body": body,
                });
                let s = payload.to_string();
                if let Err(e) = sock.send(vec!["pipeline", s.as_str()]).await {
                    warn!(error = %e, "ZMQ pipeline step send failed");
                } else {
                    info!(step = step.as_str(), "ZMQ → pipeline");
                }
            }
            TaskEventKind::AgentSpawned { name, model, context } => {
                let payload = serde_json::json!({
                    "task_id": event.task_id,
                    "name": name, "model": model, "context": context,
                });
                let s = payload.to_string();
                let _ = sock.send(vec!["agent", s.as_str()]).await;
                info!(name = name.as_str(), model = model.as_str(), "ZMQ → agent spawned");
            }
            TaskEventKind::ToolCall { tool, input, output } => {
                let payload = serde_json::json!({
                    "task_id": event.task_id,
                    "tool": tool, "input": input, "output": output,
                });
                let s = payload.to_string();
                let _ = sock.send(vec!["tool", s.as_str()]).await;
                info!(tool = tool.as_str(), "ZMQ → tool call");
            }
            TaskEventKind::FileWrite { path, bytes } => {
                let payload = serde_json::json!({
                    "task_id": event.task_id,
                    "path": path, "bytes": bytes,
                });
                let s = payload.to_string();
                let _ = sock.send(vec!["file", s.as_str()]).await;
                info!(path = path.as_str(), bytes, "ZMQ → file write");
            }
            TaskEventKind::Started => {
                let payload = serde_json::json!({ "task_id": event.task_id });
                let s = payload.to_string();
                let _ = sock.send(vec!["task", s.as_str()]).await;
            }
            TaskEventKind::Failed { .. } => {}
        }
    }
}
