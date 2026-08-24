mod commands;
mod error;
mod node;
mod settings;
mod state;

use tauri::{AppHandle, Manager};
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

/// Boots the native control window. The aruna node itself always runs as a
/// supervised child process, never inside this process.
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();

    let mut builder = tauri::Builder::default();

    // Registered first so a second launch reaches this instance; the plugin's
    // deep-link feature hands the URL from that argv to the deep-link plugin.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            focus_main(app);
        }));
    }

    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let state = AppState::build(app.handle())?;
            let auto_start = state.settings().auto_start;
            app.manage(state);

            if auto_start
                && let Err(error) = app.state::<AppState>().supervisor.start()
            {
                tracing::warn!(%error, "Could not start the node");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::node_status,
            commands::node_start,
            commands::node_stop,
            commands::node_restart,
            commands::node_logs,
            commands::node_health,
            commands::settings_get,
            commands::settings_save,
            commands::paths_get,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start the Aruna Desktop app")
}

fn focus_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
