use tauri::{AppHandle, State};

use crate::deep_link::EnrollPayload;
use crate::error::DesktopError;
use crate::node::{self, HealthReport, LogPage, NodeStatus};
use crate::settings::NodeSettings;
use crate::state::{AppState, Paths};
use crate::windows;

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

/// Hands the stashed deep-link payload to the enroll view exactly once.
#[tauri::command]
pub fn enroll_take(state: State<'_, AppState>) -> Option<EnrollPayload> {
    state
        .pending
        .lock()
        .expect("pending enrollment poisoned")
        .take()
}

/// Stores the one-time secret and restarts the node so it enrolls on boot.
/// TODO(aruna): the enrollment status poll and the device list land with the
/// user-node routes; until then the node's own logs report the outcome.
#[tauri::command]
pub async fn enroll_apply(
    state: State<'_, AppState>,
    secret: String,
) -> Result<NodeStatus, DesktopError> {
    let secret = secret.trim().to_string();
    if secret.is_empty() {
        return Err(DesktopError::Settings(
            "the enrollment secret is empty".into(),
        ));
    }
    let mut settings = state.settings();
    settings.onboarding_secret = Some(secret);
    settings.validate()?;
    settings.save(&state.settings_path)?;
    *state.settings.lock().expect("settings poisoned") = settings;
    state.supervisor.restart().await
}

#[tauri::command]
pub fn portal_open(app: AppHandle, state: State<'_, AppState>) -> Result<(), DesktopError> {
    windows::open_portal(&app, &state.settings().portal_url(), None)
}
