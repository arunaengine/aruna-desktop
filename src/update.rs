//! In-app updates to the aruna version the realm runs. Every release publishes a
//! signed `latest.json`, so the app asks the release of exactly that version and
//! never installs one older than itself.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Error, Update, UpdaterExt};
use url::Url;

use crate::error::DesktopError;
use crate::state::AppState;
use crate::windows;

const PROGRESS_EVENT: &str = "update-progress";
/// How many downloaded bytes pass between two progress events.
const PROGRESS_STEP: u64 = 1 << 20;

/// Set while an update downloads or installs, so a second request cannot race it.
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// A signed build of the realm's version that this app can install itself.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOffer {
    pub version: String,
}

/// The payload of `update-progress`. `total` is unknown when the server sends no length.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
    #[serde(skip)]
    reported: u64,
}

impl Progress {
    /// Counts a chunk and tells whether it is time for another event.
    fn advance(&mut self, chunk: usize, total: Option<u64>) -> bool {
        self.downloaded += chunk as u64;
        self.total = total;
        let due =
            self.downloaded - self.reported >= PROGRESS_STEP || total == Some(self.downloaded);
        if due {
            self.reported = self.downloaded;
        }
        due
    }
}

/// The signed build of the realm's version for this platform and installer.
/// `None` when the realm needs no update, is older, or its release has no such
/// build, which includes every build made without an update signing key.
pub async fn find(app: &AppHandle, state: &AppState) -> Result<Option<Update>, DesktopError> {
    let Some(required) = state.supervisor.status().update_required else {
        return Ok(None);
    };
    let Some(endpoint) = release_url(&required.realm) else {
        return Ok(None);
    };
    if !signed(app) {
        return Ok(None);
    }
    let target = required.realm;
    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint])?
        .version_comparator(move |current, remote| {
            remote.version > current && remote.version.to_string() == target
        })
        .build()?;
    match updater.check().await {
        Ok(update) => Ok(update),
        Err(Error::ReleaseNotFound | Error::TargetNotFound(_) | Error::TargetsNotFound(_)) => {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

/// Downloads, verifies and installs the realm's version. Windows ends the app
/// in the installer and starts the new one; elsewhere the portal asks for the restart.
pub async fn install(app: &AppHandle, state: &AppState) -> Result<(), DesktopError> {
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err(DesktopError::Refused(
            "the update is already being installed".into(),
        ));
    }
    let result = download_install(app, state).await;
    if result.is_err() {
        INSTALLING.store(false, Ordering::SeqCst);
    }
    result
}

async fn download_install(app: &AppHandle, state: &AppState) -> Result<(), DesktopError> {
    let update = find(app, state).await?.ok_or_else(|| {
        DesktopError::Refused("there is no signed update for this computer".into())
    })?;
    let mut progress = Progress::default();
    let bytes = update
        .download(
            |chunk, total| {
                if progress.advance(chunk, total)
                    && let Err(error) = app.emit_to(windows::MAIN_LABEL, PROGRESS_EVENT, progress)
                {
                    tracing::warn!(%error, "Could not report the update progress");
                }
            },
            || {},
        )
        .await?;
    // The Windows installer ends this process, so the node is stopped first.
    state.supervisor.stop().await;
    update.install(bytes)?;
    tracing::info!(version = %update.version, "Installed the update");
    Ok(())
}

/// Whether this build carries the public key update signatures are checked with.
fn signed(app: &AppHandle) -> bool {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("pubkey"))
        .and_then(|key| key.as_str())
        .is_some_and(|key| !key.trim().is_empty())
}

/// The update manifest of the release tagged with `version`. Only a plain
/// version is put into the URL.
fn release_url(version: &str) -> Option<Url> {
    let plain = version.starts_with(|c: char| c.is_ascii_digit())
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !plain {
        return None;
    }
    let repository = env!("CARGO_PKG_REPOSITORY");
    Url::parse(&format!(
        "{repository}/releases/download/v{version}/latest.json"
    ))
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_url_names_tag() {
        let url = release_url("3.0.0-alpha.140").expect("a plain version");
        assert_eq!(
            url.as_str(),
            "https://github.com/arunaengine/aruna-desktop/releases/download/v3.0.0-alpha.140/latest.json"
        );
    }

    #[test]
    fn release_url_rejects_paths() {
        for version in [
            "",
            "../latest",
            "3.0.0/../../x",
            "3.0.0+build",
            "v3.0.0",
            "3.0.0?x=1",
        ] {
            assert_eq!(release_url(version), None, "{version}");
        }
    }

    #[test]
    fn progress_reports_steps() {
        let mut progress = Progress::default();
        let total = Some(3 * PROGRESS_STEP);
        assert!(!progress.advance(1024, total));
        assert!(progress.advance(PROGRESS_STEP as usize, total));
        assert!(!progress.advance(1024, total));
        let rest = (3 * PROGRESS_STEP - progress.downloaded) as usize;
        assert!(
            progress.advance(rest, total),
            "the last chunk is always reported"
        );
        assert_eq!(progress.downloaded, 3 * PROGRESS_STEP);
    }

    #[test]
    fn progress_without_length() {
        let mut progress = Progress::default();
        assert!(!progress.advance(10, None));
        assert!(progress.advance(PROGRESS_STEP as usize, None));
        assert_eq!(progress.total, None);
    }
}
