use std::collections::HashMap;
use std::sync::Arc;
use chrono::{DateTime, Utc};
use glorfindel_schemas::agent::AgentResponse;
use glorfindel_schemas::task::TaskRequest;
use glorfindel_schemas::types::Status;
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

/// Tracks task lifecycle from submission through completion.
///
/// The TaskManager maintains the state of all active tasks, tracks
/// parent-child relationships for delegated sub-tasks, and provides
/// a unified view of system activity.
pub struct TaskManager {
    tasks: Arc<RwLock<HashMap<Uuid, TaskRecord>>>,
}

/// A record of a task's current state and history.
#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub task_id: Uuid,
    pub parent_task_id: Option<Uuid>,
    pub intent: String,
    pub status: Status,
    pub assigned_agent: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub child_tasks: Vec<Uuid>,
    pub response: Option<AgentResponse>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a new task.
    pub async fn register_task(&self, task: &TaskRequest) {
        let record = TaskRecord {
            task_id: task.task_id,
            parent_task_id: task.parent_task_id,
            intent: task.intent.clone(),
            status: Status::Pending,
            assigned_agent: None,
            created_at: Utc::now(),
            completed_at: None,
            child_tasks: Vec::new(),
            response: None,
        };

        // If this is a child task, register it with the parent
        if let Some(parent_id) = task.parent_task_id {
            let mut tasks = self.tasks.write().await;
            if let Some(parent) = tasks.get_mut(&parent_id) {
                parent.child_tasks.push(task.task_id);
            }
            tasks.insert(task.task_id, record);
        } else {
            self.tasks.write().await.insert(task.task_id, record);
        }

        info!(task_id = %task.task_id, intent = %task.intent, "Task registered");
    }

    /// Mark a task as assigned to an agent.
    pub async fn assign_task(&self, task_id: Uuid, agent_id: &str) {
        if let Some(task) = self.tasks.write().await.get_mut(&task_id) {
            task.status = Status::InProgress;
            task.assigned_agent = Some(agent_id.to_string());
            info!(task_id = %task_id, agent_id, "Task assigned");
        }
    }

    /// Record a task's completion.
    pub async fn complete_task(&self, task_id: Uuid, response: AgentResponse) {
        if let Some(task) = self.tasks.write().await.get_mut(&task_id) {
            task.status = response.status.clone();
            task.completed_at = Some(Utc::now());
            task.response = Some(response);
            info!(task_id = %task_id, "Task completed");
        } else {
            warn!(task_id = %task_id, "Attempted to complete unknown task");
        }
    }

    /// Get a task's current record.
    pub async fn get_task(&self, task_id: Uuid) -> Option<TaskRecord> {
        self.tasks.read().await.get(&task_id).cloned()
    }

    /// List all active (non-completed) tasks.
    pub async fn active_tasks(&self) -> Vec<TaskRecord> {
        self.tasks
            .read()
            .await
            .values()
            .filter(|t| matches!(t.status, Status::Pending | Status::InProgress))
            .cloned()
            .collect()
    }

    /// Check if all child tasks of a parent are complete.
    pub async fn all_children_complete(&self, parent_id: Uuid) -> bool {
        let tasks = self.tasks.read().await;
        if let Some(parent) = tasks.get(&parent_id) {
            parent.child_tasks.iter().all(|child_id| {
                tasks
                    .get(child_id)
                    .is_some_and(|c| matches!(c.status, Status::Complete | Status::Failed))
            })
        } else {
            true
        }
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}
