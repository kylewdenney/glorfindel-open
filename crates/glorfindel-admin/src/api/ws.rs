use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::IntoResponse,
};
use glorfindel_schemas::types::Status;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::state::{AppState, TaskEvent, TaskEventKind};

pub async fn task_stream(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, task_id))
}

/// Broadcasts all PipelineStep events across every task — connects the DM Dashboard bus feed.
pub async fn pipeline_stream(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_pipeline_socket(socket, state))
}

async fn handle_pipeline_socket(mut socket: WebSocket, state: Arc<AppState>) {
    let mut rx = state.task_events.subscribe();
    loop {
        match rx.recv().await {
            Ok(event) => {
                // Forward every event kind — the dashboard renders each type differently
                if let Ok(msg) = serde_json::to_string(&event) {
                    if socket.send(Message::Text(msg)).await.is_err() {
                        break;
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>, task_id: Uuid) {
    // If task already finished, send the terminal event immediately and close.
    {
        let tasks = state.tasks.read().await;
        if let Some(record) = tasks.get(&task_id) {
            let terminal: Option<TaskEvent> = match &record.status {
                Status::Complete => record.response.clone().map(|r| TaskEvent {
                    task_id,
                    kind: TaskEventKind::Complete { response: r },
                }),
                Status::Failed => Some(TaskEvent {
                    task_id,
                    kind: TaskEventKind::Failed {
                        message: record.error.clone().unwrap_or_default(),
                    },
                }),
                _ => None,
            };
            if let Some(event) = terminal {
                if let Ok(msg) = serde_json::to_string(&event) {
                    let _ = socket.send(Message::Text(msg)).await;
                }
                return;
            }
        }
    }

    // Send "started" so the client knows we're connected and running.
    let started = serde_json::to_string(&TaskEvent {
        task_id,
        kind: TaskEventKind::Started,
    })
    .unwrap_or_default();
    if socket.send(Message::Text(started)).await.is_err() {
        return;
    }

    let mut rx = state.task_events.subscribe();
    loop {
        match rx.recv().await {
            Ok(event) if event.task_id == task_id => {
                let terminal = matches!(
                    event.kind,
                    TaskEventKind::Complete { .. } | TaskEventKind::Failed { .. }
                );
                if let Ok(msg) = serde_json::to_string(&event) {
                    if socket.send(Message::Text(msg)).await.is_err() {
                        break;
                    }
                }
                if terminal {
                    break;
                }
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}
