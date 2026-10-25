//! WebKitGTK's DMA-BUF renderer trips over several Wayland compositors: the
//! webview dies with a protocol error before it paints anything. On NVIDIA,
//! turning off explicit sync fixes this and keeps GPU rendering; elsewhere
//! the DMA-BUF renderer is turned off.

/// The WebKitGTK switch that turns the DMA-BUF renderer off.
const RENDERER: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";

/// The NVIDIA driver switch that turns Wayland explicit sync off.
const EXPLICIT_SYNC: &str = "__NV_DISABLE_EXPLICIT_SYNC";

/// Present while the NVIDIA kernel driver is loaded.
const NVIDIA_DRIVER: &str = "/proc/driver/nvidia/version";

/// Set to anything to keep the desktop from touching either switch.
const ESCAPE: &str = "ARUNA_DESKTOP_NO_WAYLAND_WORKAROUND";

/// Applies the workaround. Must run before GTK, the webview, or any thread the
/// app spawns.
pub fn guard() {
    let Some(key) = workaround(
        [var(RENDERER), var(EXPLICIT_SYNC), var(ESCAPE)],
        var("WAYLAND_DISPLAY").as_deref(),
        var("XDG_SESSION_TYPE").as_deref(),
        std::path::Path::new(NVIDIA_DRIVER).exists(),
    ) else {
        return;
    };
    // SAFETY: called before the app spawns a thread, so nothing else can be
    // reading the environment.
    unsafe { std::env::set_var(key, "1") };
    tracing::info!(variable = key, "Set a WebKitGTK workaround for Wayland");
}

/// The variable to set, if any. The owner's own setting always wins, on any of
/// the three variables.
fn workaround(
    owner: [Option<String>; 3],
    display: Option<&str>,
    session: Option<&str>,
    nvidia: bool,
) -> Option<&'static str> {
    let wayland =
        display.is_some() || session.is_some_and(|value| value.eq_ignore_ascii_case("wayland"));
    if !wayland || owner.iter().any(Option::is_some) {
        return None;
    }
    Some(if nvidia { EXPLICIT_SYNC } else { RENDERER })
}

fn var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNSET: [Option<String>; 3] = [None, None, None];

    #[test]
    fn guards_on_wayland() {
        assert_eq!(
            workaround(UNSET, Some("wayland-0"), None, false),
            Some(RENDERER)
        );
        assert_eq!(
            workaround(UNSET, None, Some("Wayland"), false),
            Some(RENDERER)
        );
    }

    #[test]
    fn keeps_nvidia_gpu() {
        assert_eq!(
            workaround(UNSET, Some("wayland-0"), None, true),
            Some(EXPLICIT_SYNC)
        );
        assert_eq!(
            workaround(UNSET, None, Some("wayland"), true),
            Some(EXPLICIT_SYNC)
        );
    }

    #[test]
    fn skips_other_sessions() {
        assert_eq!(workaround(UNSET, None, Some("x11"), false), None);
        assert_eq!(workaround(UNSET, None, Some("x11"), true), None);
        assert_eq!(workaround(UNSET, None, None, true), None);
    }

    #[test]
    fn keeps_owner_choice() {
        for slot in 0..3 {
            let mut owner = UNSET;
            owner[slot] = Some("0".to_string());
            assert_eq!(
                workaround(owner.clone(), Some("wayland-0"), None, true),
                None
            );
            assert_eq!(workaround(owner, Some("wayland-0"), None, false), None);
        }
    }
}
