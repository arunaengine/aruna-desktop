//! The command channel the portal talks to. One portal build serves both the
//! web and this shell, so the names, arguments and answers here are a contract:
//! the portal's `desktopBridge` module wraps exactly these commands, and one it
//! does not get an answer for is reported as a missing shell half.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::oneshot;

use crate::deep_link::{EnrollInvite, EnrollLink};
use crate::enroll::{EnrollPhase, Observed};
use crate::error::DesktopError;
use crate::node::compute::ComputeProbe;
use crate::node::{self, NodeInfo, NodeState, NodeStatus, RealmMismatch, UpdateRequired};
use crate::oidc::Document;
use crate::realm::{self, RealmTarget};
use crate::settings::{LocalSettings, NodeSettings, SettingsPatch};
use crate::shell::{self, ShellContext};
use crate::state::AppState;
use crate::windows;

/// Every command this shell answers. A name that is not here is rejected as
/// unknown, which the portal reads as a bridge missing that half.
pub const KNOWN: [&str; 17] = [
    "shell_context",
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
    "auth_open",
    "oidc_discovery",
    "oidc_token",
    "validate_realm",
    "enroll_invite_last",
    "app_quit",
];

/// The phrase `wipe_device` demands back before it destroys anything. The
/// portal asks for it too; the shell never trusts that it did.
const WIPE_PHRASE: &str = "wipe";
const REALM_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// How often a running node is checked against the realm's version.
const VERSION_POLL: Duration = Duration::from_secs(60);

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
    /// Whether the node answers yet. `running` is where it starts serving, not
    /// where it was spawned, and the shell context follows this rather than the
    /// state.
    pub ready: bool,
    pub node_id: Option<String>,
    pub realm: Option<String>,
    pub enrolled: bool,
    /// Whether a join is under way: a secret is armed and the realm has not
    /// listed this device yet.
    pub enrolling: bool,
    pub api_base_url: Option<String>,
    pub version: Option<String>,
    pub uptime_seconds: Option<u64>,
    pub message: Option<String>,
    pub detail: Option<String>,
    pub realm_mismatch: Option<RealmMismatch>,
    /// Set while the realm runs another aruna version than this app bundles.
    pub update_required: Option<UpdateRequired>,
}

/// What the shell knows the moment the secret is armed. The node id follows
/// from the realm, not from here.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnrollResult {
    pub node_id: Option<String>,
    pub realm: Option<String>,
}

