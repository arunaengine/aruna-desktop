use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use url::Url;

use crate::error::DesktopError;

pub const MAIN_LABEL: &str = "main";
pub const PORTAL_LABEL: &str = "portal";

pub fn focus_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Opens the node's own portal listener in a second window. The node sends
/// `frame-ancestors 'none'`, so the portal is always a top-level window and
/// never an iframe inside the control window.
pub fn open_portal(app: &AppHandle, base: &str, path: Option<&str>) -> Result<(), DesktopError> {
    let target = Url::parse(&format!("{base}{}", path.unwrap_or("/")))
        .map_err(|error| DesktopError::DeepLink(error.to_string()))?;

    if let Some(window) = app.get_webview_window(PORTAL_LABEL) {
        window.navigate(target)?;
        window.set_focus()?;
        return Ok(());
    }

    WebviewWindowBuilder::new(app, PORTAL_LABEL, WebviewUrl::External(target))
        .title("Aruna Portal")
        .inner_size(1180.0, 820.0)
        .min_inner_size(820.0, 560.0)
        .build()?;
    Ok(())
}
