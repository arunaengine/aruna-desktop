use std::path::PathBuf;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use url::Url;

use crate::error::DesktopError;
use crate::shell::{self, ShellContext};
use crate::state::AppState;

pub const MAIN_LABEL: &str = "main";
pub const PORTAL_LABEL: &str = "portal";

/// What a fresh window opens on; Tauri reads it as the app root.
const ROOT: &str = "index.html";

/// The app window, whatever generation it is on.
pub fn main(app: &AppHandle) -> Option<WebviewWindow> {
    let label = app.state::<AppState>().main_label();
    app.get_webview_window(&label)
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
pub fn open_main(
    app: &AppHandle,
    label: &str,
    context: &ShellContext,
    path: &str,
) -> Result<WebviewWindow, DesktopError> {
    Ok(
        WebviewWindowBuilder::new(app, label, WebviewUrl::App(PathBuf::from(path)))
            .title("Aruna")
            .inner_size(1180.0, 820.0)
            .min_inner_size(820.0, 560.0)
            .initialization_script(shell::init_script(context))
            .build()?,
    )
}

/// Swaps the app window for one carrying `context`, on the page the old one was
/// showing. The new window is opened before the old one is destroyed, so the
/// app is never briefly without a window.
pub fn replace_main(
    app: &AppHandle,
    label: &str,
    context: &ShellContext,
) -> Result<(), DesktopError> {
    let current = main(app);
    let path = current
        .as_ref()
        .and_then(|window| window.url().ok())
        .map(|url| route(&url))
        .unwrap_or_else(|| ROOT.to_string());

    open_main(app, label, context, &path)?;
    if let Some(window) = current {
        let _ = window.destroy();
    }
    Ok(())
}

/// The page a window is showing, relative to the app root, so reopening lands
/// the portal's router back where the owner was.
fn route(url: &Url) -> String {
    let mut route = url.path().trim_start_matches('/').to_string();
    if let Some(query) = url.query() {
        route.push('?');
        route.push_str(query);
    }
    if let Some(fragment) = url.fragment() {
        route.push('#');
        route.push_str(fragment);
    }
    route
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_current_page() {
        // Replacing the window must not send the owner back to the start.
        let url = Url::parse("tauri://localhost/app/device?tab=local#logs").expect("valid url");
        assert_eq!(route(&url), "app/device?tab=local#logs");
        assert_eq!(
            route(&Url::parse("tauri://localhost/").expect("valid url")),
            ""
        );
    }
}