/// What the portal must talk to right now, in the shape the initialization
/// script injected, minus the bridge. The same payload arrives as
/// `context-changed`; this is how a portal that missed one reads it back.
#[tauri::command]
pub fn shell_context(state: State<'_, AppState>) -> ShellContext {
    state.shell.lock().expect("shell poisoned").clone()
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
    // The probe this call just took outranks the supervisor's last one; without
    // one the node is not running, so its published readiness is all there is.
    let ready = seen.as_ref().map_or(node.ready, |seen| seen.ready);
    let info = seen.map(|seen| seen.info).unwrap_or_default();
    Ok(DeviceStatus {
        state: node.state.into(),
        ready,
        node_id: None,
        realm: info.realm.or(settings.realm.clone()),
        enrolled: settings.enrolled,
        enrolling: enrolling(&settings),
        // A spawned node has bound nothing yet, so naming its base would be an
        // address that refuses the call.
        api_base_url: ready.then(|| api_base(&settings)),
        version: info.version,
        uptime_seconds: state.supervisor.uptime(),
        message: joining
            .or(node.detail.clone())
            .or_else(|| settings.paused.then(|| "the node is paused".to_string())),
        detail: node.detail,
        realm_mismatch: node.realm_mismatch,
        update_required: node.update_required,
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
/// node and resuming starts it; where it stores data, what it may compute and
/// its S3 listener are read at startup, so changing one restarts a live node.
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
            state.supervisor.stop().await;
            if let Err(error) = start_node(&state).await {
                tracing::warn!(%error, "Could not restart the node");
            }
        }
        // A device with no node binary stays a usable app, so a failed resume
        // is left in the status rather than refusing the setting.
        Some(false) if !live => {
            if let Err(error) = start_node(&state).await {
                tracing::warn!(%error, "Could not resume the node");
            }
        }
        Some(false) => {}
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

/// Shows a path in the owner's file manager. Nothing is read or written, and
/// the owner's data lives wherever they put it, so any absolute path that is
/// there may be shown.
#[tauri::command]
pub fn reveal_path(app: AppHandle, path: String) -> Result<(), DesktopError> {
    let target = reveal_target(&path)?;
    app.opener()
        .reveal_item_in_dir(&target)
        .map_err(|error| DesktopError::Unreachable(error.to_string()))
}

/// What a reveal may act on: a path the owner named absolutely, resolved, which
/// is also how one that is not there is refused.
fn reveal_target(raw: &str) -> Result<PathBuf, DesktopError> {
    let path = Path::new(raw.trim());
    if !path.is_absolute() {
        return Err(DesktopError::Settings(format!(
            "{} must be an absolute path",
            path.display()
        )));
    }
    fs::canonicalize(path)
        .map_err(|_| DesktopError::Settings(format!("there is no {} to show", path.display())))
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
    if let (Some(site), Some(seed)) = (link.site.as_deref(), link.seed.as_deref()) {
        settings.remember_site(site, seed);
    }
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
    state.supervisor.stop().await;
    start_node(&state).await?;
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
    may_join(&state.settings())?;
    let origin = realm::normalize(&input)?;
    let target = realm::check(&state.http, &origin)
        .await
        .map_err(|reach| DesktopError::Unreachable(reach.message(&origin)))?;

    let mut settings = state.settings();
    settings.remember_realm(
        &target.origin,
        &target.api_base_url,
        target.api_version.as_deref(),
    );
    state.store(settings)?;
    shell::sync(&app);
    Ok(target)
}

/// Whether a join is running: the secret is spent the moment the realm lists
/// this device, so an armed one that has not joined is one in flight.
fn enrolling(settings: &NodeSettings) -> bool {
    settings.onboarding_secret.is_some() && !settings.enrolled
}

/// Whether this device may still be pointed at a realm. A device that joined
/// one belongs to it: its node, its keys and its data are that realm's, so
/// another realm is a wipe away rather than a setting.
fn may_join(settings: &NodeSettings) -> Result<(), DesktopError> {
    if settings.enrolled {
        return Err(DesktopError::Refused(
            "wipe this device before joining another realm".into(),
        ));
    }
    Ok(())
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
    // Before the stop, so a start still waiting on its probe cannot follow it.
    state.invalidate();
    state.supervisor.stop().await;

    let settings = state.settings();
    let storage = settings.storage_dir(&state.data_dir);
    if !storage.starts_with(&state.data_dir) {
        purge(&app, &storage)?;
    }
    purge(&app, &state.data_dir)?;

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

/// Opens the provider's sign-in page in a window of the app's own, which
/// catches the redirect back itself. Only a web address is opened.
#[tauri::command]
pub fn auth_open(app: AppHandle, url: String) -> Result<(), DesktopError> {
    let target = url::Url::parse(&web_url(&url)?)
        .map_err(|error| DesktopError::Link(format!("unreadable URL: {error}")))?;
    windows::open_login(&app, target)?;
    Ok(())
}

/// Fetches a provider's discovery document, which the portal cannot fetch from
/// the app's own origin. The token endpoint it names is what `oidc_token` may
/// post to for the rest of this run.
#[tauri::command]
pub async fn oidc_discovery(
    state: State<'_, AppState>,
    url: String,
) -> Result<Document, DesktopError> {
    state.oidc.discover(&url).await
}

/// Posts the portal's token request form to an endpoint `oidc_discovery`
/// returned in this run, and answers the provider's JSON.
#[tauri::command]
pub async fn oidc_token(
    state: State<'_, AppState>,
    endpoint: String,
    form: BTreeMap<String, String>,
) -> Result<Document, DesktopError> {
    state.oidc.request_token(&endpoint, &form).await
}

/// Ends the app, stopping the node first: the portal owns no window and the
/// node's own shutdown is what keeps its data whole.
#[tauri::command]
pub async fn app_quit(app: AppHandle) -> Result<(), DesktopError> {
    windows::quit(app).await;
    Ok(())
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

/// Whether the node has to be restarted for a patch to reach it. All three are
/// read once at startup, so a later start would be too late.
fn restarts(before: &NodeSettings, after: &NodeSettings) -> bool {
    before.storage_path != after.storage_path
        || before.compute != after.compute
        || before.s3_enabled != after.s3_enabled
}

/// The node's REST base, the same one the portal is told to talk to.
pub fn api_base(settings: &NodeSettings) -> String {
    format!("{}/api/v1", settings.api_url())
}

pub(crate) async fn start_node(state: &AppState) -> Result<NodeStatus, DesktopError> {
    let settings = state.settings();
    let mut generation = settings.generation;
    let Some(realm_url) = realm_to_check(&settings) else {
        return state.supervisor.start(generation);
    };
    if let Some(target) = check_realm(state, realm_url).await {
        if settings.enrolled
            && let Some((expected, actual)) =
                realm_mismatch(settings.realm.as_deref(), target.realm.as_deref())
        {
            return Ok(state.supervisor.record_realm_mismatch(RealmMismatch {
                expected,
                actual,
                realm_url: realm_url.to_string(),
            }));
        }
        // An answer that lost to a wipe, a realm change or a newer probe ends
        // this start; whatever changed has its own say in what runs.
        let Some(current) =
            remember_version(state, realm_url, generation, target.api_version.as_deref())
        else {
            return Ok(state.supervisor.status());
        };
        generation = current;
    }
    // The start itself weighs the stored version, the pause, a quit and the
    // generation, as they are when it claims the run slot.
    state.supervisor.start(generation)
}

async fn check_realm(state: &AppState, realm_url: &str) -> Option<RealmTarget> {
    tokio::time::timeout(REALM_PROBE_TIMEOUT, realm::check(&state.http, realm_url))
        .await
        .ok()?
        .ok()
}

/// Stores what a probe of `realm_url` reported, which began under `generation`.
/// `None` when it is out of date and was dropped; otherwise the generation the
/// caller carries on with.
fn remember_version(
    state: &AppState,
    realm_url: &str,
    generation: u64,
    version: Option<&str>,
) -> Option<u64> {
    let Some(version) = version else {
        return Some(generation);
    };
    match state.note_version(realm_url, generation, version) {
        Ok(current) => current,
        Err(error) => {
            tracing::warn!(%error, "Could not store the realm version");
            Some(state.settings().generation)
        }
    }
}

/// The realm a node start must agree with: only a device that joined, or is
/// joining, belongs to one.
fn realm_to_check(settings: &NodeSettings) -> Option<&str> {
    settings
        .joins_realm()
        .then_some(settings.realm_url.as_deref())
        .flatten()
}

/// Keeps asking the realm for its version while a node runs, and stops the node
/// once the realm moved on.
pub(crate) async fn watch_version(app: AppHandle) {
    loop {
        tokio::time::sleep(VERSION_POLL).await;
        let state = app.state::<AppState>();
        let live = matches!(
            state.supervisor.status().state,
            NodeState::Starting | NodeState::Running
        );
        let settings = state.settings();
        let Some(realm_url) = realm_to_check(&settings).filter(|_| live) else {
            continue;
        };
        let Some(target) = check_realm(&state, realm_url).await else {
            continue;
        };
        let generation = settings.generation;
        if remember_version(&state, realm_url, generation, target.api_version.as_deref()).is_none()
        {
            continue;
        }
        if let Some(required) = node::version_gap(target.api_version.as_deref()) {
            tracing::warn!(realm = %required.realm, "The realm runs another aruna version");
            state.supervisor.stop_for_update(required).await;
            shell::sync(&app);
        }
    }
}

fn realm_mismatch(settings_realm: Option<&str>, probed: Option<&str>) -> Option<(String, String)> {
    let expected = settings_realm?;
    let actual = probed?;
    (expected != actual).then(|| (expected.to_string(), actual.to_string()))
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

fn purge(app: &AppHandle, path: &Path) -> Result<(), DesktopError> {
    if !removable(path, home(app).as_deref()) {
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

/// The owner's home as the platform reports it, which `HOME` is not on Windows.
fn home(app: &AppHandle) -> Option<PathBuf> {
    app.path().home_dir().ok()
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
    fn serializes_status_fields() {
        let status = DeviceStatus {
            state: DeviceState::Stopped,
            ready: false,
            node_id: None,
            realm: Some("realm-old".into()),
            enrolled: true,
            enrolling: false,
            api_base_url: None,
            version: None,
            uptime_seconds: None,
            message: Some("the realm changed".into()),
            detail: Some("the realm changed".into()),
            realm_mismatch: Some(RealmMismatch {
                expected: "realm-old".into(),
                actual: "realm-new".into(),
                realm_url: "https://aruna.example".into(),
            }),
            update_required: Some(UpdateRequired {
                local: "3.0.0-alpha.1".into(),
                realm: "3.0.0-alpha.2".into(),
            }),
        };
        let value = serde_json::to_value(status).expect("a serializable status");

        assert_eq!(
            value.get("detail"),
            Some(&serde_json::json!("the realm changed"))
        );
        assert_eq!(
            value.get("realmMismatch"),
            Some(&serde_json::json!({
                "expected": "realm-old",
                "actual": "realm-new",
                "realmUrl": "https://aruna.example"
            }))
        );
        assert_eq!(
            value.get("updateRequired"),
            Some(&serde_json::json!({
                "local": "3.0.0-alpha.1",
                "realm": "3.0.0-alpha.2"
            }))
        );
    }

    #[test]
    fn checks_joined_realm_only() {
        let mut settings = NodeSettings::default();
        settings.remember_realm(
            "https://aruna.example",
            "https://aruna.example/api/v1",
            None,
        );
        assert_eq!(realm_to_check(&settings), None);

        settings.onboarding_secret = Some("secret".into());
        assert_eq!(realm_to_check(&settings), Some("https://aruna.example"));

        settings.onboarding_secret = None;
        settings.joined(Some("r-1".into()));
        assert_eq!(realm_to_check(&settings), Some("https://aruna.example"));
    }

    #[test]
    fn names_the_api_base() {
        assert_eq!(
            api_base(&NodeSettings::default()),
            "http://127.0.0.1:34116/api/v1"
        );
    }

    #[test]
    fn detects_recreated_realm() {
        assert_eq!(
            realm_mismatch(Some("realm-old"), Some("realm-new")),
            Some(("realm-old".into(), "realm-new".into()))
        );
        assert_eq!(realm_mismatch(Some("realm-old"), Some("realm-old")), None);
    }

    #[test]
    fn ignores_unknown_realm_ids() {
        assert_eq!(realm_mismatch(None, Some("realm-new")), None);
        assert_eq!(realm_mismatch(Some("realm-old"), None), None);
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
    fn checks_reveal_target() {
        // Relative input would resolve against the app's own directory, and a
        // path that is not there has nothing to show.
        assert!(reveal_target("ui").is_err());
        assert!(reveal_target("/aruna-desktop-no-such-path").is_err());
        let here = env!("CARGO_MANIFEST_DIR");
        assert_eq!(
            reveal_target(&format!(" {here} ")).expect("this crate"),
            fs::canonicalize(here).expect("this crate")
        );
    }

    #[test]
    fn restarts_startup_keys() {
        let before = NodeSettings {
            storage_path: Some(PathBuf::from("/data/one")),
            ..NodeSettings::default()
        };
        let mut after = before.clone();
        assert!(!restarts(&before, &after));

        after.storage_path = Some(PathBuf::from("/data/two"));
        assert!(restarts(&before, &after));

        let mut computed = before.clone();
        computed.compute.max_concurrent = Some(4);
        assert!(restarts(&before, &computed));

        let mut without_s3 = before.clone();
        without_s3.s3_enabled = !before.s3_enabled;
        assert!(restarts(&before, &without_s3));

        // A pause is not a restart: it stops the node outright.
        let mut paused = before.clone();
        paused.paused = true;
        assert!(!restarts(&before, &paused));
    }

    #[test]
    fn refuses_second_realm() {
        // A wipe keeps the remembered realm, so the device still has one to
        // sign in to; joining another is what it refuses.
        let mut settings = NodeSettings::default();
        assert!(may_join(&settings).is_ok());

        settings.remember_realm(
            "https://aruna.example",
            "https://aruna.example/api/v1",
            None,
        );
        settings.joined(Some("r-1".into()));
        let refusal = may_join(&settings).expect_err("an enrolled device");
        assert!(refusal.to_string().contains("wipe this device"));

        settings.wipe();
        assert!(may_join(&settings).is_ok());
        assert_eq!(settings.realm_url.as_deref(), Some("https://aruna.example"));
    }

    #[test]
    fn reports_a_running_join() {
        // The armed secret is the join: it is spent the moment the realm lists
        // this device.
        let mut settings = NodeSettings::default();
        assert!(!enrolling(&settings));

        settings.enroll("secret".into(), Some("https://seed.example".into()), None);
        assert!(enrolling(&settings));

        settings.joined(Some("r-1".into()));
        assert!(!enrolling(&settings));
    }

    #[test]
    fn reads_a_settings_patch() {
        // The portal sends whichever fields it changed, in camelCase.
        let patch: SettingsPatch = serde_json::from_value(serde_json::json!({
            "storagePath": "/home/owner/aruna",
            "s3Enabled": false,
            "paused": true,
        }))
        .expect("the portal's shape");
        assert_eq!(patch.storage_path.as_deref(), Some("/home/owner/aruna"));
        assert_eq!(patch.s3_enabled, Some(false));
        assert_eq!(patch.paused, Some(true));
    }
}
