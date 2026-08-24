use tauri::State;

use crate::error::DesktopError;
use crate::node::{self, HealthReport, LogPage, NodeStatus};
use crate::settings::NodeSettings;
use crate::state::{AppState, Paths};

#[tauri::command]
pub fn node_status(state: State<'_, AppState>) -> NodeStatus {
    state.supervisor.status()
}

#[tauri::command]
pub fn node_start(state: State<'_, AppState>) -> Result<NodeStatus, DesktopError> {
    state.supervisor.start()
}

#[tauri::command]
pub async fn node_stop(state: State<'_, AppState>) -> Result<NodeStatus, DesktopError> {
    Ok(state.supervisor.stop().await)
}

#[tauri::command]
pub async fn node_restart(state: State<'_, AppState>) -> Result<NodeStatus, DesktopError> {
    state.supervisor.restart().await
}

#[tauri::command]
pub fn node_logs(state: State<'_, AppState>, cursor: u64) -> LogPage {
    state.supervisor.logs().page(cursor)
}

#[tauri::command]
pub async fn node_health(state: State<'_, AppState>) -> Result<HealthReport, DesktopError> {
    let port = state.settings().ports.ops;
    Ok(node::probe(&state.http, port).await)
}

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> NodeSettings {
    state.settings()
}

/// Persists the settings. They reach the node on its next start, which the
/// settings view makes explicit rather than restarting behind the owner's back.
#[tauri::command]
pub fn settings_save(
    state: State<'_, AppState>,
    settings: NodeSettings,
) -> Result<NodeSettings, DesktopError> {
    settings.validate()?;
    settings.save(&state.settings_path)?;
    *state.settings.lock().expect("settings poisoned") = settings.clone();
    Ok(settings)
}

#[tauri::command]
pub fn paths_get(state: State<'_, AppState>) -> Paths {
    state.paths()
}
