//! The command channel the portal talks to. One portal build serves both the
//! web and this shell, so the names, arguments and answers here are a contract:
//! the portal's `desktopBridge` module wraps exactly these commands, and one it
//! does not get an answer for is reported as a missing shell half.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::oneshot;

use crate::deep_link::{EnrollInvite, EnrollLink};
use crate::enroll::{EnrollPhase, Observed};
use crate::error::DesktopError;
use crate::node::compute::ComputeProbe;
use crate::node::{self, NodeInfo, NodeState, NodeStatus};
use crate::realm::{self, RealmTarget};
use crate::settings::{LocalSettings, NodeSettings, SettingsPatch};
use crate::shell;
use crate::state::AppState;

/// Every command this shell answers. A name that is not here is rejected as
/// unknown, which the portal reads as a bridge missing that half.
pub const KNOWN: [&str; 12] = [
    "node_status",
    "node_logs_tail",
    "node_settings_get",
    "node_settings_set",
    "compute_probe",
    "enroll_apply",
    "wipe_device",
    "pick_directory",
    "reveal_path",
    "open_external",
    "validate_realm",
    "enroll_invite_last",
];

/// The phrase `wipe_device` demands back before it destroys anything. The
/// portal asks for it too; the shell never trusts that it did.
const WIPE_PHRASE: &str = "wipe";

/// The supervisor's state in the four the portal renders. `Stopping` counts as
/// stopped and `Restarting` as starting: both are on their way to that state.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DeviceState {
    Stopped,
    Starting,
    Running,
    Error,
}

impl From<NodeState> for DeviceState {
    fn from(state: NodeState) -> Self {
        match state {
            NodeState::Stopped | NodeState::Stopping => Self::Stopped,
            NodeState::Starting | NodeState::Restarting => Self::Starting,
            NodeState::Running => Self::Running,
            NodeState::Failed | NodeState::BinaryMissing => Self::Error,
        }
    }
}

/// What the portal shows about the node this app embeds. `node_id` stays empty:
/// a node reports its own id only to a caller holding a realm token, which the
/// desktop never has, so the portal reads it with the owner's own.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStatus {
    pub state: DeviceState,
    pub node_id: Option<String>,
    pub realm: Option<String>,
    pub enrolled: bool,
    pub api_base_url: Option<String>,
    pub version: Option<String>,
    pub uptime_seconds: Option<u64>,
    pub message: Option<String>,
}

/// What the shell knows the moment the secret is armed. The node id follows
/// from the realm, not from here.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnrollResult {
    pub node_id: Option<String>,
    pub realm: Option<String>,
}

