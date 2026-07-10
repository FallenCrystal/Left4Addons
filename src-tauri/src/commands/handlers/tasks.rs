use super::*;
use tauri::{AppHandle, State, Manager};

#[tauri::command]
pub async fn get_background_tasks(
    state: State<'_, crate::AppState>,
) -> Result<serde_json::Value, String> {
    if !state.background_tasks_path.exists() {
        return Ok(serde_json::json!([]));
    }
    let content = fs::read_to_string(&state.background_tasks_path)
        .map_err(|e| format!("Failed to read background tasks: {}", e))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse background tasks: {}", e))
}

#[tauri::command]
pub async fn save_background_task_snapshot(
    tasks: serde_json::Value,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    if let Some(parent) = state.background_tasks_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create task directory: {}", e))?;
    }
    let json = serde_json::to_string_pretty(&tasks)
        .map_err(|e| format!("Failed to serialize background tasks: {}", e))?;
    fs::write(&state.background_tasks_path, json)
        .map_err(|e| format!("Failed to write background tasks: {}", e))
}

#[tauri::command]
pub async fn cancel_download(
    workshop_id: String,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    request_download_cancellation(&state, &workshop_id)
}

#[tauri::command]
pub async fn append_workshop_crawl_log(
    record: serde_json::Value,
    state: State<'_, crate::AppState>,
) -> Result<(), String> {
    append_workshop_crawl_log_internal(&state.workshop_crawl_log_path, record)
}

