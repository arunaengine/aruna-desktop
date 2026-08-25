mod bridge;
mod deep_link;
mod enroll;
mod error;
mod node;
mod realm;
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

    // Tauri rejects an unregistered command with its own wording; naming it
    // "unknown" is what the portal reads as a bridge without that half.
    let commands: Box<dyn Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync> =
        Box::new(tauri::generate_handler![
            bridge::node_status,
            bridge::node_logs_tail,
            bridge::node_settings_get,
            bridge::node_settings_set,
            bridge::compute_probe,
            bridge::enroll_apply,
            bridge::wipe_device,
            bridge::pick_directory,
            bridge::reveal_path,
            bridge::open_external,
            bridge::validate_realm,
            bridge::enroll_invite_last,
        ]);
    let handler = move |invoke: tauri::ipc::Invoke<tauri::Wry>| {
        let command = invoke.message.command();
        if !bridge::KNOWN.contains(&command) {
            let refusal = format!("unknown command: {command}");
            invoke.resolver.reject(refusal);
            return true;
        }
        commands(invoke)
    };

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
        .invoke_handler(handler)
        .run(tauri::generate_context!())
        .expect("failed to start the Aruna Desktop app")
}