/// Supervisor state of the embedded node, and the enrollment it is finishing.
#[tauri::command]
pub async fn node_status(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<DeviceStatus, DesktopError> {
    let node = state.supervisor.status();
    let seen = match node.state {
        NodeState::Running => Some(look(&state).await),
        _ => None,
    };
    let joining = settle(&state, &node, seen.as_ref()).await?;

    shell::sync(&app);
    let settings = state.settings();
    let info = seen.map(|seen| seen.info).unwrap_or_default();
    Ok(DeviceStatus {
        state: node.state.into(),
        node_id: None,
        realm: info.realm.or(settings.realm.clone()),
        enrolled: settings.enrolled,
        api_base_url: (node.state == NodeState::Running).then(|| api_base(&settings)),
        version: info.version,
        uptime_seconds: state.supervisor.uptime(),
        message: joining
            .or(node.detail)
            .or_else(|| settings.paused.then(|| "the node is paused".to_string())),
    })
}

/// The node's own output, oldest line first.
#[tauri::command]
pub fn node_logs_tail(state: State<'_, AppState>, lines: Option<usize>) -> Vec<String> {
    state.supervisor.logs().tail(lines.unwrap_or(200))
}

#[tauri::command]
pub fn node_settings_get(state: State<'_, AppState>) -> LocalSettings {
    state.settings().local(&state.data_dir)
}

/// Applies a settings patch and answers with what was stored. Pausing stops the
/// node and resuming starts it; where it stores data and what it may compute
/// are read at startup, so changing either restarts a node that is up.
#[tauri::command]
pub async fn node_settings_set(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: SettingsPatch,
) -> Result<LocalSettings, DesktopError> {
    let switched = settings.paused;
    let before = state.settings();
    let mut stored = before.clone();
    stored.apply(settings)?;
    stored.validate()?;
    let moved = restarts(&before, &stored);
    state.store(stored)?;

    let live = matches!(
        DeviceState::from(state.supervisor.status().state),
        DeviceState::Running | DeviceState::Starting
    );
    match switched {
        Some(true) => {
            state.supervisor.stop().await;
        }
        _ if moved && live => {
            if let Err(error) = state.supervisor.restart().await {
                tracing::warn!(%error, "Could not restart the node");
            }
        }
        // A device with no node binary stays a usable app, so a failed resume
        // is left in the status rather than refusing the setting.
        Some(false) => {
            if let Err(error) = state.supervisor.start() {
                tracing::warn!(%error, "Could not resume the node");
            }
        }
        None => {}
    }
    shell::sync(&app);
    Ok(state.settings().local(&state.data_dir))
}

/// What this machine can run a local job in, asked fresh: a daemon comes and
/// goes while the app stays open. The answer is the machine's, not the owner's
/// choice, which stays in the settings.
#[tauri::command]
pub async fn compute_probe() -> ComputeProbe {
    node::compute::probe().await
}

/// Shows a path in the owner's file manager. Only their own home and the
/// node's data are revealed; anything else is refused, like the wipe guard
/// refuses a path that was never ours.
#[tauri::command]
pub fn reveal_path(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<(), DesktopError> {
    let raw = path.trim();
    let target = fs::canonicalize(raw)
        .map_err(|_| DesktopError::Settings(format!("there is no {raw} to show")))?;
    let roots: Vec<PathBuf> = [home(), Some(state.data_dir.clone())]
        .into_iter()
        .flatten()
        .map(resolved)
        .collect();
    if !revealable(&target, &roots) {
        return Err(DesktopError::Settings(format!(
            "refusing to show {}",
            target.display()
        )));
    }
    app.opener()
        .reveal_item_in_dir(&target)
        .map_err(|error| DesktopError::Unreachable(error.to_string()))
}

/// Arms the next node start with a one-time enrollment secret and restarts the
/// node, which consumes it while it boots. Everything after this is observed.
/// Only the secret is required: the seed URL and the realm travel inside it,
/// and the desktop keeps its own copy of both only to watch the join.
#[tauri::command]
pub async fn enroll_apply(
    app: AppHandle,
    secret: String,
    seed_url: Option<String>,
    realm: Option<String>,
    label: Option<String>,
) -> Result<EnrollResult, DesktopError> {
    arm(&app, EnrollLink::accept(secret, seed_url, realm)?, label).await
}

/// Stores a one-time secret and restarts the node, which consumes it while it
/// boots. Shared with the `aruna://enroll` deep link, so a followed link and a
/// pasted code arm the device the same way.
pub async fn arm(
    app: &AppHandle,
    link: EnrollLink,
    label: Option<String>,
) -> Result<EnrollResult, DesktopError> {
    let state = app.state::<AppState>();
    let label = label.map(|label| label.trim().to_string());

    let mut settings = state.settings();
    settings.enroll(link.secret, link.seed.clone(), link.realm.clone());
    settings.device_label = label.filter(|label| !label.is_empty());
    // Enrolling is an explicit "run this node", so it lifts a pause.
    settings.paused = false;
    settings.validate()?;
    state.store(settings)?;

    {
        let mut tracker = state.enroll.lock().expect("enrollment poisoned");
        tracker
            .progress
            .accept(link.seed.as_deref(), link.realm.as_deref());
        tracker.cursor = 0;
    }
    state.supervisor.restart().await?;
    shell::sync(app);

    Ok(EnrollResult {
        node_id: None,
        realm: link.realm.or_else(|| state.settings().realm.clone()),
    })
}

/// The `aruna://enroll` outcome nobody has collected yet, exactly as it was
/// emitted, or `null`. A link followed into a cold start emits its event before
/// the portal listens, so the payload waits here; collecting it clears it,
/// because replaying an old outcome on every visit is stale news.
#[tauri::command]
pub fn enroll_invite_last(state: State<'_, AppState>) -> Option<EnrollInvite> {
    state.invite.take()
}

/// Checks a realm address the owner typed, remembers it as this device's realm
/// and answers with the origin the portal will talk to. Remembering is the
/// point: it is what the shell points the portal at until a node is enrolled.
#[tauri::command]
pub async fn validate_realm(
    app: AppHandle,
    state: State<'_, AppState>,
    input: String,
) -> Result<RealmTarget, DesktopError> {
    let origin = realm::normalize(&input)?;
    let target = realm::check(&state.http, &origin)
        .await
        .map_err(|reach| DesktopError::Unreachable(reach.message(&origin)))?;

    let mut settings = state.settings();
    settings.remember_realm(&target.origin, &target.api_base_url);
    state.store(settings)?;
    shell::reconnect(&app);
    Ok(target)
}

/// Destroys this device: the node stops, everything the desktop provisioned for
/// it is deleted, and the enrollment is forgotten. The node's own REST wipe is
/// a separate surface; this one is about what lives on this machine.
#[tauri::command]
pub async fn wipe_device(
    app: AppHandle,
    state: State<'_, AppState>,
    confirm: String,
) -> Result<(), DesktopError> {
    if !confirm.trim().eq_ignore_ascii_case(WIPE_PHRASE) {
        return Err(DesktopError::Settings(format!(
            "type {WIPE_PHRASE} to confirm the wipe"
        )));
    }
    state.supervisor.stop().await;

    let settings = state.settings();
    let storage = settings.storage_dir(&state.data_dir);
    if !storage.starts_with(&state.data_dir) {
        purge(&storage)?;
    }
    purge(&state.data_dir)?;

    let mut wiped = settings;
    wiped.wipe();
    state.store(wiped)?;
    *state.enroll.lock().expect("enrollment poisoned") = crate::enroll::Tracker::default();
    state.invite.clear();
    shell::sync(&app);
    Ok(())
}

/// The native folder dialog; `None` when the owner closed it.
#[tauri::command]
pub async fn pick_directory(
    app: AppHandle,
    title: Option<String>,
    start_path: Option<String>,
) -> Result<Option<String>, DesktopError> {
    let mut dialog = app.dialog().file();
    if let Some(title) = title.filter(|title| !title.trim().is_empty()) {
        dialog = dialog.set_title(title);
    }
    if let Some(start) = start_path.map(PathBuf::from).filter(|path| path.is_dir()) {
        dialog = dialog.set_directory(start);
    }

    let (sender, receiver) = oneshot::channel();
    dialog.pick_folder(move |picked| {
        let _ = sender.send(picked);
    });
    let picked = receiver
        .await
        .map_err(|_| DesktopError::Settings("the folder dialog closed unexpectedly".into()))?;
    Ok(picked
        .and_then(|folder| folder.into_path().ok())
        .map(|path| path.display().to_string()))
}

/// Hands a URL to the system browser. The webview never follows it: that is the
/// point of the command, and it is why only web addresses are accepted.
#[tauri::command]
pub fn open_external(app: AppHandle, url: String) -> Result<(), DesktopError> {
    let target = web_url(&url)?;
    app.opener()
        .open_url(target, None::<&str>)
        .map_err(|error| DesktopError::Unreachable(error.to_string()))
}

fn web_url(raw: &str) -> Result<String, DesktopError> {
    let url = url::Url::parse(raw.trim())
        .map_err(|error| DesktopError::Link(format!("unreadable URL: {error}")))?;
    match url.scheme() {
        "http" | "https" => Ok(url.to_string()),
        scheme => Err(DesktopError::Link(format!(
            "{scheme} links are not opened outside the app"
        ))),
    }
}

/// Whether the node has to be restarted for a patch to reach it. Both settings
/// are read once at startup, so a later start would be too late.
fn restarts(before: &NodeSettings, after: &NodeSettings) -> bool {
    before.storage_path != after.storage_path || before.compute != after.compute
}

/// The node's REST base, the same one the portal is told to talk to.
pub fn api_base(settings: &NodeSettings) -> String {
    format!("{}/api/v1", settings.api_url())
}

/// One round of what a running node says about itself.
struct Seen {
    ready: bool,
    info: NodeInfo,
}

async fn look(state: &AppState) -> Seen {
    let settings = state.settings();
    Seen {
        ready: node::probe(&state.http, settings.ports.ops).await.ready.ok,
        info: node::info(&state.http, &settings.api_url())
            .await
            .unwrap_or_default(),
    }
}

/// Advances a running enrollment from what can be observed, and spends the
/// secret once the node serves the realm: it is needed again while the join is
/// half done, and must never be replayable after it.
async fn settle(
    state: &AppState,
    node: &NodeStatus,
    seen: Option<&Seen>,
) -> Result<Option<String>, DesktopError> {
    let (active, mut realm, seed, cursor) = {
        let tracker = state.enroll.lock().expect("enrollment poisoned");
        (
            tracker.progress.active(),
            tracker.progress.realm.clone(),
            tracker.progress.seed.clone(),
            tracker.cursor,
        )
    };
    if !active {
        return Ok(state.progress().detail);
    }

    let page = state.supervisor.logs().page(cursor);
    let refusal = page
        .lines
        .iter()
        .find_map(|line| crate::enroll::refusal(&line.text));
    let ready = seen.is_some_and(|seen| seen.ready);

    // A pasted bare secret names no realm, so the seed is asked which one it
    // serves; the seed cannot list its nodes to an anonymous caller.
    if ready
        && realm.is_none()
        && let Some(seed) = seed.as_deref()
    {
        realm = node::realm_id(&state.http, seed).await;
    }
    let served = seen.and_then(|seen| seen.info.realm.clone());
    let in_realm = served.is_some() && served == realm;

    let mut tracker = state.enroll.lock().expect("enrollment poisoned");
    tracker.cursor = page.cursor;
    if tracker.progress.realm.is_none() {
        tracker.progress.realm = realm;
    }
    tracker.progress.observe(Observed {
        state: node.state,
        ready,
        in_realm,
        refusal,
    });
    if tracker.progress.phase == EnrollPhase::Visible {
        let mut settings = state.settings();
        settings.joined(tracker.progress.realm.clone());
        state.store(settings)?;
        tracker.progress.finish();
    }
    Ok(tracker.progress.detail.clone())
}

fn purge(path: &Path) -> Result<(), DesktopError> {
    if !removable(path, home().as_deref()) {
        return Err(DesktopError::Settings(format!(
            "refusing to delete {}",
            path.display()
        )));
    }
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

/// A directory the desktop provisioned sits inside something: a root, a drive
/// or a home directory was never ours to delete, whatever the settings say.
fn removable(path: &Path, home: Option<&Path>) -> bool {
    path.is_absolute() && path.components().count() > 2 && home.is_none_or(|home| path != home)
}

/// The owner's data is theirs to look at; the rest of the machine is not. Both
/// sides are compared resolved, so no link leads out of a root.
fn revealable(path: &Path, roots: &[PathBuf]) -> bool {
    path.is_absolute()
        && !path.components().any(|part| part == Component::ParentDir)
        && roots.iter().any(|root| path.starts_with(root))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn resolved(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_supervisor_states() {
        // The portal knows four states; the supervisor tracks seven.
        assert_eq!(DeviceState::from(NodeState::Stopping), DeviceState::Stopped);
        assert_eq!(
            DeviceState::from(NodeState::Restarting),
            DeviceState::Starting
        );
        assert_eq!(DeviceState::from(NodeState::Running), DeviceState::Running);
        assert_eq!(
            DeviceState::from(NodeState::BinaryMissing),
            DeviceState::Error
        );
        assert_eq!(DeviceState::from(NodeState::Failed), DeviceState::Error);
    }

    #[test]
    fn names_the_api_base() {
        assert_eq!(
            api_base(&NodeSettings::default()),
            "http://127.0.0.1:34116/api/v1"
        );
    }

    #[test]
    fn opens_web_links_only() {
        assert!(web_url("https://aruna.example/login").is_ok());
        assert!(web_url(" http://127.0.0.1:34115 ").is_ok());
        assert!(web_url("file:///etc/passwd").is_err());
        assert!(web_url("javascript:alert(1)").is_err());
        assert!(web_url("not a url").is_err());
    }

    #[test]
    fn guards_the_wipe_target() {
        let home = Path::new("/home/owner");
        assert!(removable(Path::new("/home/owner/.local/share"), Some(home)));
        assert!(!removable(Path::new("/home/owner"), Some(home)));
        assert!(!removable(Path::new("/"), Some(home)));
        assert!(!removable(Path::new("/data"), Some(home)));
        assert!(!removable(Path::new("relative/path"), Some(home)));
    }

    #[test]
    fn guards_reveal_paths() {
        let roots = [
            PathBuf::from("/home/owner"),
            PathBuf::from("/var/lib/aruna/node"),
        ];
        assert!(revealable(Path::new("/home/owner/data/set.csv"), &roots));
        assert!(revealable(Path::new("/var/lib/aruna/node/storage"), &roots));
        assert!(!revealable(Path::new("/etc/shadow"), &roots));
        assert!(!revealable(Path::new("/home/other/data"), &roots));
        assert!(!revealable(Path::new("relative/path"), &roots));
        // A traversal that lands back inside a root is still not a path the
        // owner named.
        assert!(!revealable(Path::new("/home/owner/../owner/data"), &roots));
    }

    #[test]
    fn restarts_on_startup_keys() {
        let before = NodeSettings {
            storage_path: Some(PathBuf::from("/data/one")),
            ..NodeSettings::default()
        };
        let mut after = before.clone();
        assert!(!restarts(&before, &after));

        after.storage_path = Some(PathBuf::from("/data/two"));
        assert!(restarts(&before, &after));

        let mut computed = before.clone();
        computed.compute.max_concurrent = 4;
        assert!(restarts(&before, &computed));

        // A pause is not a restart: it stops the node outright.
        let mut paused = before.clone();
        paused.paused = true;
        assert!(!restarts(&before, &paused));
    }

    #[test]
    fn reads_a_settings_patch() {
        // The portal sends whichever fields it changed, in camelCase.
        let patch: SettingsPatch = serde_json::from_value(serde_json::json!({
            "storagePath": "/home/owner/aruna",
            "paused": true,
        }))
        .expect("the portal's shape");
        assert_eq!(patch.storage_path.as_deref(), Some("/home/owner/aruna"));
        assert_eq!(patch.paused, Some(true));
        assert!(patch.auto_start.is_none());
    }
}
