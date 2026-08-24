use tauri::{AppHandle, Manager};
use tracing_subscriber::EnvFilter;

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
