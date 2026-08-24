//! The context the portal reads before its first script runs.
//!
//! One portal build serves the web and this shell. On the web it finds no
//! `window.__ARUNA_DESKTOP__` and behaves like any hosted portal; here the
//! shell defines it in an initialization script, which the webview runs at
//! document start, so the bundle sees it the moment it boots.

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::bridge;
use crate::node::NodeState;
use crate::settings::NodeSettings;
use crate::state::AppState;
use crate::windows;

/// The command set this shell implements. The portal pins it, so a shell that
/// answers fewer commands must not claim this version.
pub const BRIDGE_VERSION: u32 = 1;

/// Runtime switches the portal reads. Both are what this shell always is: the
/// desktop views exist, and sign-in leaves through the system browser.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Features {
    pub desktop: bool,
    pub system_browser_auth: bool,
}

/// What the portal is told at boot. `bridge` is added by the script itself,
/// because a function cannot be serialized.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShellContext {
    pub api_base_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_callback_origin: Option<String>,
    pub features: Features,
}

/// The app window and the context it was opened with. The context is fixed for
/// the life of a webview, so both move together.
pub struct Shell {
    pub context: ShellContext,
    pub label: String,
}

impl Shell {
    pub fn new(context: ShellContext) -> Self {
        Self {
            context,
            label: windows::MAIN_LABEL.to_string(),
        }
    }
}

/// Picks what the portal talks to. An enrolled node that is running owns the
/// device's data, so it wins; otherwise the remembered realm answers, and with
/// neither there is only the local node to point at.
pub fn context(settings: &NodeSettings, running: bool) -> ShellContext {
    let local = settings.enrolled && running;
    let api_base_url = match (local, settings.realm_url.as_deref()) {
        (false, Some(realm)) => format!("{}/api/v1", realm.trim_end_matches('/')),
        _ => bridge::api_base(settings),
    };
    ShellContext {
        api_base_url,
        // The only http origin this device can register with a provider is the
        // node's own portal listener, and only while it serves one.
        auth_callback_origin: (local && settings.portal_dir.is_some())
            .then(|| settings.portal_url()),
        features: Features {
            desktop: true,
            system_browser_auth: true,
        },
    }
}

/// Renders the initialization script. The context is frozen and its binding is
/// not writable, so nothing the page loads can repoint the portal afterwards.
pub fn init_script(context: &ShellContext) -> String {
    let json = serde_json::to_string(context).expect("the shell context is serializable");
    format!(
        r#"(function () {{
  var context = {json};
  context.bridge = {{
    version: {BRIDGE_VERSION},
    invoke: function (command, args) {{
      var internals = window.__TAURI_INTERNALS__;
      if (!internals || typeof internals.invoke !== 'function') {{
        return Promise.reject(new Error('the Aruna Desktop bridge is not available'));
      }}
      return internals.invoke(command, args || {{}});
    }},
  }};
  Object.freeze(context.bridge);
  Object.defineProperty(window, '__ARUNA_DESKTOP__', {{ value: Object.freeze(context) }});
}})();"#
    )
}

/// Applies the current context, on the next turn of the runtime so the command
/// that changed it still answers first.
pub fn sync(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { apply(&app) });
}

/// Replaces the app window when what the portal must talk to has moved. The
/// injected context cannot be changed after a webview is created, so a reload
/// would only run the old script again.
fn apply(app: &AppHandle) {
    let state = app.state::<AppState>();
    let running = state.supervisor.status().state == NodeState::Running;
    let wanted = context(&state.settings(), running);

    let mut shell = state.shell.lock().expect("shell poisoned");
    if shell.context == wanted {
        return;
    }
    let label = next_label(&shell.label);
    match windows::replace_main(app, &label, &wanted) {
        Ok(()) => {
            shell.context = wanted;
            shell.label = label;
        }
        Err(error) => tracing::warn!(%error, "Could not apply the shell context"),
    }
}

/// A label the old window no longer holds: closing one and opening another with
/// the same name would race the teardown.
fn next_label(current: &str) -> String {
    let generation = current
        .rsplit_once('-')
        .and_then(|(_, tail)| tail.parse::<u32>().ok())
        .unwrap_or(0);
    format!("{}-{}", windows::MAIN_LABEL, generation + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn enrolled() -> NodeSettings {
        let mut settings = NodeSettings::default();
        settings.remember_realm("https://aruna.example");
        settings.joined(Some("r-1".into()));
        settings
    }

    #[test]
    fn prefers_the_local_node() {
        let context = context(&enrolled(), true);
        assert_eq!(context.api_base_url, "http://127.0.0.1:34116/api/v1");
        assert!(context.features.desktop);
    }

    #[test]
    fn falls_back_to_the_realm() {
        // A stopped or unenrolled node leaves the realm as the only API there is.
        let mut settings = enrolled();
        assert_eq!(
            context(&settings, false).api_base_url,
            "https://aruna.example/api/v1"
        );
        settings.enrolled = false;
        assert_eq!(
            context(&settings, true).api_base_url,
            "https://aruna.example/api/v1"
        );
    }

    #[test]
    fn points_at_the_node_alone() {
        // Nothing remembered yet: the local node is the only API this app has.
        let context = context(&NodeSettings::default(), false);
        assert_eq!(context.api_base_url, "http://127.0.0.1:34116/api/v1");
        assert!(context.auth_callback_origin.is_none());
    }

    #[test]
    fn offers_the_local_callback() {
        // Only a served local portal is an origin a provider can redirect to.
        let settings = NodeSettings {
            portal_dir: Some(PathBuf::from("/opt/portal")),
            ..enrolled()
        };
        assert_eq!(
            context(&settings, true).auth_callback_origin.as_deref(),
            Some("http://127.0.0.1:34115")
        );
        assert!(context(&settings, false).auth_callback_origin.is_none());
    }

    #[test]
    fn writes_the_context_once() {
        let script = init_script(&context(&enrolled(), true));
        assert!(script.contains("\"apiBaseUrl\":\"http://127.0.0.1:34116/api/v1\""));
        assert!(script.contains("\"systemBrowserAuth\":true"));
        assert!(script.contains("version: 1"));
        assert!(script.contains("Object.defineProperty(window, '__ARUNA_DESKTOP__'"));
        assert!(script.contains("Object.freeze(context)"));
    }

    #[test]
    fn counts_window_generations() {
        assert_eq!(next_label("main"), "main-1");
        assert_eq!(next_label("main-1"), "main-2");
        assert_eq!(next_label("main-11"), "main-12");
    }
}
