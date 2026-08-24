//! WebKitGTK's DMA-BUF renderer trips over several Wayland compositors: the
//! webview dies with a protocol error before it paints anything.

/// The WebKitGTK switch that turns the DMA-BUF renderer off.
const RENDERER: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";

/// Set to anything to keep the desktop from touching that switch.
const ESCAPE: &str = "ARUNA_DESKTOP_NO_WAYLAND_WORKAROUND";

/// Applies the workaround. Must run before GTK, the webview, or any thread the
/// app spawns.
pub fn guard() {
    if !needs_guard(
        var(RENDERER).as_deref(),
        var(ESCAPE).as_deref(),
        var("WAYLAND_DISPLAY").as_deref(),
        var("XDG_SESSION_TYPE").as_deref(),
    ) {
        return;
    }
    // SAFETY: called before the app spawns a thread, so nothing else can be
    // reading the environment.
    unsafe { std::env::set_var(RENDERER, "1") };
    tracing::info!("Disabled the WebKitGTK DMA-BUF renderer for Wayland");
}

/// The owner's own setting always wins, on either variable.
fn needs_guard(
    renderer: Option<&str>,
    escape: Option<&str>,
    display: Option<&str>,
    session: Option<&str>,
) -> bool {
    if renderer.is_some() || escape.is_some() {
        return false;
    }
    display.is_some() || session.is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
}

fn var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guards_on_wayland() {
        assert!(needs_guard(None, None, Some("wayland-0"), None));
        assert!(needs_guard(None, None, None, Some("Wayland")));
    }

    #[test]
    fn skips_other_sessions() {
        assert!(!needs_guard(None, None, None, Some("x11")));
        assert!(!needs_guard(None, None, None, None));
    }

    #[test]
    fn keeps_owner_choice() {
        assert!(!needs_guard(Some("0"), None, Some("wayland-0"), None));
        assert!(!needs_guard(None, Some("1"), Some("wayland-0"), None));
    }
}
