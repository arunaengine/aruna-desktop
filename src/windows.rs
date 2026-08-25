use std::path::PathBuf;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::error::DesktopError;
use crate::shell::{self, ShellContext};

pub const MAIN_LABEL: &str = "main";

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

/// Sends the app window to a route of the portal it is already showing. The
/// portal is a single-page app, so this is how a deep link reaches its router.
pub fn navigate_main(app: &AppHandle, path: &str) -> Result<(), DesktopError> {
    let window = main(app).ok_or(DesktopError::Tauri(tauri::Error::WebviewNotFound))?;
    let target = window
        .url()?
        .join(path)
        .map_err(|error| DesktopError::DeepLink(error.to_string()))?;
    window.navigate(target)?;
    focus_main(app);
    Ok(())
}
