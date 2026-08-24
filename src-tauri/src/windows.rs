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

/// The origin the portal window is showing, so a redirect follows the login it
/// belongs to instead of whatever the settings would pick.
pub fn portal_origin(app: &AppHandle) -> Option<String> {
    let window = app.get_webview_window(PORTAL_LABEL)?;
    let url = window.url().ok()?;
    Some(url.origin().ascii_serialization())
}

/// Opens a portal in a second window. Portals send `frame-ancestors 'none'`, so
/// a portal is always a top-level window and never an iframe inside the control
/// window.
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
