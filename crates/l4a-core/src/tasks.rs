use crate::db::LocalDb;
use crate::error::CoreError;
use chrono::Utc;
use l4a_protocol::{TaskId, TaskKind, TaskRecord, TaskStatus};

pub fn create_task(
    db: &LocalDb,
    kind: TaskKind,
    target_id: impl Into<String>,
    target_title: Option<String>,
) -> Result<TaskRecord, CoreError> {
    let now = Utc::now();
    let task = TaskRecord {
        id: TaskId::generate(),
        kind,
        status: TaskStatus::Queued,
        target_id: target_id.into(),
        target_title,
        progress_percent: 0,
        message: None,
        error: None,
        created_at: now,
        updated_at: now,
    };
    db.save_task(&task)?;
    Ok(task)
}

pub fn update_task(
    db: &LocalDb,
    task: &mut TaskRecord,
    status: TaskStatus,
    progress_percent: u8,
    message: Option<String>,
    error: Option<String>,
) -> Result<(), CoreError> {
    task.status = status;
    task.progress_percent = progress_percent.min(100);
    task.message = message;
    task.error = error;
    task.updated_at = Utc::now();
    db.save_task(task)
}
