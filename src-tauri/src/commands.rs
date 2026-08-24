use tauri::{AppHandle, State};

use crate::deep_link::{self, EnrollInvite, EnrollLink};
use crate::enroll::{self, EnrollPhase, EnrollProgress, Observed};
use crate::error::DesktopError;
use crate::node::{self, HealthReport, LogPage, NodeState, NodeStatus};
use crate::portal::{self, PortalOptions, PortalPlan, PortalTarget};
use crate::realm::{self, RealmTarget};
use crate::route::{self, Landing};
use crate::settings::NodeSettings;
use crate::state::{AppState, Paths};
use crate::windows;

/// The view the control window opens on. The backend owns the rule, because it
/// owns what it depends on: the remembered realm and any waiting invitation.
#[tauri::command]
pub fn launch_route(state: State<'_, AppState>) -> Landing {
    let invited = state
        .pending
        .lock()
        .expect("pending enrollment poisoned")
        .is_some();
    route::landing(
        state.settings().realm_url.as_deref(),
        invited,
        state.progress().active(),
    )
}

#[tauri::command]
pub fn node_start(state: State<'_, AppState>) -> Result<NodeStatus, DesktopError> {
    state.supervisor.start()
}

#[tauri::command]
pub async fn node_stop(state: State<'_, AppState>) -> Result<NodeStatus, DesktopError> {
    Ok(state.supervisor.stop().await)
}

#[tauri::command]
pub async fn node_restart(state: State<'_, AppState>) -> Result<NodeStatus, DesktopError> {
    state.supervisor.restart().await
}

#[tauri::command]
pub fn node_logs(state: State<'_, AppState>, cursor: u64) -> LogPage {
    state.supervisor.logs().page(cursor)
}

#[tauri::command]
pub async fn node_health(state: State<'_, AppState>) -> Result<HealthReport, DesktopError> {
    let port = state.settings().ports.ops;
    Ok(node::probe(&state.http, port).await)
}

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> NodeSettings {
    state.settings().redacted()
}

/// Persists the settings. They reach the node on its next start, which the
/// settings view makes explicit rather than restarting behind the owner's back.
/// The enrollment fields are kept from the stored copy, because the view never
/// receives the one-time secret and so cannot send it back.
#[tauri::command]
pub fn settings_save(
    state: State<'_, AppState>,
    settings: NodeSettings,
) -> Result<NodeSettings, DesktopError> {
    let merged = state.settings().merge_edits(settings);
    merged.validate()?;
    state.store(merged.clone())?;
    Ok(merged.redacted())
}

#[tauri::command]
pub fn paths_get(state: State<'_, AppState>) -> Paths {
    state.paths()
}

/// Shows the enroll view what a stashed deep link offers, without its secret.
#[tauri::command]
pub fn enroll_pending(state: State<'_, AppState>) -> Option<EnrollInvite> {
    state
        .pending
        .lock()
        .expect("pending enrollment poisoned")
        .as_ref()
        .map(EnrollLink::invite)
}

/// Enrolls with the stashed deep link.
#[tauri::command]
pub async fn enroll_accept(state: State<'_, AppState>) -> Result<EnrollProgress, DesktopError> {
    let link = state
        .pending
        .lock()
        .expect("pending enrollment poisoned")
        .take()
        .ok_or_else(|| DesktopError::DeepLink("no invitation is waiting".into()))?;
    arm(&state, link).await
}

/// The paste fallback: a whole `aruna://enroll` link, or a bare secret with the
/// seed URL typed beside it.
#[tauri::command]
pub async fn enroll_paste(
    state: State<'_, AppState>,
    text: String,
    seed: String,
    realm: String,
) -> Result<EnrollProgress, DesktopError> {
    arm(&state, deep_link::parse_paste(&text, &seed, &realm)?).await
}

/// Stores the one-time secret and restarts the node, which consumes it while
/// it boots. Everything after this is observed, never commanded.
async fn arm(state: &AppState, link: EnrollLink) -> Result<EnrollProgress, DesktopError> {
    let seed = link.seed.clone();
    let realm = link.realm.clone();

    let mut settings = state.settings();
    settings.enroll(link.secret, seed.clone(), realm.clone());
    settings.validate()?;
    state.store(settings)?;

    {
        let mut tracker = state.enroll.lock().expect("enrollment poisoned");
        tracker.progress.accept(seed.as_deref(), realm.as_deref());
        tracker.cursor = 0;
    }

    state.supervisor.restart().await?;
    Ok(state.progress())
}

