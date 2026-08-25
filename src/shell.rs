//! The context the portal reads before its first script runs, and follows
//! afterwards.
//!
//! One portal build serves the web and this shell. On the web it finds no
//! `window.__ARUNA_DESKTOP__` and behaves like any hosted portal; here the
//! shell defines it in an initialization script, which the webview runs at
//! document start, so the bundle sees it the moment it boots. The window is
//! never replaced: every later move is emitted as `context-changed` instead.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::bridge;
use crate::node::NodeState;
use crate::settings::NodeSettings;
use crate::state::AppState;
use crate::windows;

/// The command set this shell implements. The portal pins it, so a shell that
/// answers fewer commands must not claim this version.
pub const BRIDGE_VERSION: u32 = 3;

/// Emitted to the app window whenever what the portal must talk to has moved.
/// It carries the whole context, in the shape the script injected.
pub const CONTEXT_EVENT: &str = "context-changed";

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
    /// The remembered realm. The portal's welcome guard trusts this over any
    /// address heuristic, so a loopback realm still counts as one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub realm_url: Option<String>,
    pub features: Features,
}

/// Picks what the portal talks to. An enrolled node that is running owns the
/// device's data, so it wins; otherwise the remembered realm answers, and with
/// neither there is only the local node to point at.
pub fn context(settings: &NodeSettings, running: bool) -> ShellContext {
    let local = settings.enrolled && running;
    let api_base_url = match (local, settings.realm_url.as_deref()) {
        (false, Some(realm)) => settings
            .realm_api_url
            .clone()
            .unwrap_or_else(|| format!("{}/api/v1", realm.trim_end_matches('/'))),
        _ => bridge::api_base(settings),
    };
    ShellContext {
        api_base_url,
        // The only http origin this device can register with a provider is the
        // node's own portal listener, and only while it serves one.
        auth_callback_origin: (local && settings.portal_dir.is_some())
            .then(|| settings.portal_url()),
        realm_url: settings.realm_url.clone(),
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

/// Recomputes the context and tells the window when it moved, on the next turn
/// of the runtime so the command that changed it still answers first.
pub fn sync(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        apply(&app);
    });
}

/// Stores what the portal must talk to now and emits it once, when it differs
/// from what the portal was last told.
fn apply(app: &AppHandle) {
    let state = app.state::<AppState>();
    let running = state.supervisor.status().state == NodeState::Running;
    let wanted = context(&state.settings(), running);

    // The lock is not held across the emit: `shell_context` answers from it.
    let moved = {
        let mut current = state.shell.lock().expect("shell poisoned");
        advance(&mut current, wanted)
    };
    if let Some(context) = moved
        && let Err(error) = app.emit_to(windows::MAIN_LABEL, CONTEXT_EVENT, context)
    {
        tracing::warn!(%error, "Could not emit the shell context");
    }
}

/// Stores `wanted` when it differs and answers the payload to emit; `None`
/// while the portal already has the context it needs.
fn advance(current: &mut ShellContext, wanted: ShellContext) -> Option<ShellContext> {
    if *current == wanted {
        return None;
    }
    *current = wanted.clone();
    Some(wanted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn enrolled() -> NodeSettings {
        let mut settings = NodeSettings::default();
        settings.remember_realm("https://aruna.example", "https://aruna.example/api/v1");
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
    fn resolved_api_wins() {
        // A realm may serve its portal and API on separate listeners.
        let mut settings = NodeSettings::default();
        settings.remember_realm("http://127.0.0.1:43025", "http://127.0.0.1:43021/api/v1");
        let context = context(&settings, false);
        assert_eq!(context.api_base_url, "http://127.0.0.1:43021/api/v1");
        assert_eq!(context.realm_url.as_deref(), Some("http://127.0.0.1:43025"));
    }

    #[test]
    fn points_at_the_node_alone() {
        // Nothing remembered yet: the local node is the only API this app has.
        let context = context(&NodeSettings::default(), false);
        assert_eq!(context.api_base_url, "http://127.0.0.1:34116/api/v1");
        assert!(context.auth_callback_origin.is_none());
        assert!(context.realm_url.is_none());
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
        assert!(script.contains("\"realmUrl\":\"https://aruna.example\""));
        assert!(script.contains("\"systemBrowserAuth\":true"));
        assert!(script.contains("version: 3"));
        assert!(script.contains("Object.defineProperty(window, '__ARUNA_DESKTOP__'"));
        assert!(script.contains("Object.freeze(context)"));
    }

    #[test]
    fn emits_only_on_change() {
        // The portal is told once per move, and never told what it already has.
        let mut current = context(&NodeSettings::default(), false);
        let same = current.clone();
        assert!(advance(&mut current, same).is_none());

        let wanted = context(&enrolled(), false);
        let emitted = advance(&mut current, wanted.clone()).expect("a moved context");
        assert_eq!(emitted, wanted);
        assert_eq!(emitted.api_base_url, "https://aruna.example/api/v1");
        // What `shell_context` answers from here on.
        assert_eq!(current, wanted);
        assert!(advance(&mut current, wanted).is_none());
    }

    #[test]
    fn answers_one_shape() {
        // The command and the event carry what the script injects, minus the
        // bridge, which is a function the shell cannot serialize.
        let context = context(&enrolled(), true);
        let json = serde_json::to_string(&context).expect("the shell context is serializable");
        assert!(init_script(&context).contains(&json));
        assert!(!json.contains("bridge"));
        assert!(json.contains("\"apiBaseUrl\""));
    }
}
