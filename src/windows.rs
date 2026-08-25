use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::error::DesktopError;
use crate::shell::{self, ShellContext};

pub const MAIN_LABEL: &str = "main";

/// Emitted to the app window when a deep link belongs in the portal's router.
pub const NAVIGATE_EVENT: &str = "navigate";

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
    Ok(
        WebviewWindowBuilder::new(app, MAIN_LABEL, WebviewUrl::App(PathBuf::from(ROOT)))
            .title("Aruna")
            .icon(tauri::include_image!("icons/128x128.png"))?
            .inner_size(1180.0, 820.0)
            .min_inner_size(820.0, 560.0)
            .initialization_script(shell::init_script(context))
            .build()?,
    )
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