/// Advances the enrollment from what can be observed: the supervisor's state,
/// the node's output, the ops readiness probe, and the realm the node serves.
#[tauri::command]
pub async fn enroll_poll(state: State<'_, AppState>) -> Result<EnrollProgress, DesktopError> {
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
        return Ok(state.progress());
    }

    let status = state.supervisor.status();
    let page = state.supervisor.logs().page(cursor);
    let refusal = page
        .lines
        .iter()
        .find_map(|line| enroll::refusal(&line.text));

    let settings = state.settings();
    let ready = status.state == NodeState::Running
        && node::probe(&state.http, settings.ports.ops).await.ready.ok;

    // A pasted bare secret names no realm, so the seed is asked which one it
    // serves; the seed cannot list its nodes to an anonymous caller.
    if ready
        && realm.is_none()
        && let Some(seed) = seed.as_deref()
    {
        realm = node::realm_id(&state.http, seed).await;
    }
    let served = match ready {
        true => node::realm_id(&state.http, &settings.api_url()).await,
        false => None,
    };
    let in_realm = served.is_some() && served == realm;

    let mut tracker = state.enroll.lock().expect("enrollment poisoned");
    tracker.cursor = page.cursor;
    if tracker.progress.realm.is_none() {
        tracker.progress.realm = realm;
    }
    tracker.progress.observe(Observed {
        state: status.state,
        ready,
        in_realm,
        refusal,
    });
    if tracker.progress.phase == EnrollPhase::Visible {
        // The secret is spent only once the node serves the realm: dropping it
        // while the join is half done leaves the node unable to finish.
        let mut settings = state.settings();
        settings.joined(tracker.progress.realm.clone());
        state.store(settings)?;
        tracker.progress.finish();
    }
    Ok(tracker.progress.clone())
}

/// Validates a realm address, remembers it and opens its portal window. The
/// login itself happens in that window, like it would in any browser.
#[tauri::command]
pub async fn realm_connect(
    app: AppHandle,
    state: State<'_, AppState>,
    address: String,
) -> Result<RealmTarget, DesktopError> {
    let origin = realm::normalize(&address)?;
    let target = realm::check(&state.http, &origin)
        .await
        .map_err(|reach| DesktopError::Unreachable(reach.message(&origin)))?;

    let mut settings = state.settings();
    settings.remember_realm(&target.url);
    state.store(settings)?;

    windows::open_portal(&app, &target.url, None)?;
    Ok(target)
}

/// Reports which portals can be opened right now, so a view can name its target
/// and an unreachable one can offer the other.
#[tauri::command]
pub async fn portal_options(state: State<'_, AppState>) -> Result<PortalOptions, DesktopError> {
    let settings = state.settings();
    Ok(portal::options(&settings, ready(&state, &settings).await))
}

/// Opens a portal window, defaulting to the remembered realm. The target is
/// probed first: a refused or foreign address becomes an error the control
/// window can explain, never a raw webview failure page.
#[tauri::command]
pub async fn portal_open(
    app: AppHandle,
    state: State<'_, AppState>,
    target: Option<PortalTarget>,
) -> Result<PortalPlan, DesktopError> {
    let settings = state.settings();
    let plan = portal::plan(&settings, ready(&state, &settings).await, target)?;
    realm::check(&state.http, &plan.url)
        .await
        .map_err(|reach| DesktopError::Unreachable(reach.message(&plan.url)))?;
    windows::open_portal(&app, &plan.url, None)?;
    Ok(plan)
}

/// Whether the embedded node can serve anything at all. The probe is skipped
/// while the supervisor knows the node is not running.
async fn ready(state: &AppState, settings: &NodeSettings) -> bool {
    state.supervisor.status().state == NodeState::Running
        && node::probe(&state.http, settings.ports.ops).await.ready.ok
}
