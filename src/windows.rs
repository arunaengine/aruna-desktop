use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use url::Url;

use crate::deep_link;
use crate::error::DesktopError;
use crate::shell::{self, ShellContext};
use crate::state::AppState;

pub const MAIN_LABEL: &str = "main";
pub const LOGIN_LABEL: &str = "login";

/// Emitted to the app window when a deep link belongs in the portal's router.
pub const NAVIGATE_EVENT: &str = "navigate";

/// Emitted to the app window when the sign-in window went away before the
/// provider answered, so the portal drops the sign-in it was waiting on.
pub const AUTH_CANCELLED_EVENT: &str = "auth-cancelled";

/// What the window opens on; Tauri reads it as the app root.
const ROOT: &str = "index.html";

/// The app window. It is opened once and kept for the life of the app: the
/// portal follows the shell context through `context-changed`.
pub fn main(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(MAIN_LABEL)
}

pub fn focus_main(app: &AppHandle) {
    if let Some(window) = main(app) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Opens the app window. The shell context is injected as an initialization
/// script, which the webview runs before the portal's own scripts.
pub fn open_main(app: &AppHandle, context: &ShellContext) -> Result<WebviewWindow, DesktopError> {
    let window = WebviewWindowBuilder::new(app, MAIN_LABEL, WebviewUrl::App(PathBuf::from(ROOT)))
        .title("Aruna")
        .icon(tauri::include_image!("icons/128x128.png"))?
        .inner_size(1180.0, 820.0)
        .min_inner_size(820.0, 560.0)
        .initialization_script(shell::init_script(context))
        .build()?;

    // Outside macOS the menu sits in the window; hidden, it still serves Ctrl+Q.
    #[cfg(not(target_os = "macos"))]
    if let Err(error) = window.hide_menu() {
        tracing::warn!(%error, "Could not hide the window menu");
    }

    let owner = app.clone();
    window.on_window_event(move |event| {
        // Closing the app window ends the app, but the node it supervises gets
        // its own shutdown first, so the close waits for the stop.
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            tauri::async_runtime::spawn(quit(owner.clone()));
        }
    });
    Ok(window)
}

/// Stops the node the way its own shutdown expects, then ends the app. The
/// first caller owns it; a second one leaves that stop to finish.
pub async fn quit(app: AppHandle) {
    if !app.state::<AppState>().claim_quit() {
        return;
    }
    app.state::<AppState>().supervisor.shutdown().await;
    app.exit(0);
}

/// Runs the quit path on SIGTERM or SIGINT, so a logout, a service stop or a
/// kill leaves no node behind.
#[cfg(unix)]
pub async fn quit_on_signal(app: AppHandle) {
    use tokio::signal::unix::{SignalKind, signal};

    let (Ok(mut term), Ok(mut interrupt)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        tracing::warn!("Could not listen for termination signals");
        return;
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = interrupt.recv() => {}
    }
    quit(app).await;
}

/// Opens the provider's sign-in page in a window of the app's own. The realm
/// website's callback moves on to this app's scheme; that navigation is never
/// loaded but routed like a deep link, and the window closes on it. Closing it
/// any other way tells the portal the sign-in was abandoned.
pub fn open_login(app: &AppHandle, url: Url) -> Result<WebviewWindow, DesktopError> {
    if let Some(open) = app.get_webview_window(LOGIN_LABEL) {
        open.destroy()?;
    }
    let claimed = Arc::new(AtomicBool::new(false));
    let router = app.clone();
    let claim = claimed.clone();
    let window = WebviewWindowBuilder::new(app, LOGIN_LABEL, WebviewUrl::External(url))
        .title("Sign in to Aruna")
        .icon(tauri::include_image!("icons/128x128.png"))?
        .inner_size(560.0, 780.0)
        .min_inner_size(420.0, 560.0)
        .on_navigation(move |target| {
            if target.scheme() != deep_link::SCHEME {
                return true;
            }
            claim.store(true, Ordering::SeqCst);
            let app = router.clone();
            let link = target.to_string();
            // Routed off the webview's own thread: handling it closes this window.
            let _ = router.run_on_main_thread(move || {
                deep_link::handle(&app, &link);
                if let Some(login) = app.get_webview_window(LOGIN_LABEL) {
                    let _ = login.destroy();
                }
            });
            false
        })
        .build()?;
    let owner = app.clone();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) && !claimed.load(Ordering::SeqCst) {
            let _ = owner.emit_to(MAIN_LABEL, AUTH_CANCELLED_EVENT, ());
            focus_main(&owner);
        }
    });
    Ok(window)
}

/// Where a deep link wants the portal's router: a route of the portal root,
/// carrying the query and fragment the link arrived with.
#[derive(Clone, Debug, Serialize)]
struct Navigate {
    path: String,
}

/// Asks the portal to route somewhere, and brings the window forward. It is an
/// event rather than a navigation: loading the URL would reload the page and
/// drop the session and the context the portal has already built.
pub fn navigate_main(app: &AppHandle, path: &str) -> Result<(), DesktopError> {
    let route = Navigate {
        path: path.to_string(),
    };
    app.emit_to(MAIN_LABEL, NAVIGATE_EVENT, route)?;
    focus_main(app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_route() {
        // The portal reads one key, and it is a route of its own router.
        let route = Navigate {
            path: "/auth/callback?code=x#state=y".into(),
        };
        let json = serde_json::to_string(&route).expect("a serializable route");
        assert_eq!(json, r#"{"path":"/auth/callback?code=x#state=y"}"#);
    }
}
