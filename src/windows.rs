use std::path::PathBuf;

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use url::Url;

use crate::error::DesktopError;
use crate::shell::{self, ShellContext};
use crate::state::AppState;

pub const MAIN_LABEL: &str = "main";

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
            .icon(tauri::include_image!("icons/128x128.png"))?
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
