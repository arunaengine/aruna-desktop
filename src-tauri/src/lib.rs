mod bridge;
mod commands;
mod deep_link;
mod enroll;
mod error;
mod node;
mod portal;
mod realm;
mod route;
mod settings;
mod shell;
mod state;
#[cfg(target_os = "linux")]
mod wayland;
mod windows;

use tauri::Manager;
use tauri_plugin_deep_link::DeepLinkExt;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

/// Boots the native control window. The aruna node itself always runs as a
/// supervised child process, never inside this process.
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();

    #[cfg(target_os = "linux")]
    wayland::guard();

    let mut builder = tauri::Builder::default();

    // Registered first so a second launch reaches this instance; the plugin's
    // deep-link feature hands the URL from that argv to the deep-link plugin.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            windows::focus_main(app);
        }));
    }

    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let state = AppState::build(&handle)?;
            let settings = state.settings();
            let auto_start = settings.auto_start && !settings.paused;
            let context = state.shell.lock().expect("shell poisoned").context.clone();
            app.manage(state);
            windows::open_main(&handle, windows::MAIN_LABEL, &context, "index.html")?;

            // Dev builds carry no installed desktop entry, so the scheme is
            // claimed at runtime where the platform allows it.
            #[cfg(any(target_os = "linux", windows))]
            if let Err(error) = app.deep_link().register_all() {
                tracing::warn!(%error, "Could not register the aruna scheme");
            }

            let router = handle.clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    deep_link::handle(&router, url.as_str());
                }
            });

            if auto_start && let Err(error) = handle.state::<AppState>().supervisor.start() {
                tracing::warn!(%error, "Could not start the node");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bridge::node_status,
            bridge::node_logs_tail,
            bridge::node_settings_get,
            bridge::node_settings_set,
            bridge::enroll_apply,
            bridge::wipe_device,
            bridge::pick_directory,
            bridge::open_external,
            commands::launch_route,
            commands::node_start,
            commands::node_stop,
            commands::node_restart,
            commands::node_logs,
            commands::node_health,
            commands::settings_get,
            commands::settings_save,
            commands::paths_get,
            commands::enroll_pending,
            commands::enroll_accept,
            commands::enroll_paste,
            commands::enroll_poll,
            commands::realm_connect,
            commands::portal_open,
            commands::portal_options,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start the Aruna Desktop app")
}
