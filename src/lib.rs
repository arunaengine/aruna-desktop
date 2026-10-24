mod bridge;
mod deep_link;
mod enroll;
mod error;
mod node;
mod oidc;
mod realm;
mod settings;
mod shell;
mod state;
#[cfg(target_os = "linux")]
mod wayland;
mod windows;

use tauri::Manager;
use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri_plugin_deep_link::DeepLinkExt;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

const QUIT_MENU_ID: &str = "quit-aruna-desktop";

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
            bridge::shell_context,
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
            bridge::auth_open,
            bridge::oidc_discovery,
            bridge::oidc_token,
            bridge::validate_realm,
            bridge::enroll_invite_last,
            bridge::app_quit,
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
        .menu(|app| {
            let quit = MenuItemBuilder::with_id(QUIT_MENU_ID, "Quit Aruna Desktop")
                .accelerator("CmdOrCtrl+Q")
                .build(app)?;
            let file = SubmenuBuilder::new(app, "File").item(&quit).build()?;
            // Without these the webview gets no Cmd+C, Cmd+V or Cmd+A on macOS.
            let edit = SubmenuBuilder::new(app, "Edit")
                .undo()
                .redo()
                .separator()
                .cut()
                .copy()
                .paste()
                .select_all()
                .build()?;
            MenuBuilder::new(app).item(&file).item(&edit).build()
        })
        .on_menu_event(|app, event| {
            if event.id() == QUIT_MENU_ID {
                tauri::async_runtime::spawn(windows::quit(app.clone()));
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let state = AppState::build(&handle)?;
            let settings = state.settings();
            let context = state.shell.lock().expect("shell poisoned").clone();
            app.manage(state);
            windows::open_main(&handle, &context)?;

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

            tauri::async_runtime::spawn(bridge::watch_version(handle.clone()));
            #[cfg(unix)]
            tauri::async_runtime::spawn(windows::quit_on_signal(handle.clone()));

            if settings.starts_on_launch() {
                tauri::async_runtime::spawn(async move {
                    let state = handle.state::<AppState>();
                    if let Err(error) = bridge::start_node(&state).await {
                        tracing::warn!(%error, "Could not start the node");
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(handler)
        .build(tauri::generate_context!())
        .expect("failed to build the Aruna Desktop app")
        .run(|app, event| match event {
            // A Dock quit, a logout or an updater restart skips the close
            // handler, so the node is stopped here before the app may end.
            tauri::RunEvent::ExitRequested {
                api, code: None, ..
            } => {
                api.prevent_exit();
                tauri::async_runtime::spawn(windows::quit(app.clone()));
            }
            // Last resort for any other way out; a stopped node is a no-op.
            tauri::RunEvent::Exit => {
                if let Some(state) = app.try_state::<AppState>() {
                    tauri::async_runtime::block_on(state.supervisor.shutdown());
                }
            }
            _ => {}
        });
}
