use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager};

use crate::deep_link::LastInvite;
use crate::enroll::{EnrollProgress, Tracker};
use crate::error::DesktopError;
use crate::node::Supervisor;
use crate::settings::NodeSettings;
use crate::shell::{self, ShellContext};

pub struct AppState {
    pub settings: Arc<Mutex<NodeSettings>>,
    pub settings_path: PathBuf,
    pub data_dir: PathBuf,
    pub supervisor: Supervisor,
    pub http: reqwest::Client,
    pub enroll: Mutex<Tracker>,
    /// The last `aruna://enroll` outcome, for a portal that missed its event.
    pub invite: LastInvite,
    /// What the portal was last told to talk to: injected at boot, emitted as
    /// `context-changed` afterwards, and answered by `shell_context`.
    pub shell: Mutex<ShellContext>,
    /// Set once a quit owns the shutdown, so a second one does not stop the
    /// node again while the first is still waiting for it.
    quitting: AtomicBool,
}

impl AppState {
    pub fn build(app: &AppHandle) -> Result<Self, DesktopError> {
        let paths = app.path();
        let settings_path = paths.app_config_dir()?.join("settings.json");
        let data_dir = paths.app_data_dir()?.join("node");
        let resources = paths.resource_dir().ok();

        let stored = NodeSettings::load(&settings_path);
        let mut tracker = Tracker::default();
        if stored.enrolled {
            tracker
                .progress
                .settle(stored.seed_url.clone(), stored.realm.clone());
        }

        let context = shell::context(&stored, false);
        let settings = Arc::new(Mutex::new(stored));
        let supervisor = Supervisor::new(
            app.clone(),
            data_dir.clone(),
            resources,
            Arc::clone(&settings),
        );

        Ok(Self {
            settings,
            settings_path,
            data_dir,
            supervisor,
            http: reqwest::Client::new(),
            enroll: Mutex::new(tracker),
            invite: LastInvite::default(),
            shell: Mutex::new(context),
            quitting: AtomicBool::new(false),
        })
    }

    /// Whether this caller owns the shutdown. Only the first one does; a later
    /// close request leaves the stop that is already running to finish.
    pub fn claim_quit(&self) -> bool {
        !self.quitting.swap(true, Ordering::SeqCst)
    }

    pub fn settings(&self) -> NodeSettings {
        self.settings.lock().expect("settings poisoned").clone()
    }

    /// Replaces the stored settings and the file behind them together, so the
    /// next node start and the next launch agree.
    pub fn store(&self, settings: NodeSettings) -> Result<(), DesktopError> {
        settings.save(&self.settings_path)?;
        *self.settings.lock().expect("settings poisoned") = settings;
        Ok(())
    }

    /// Keeps the realm's reported version with the settings, so it outlives
    /// the launch. Nothing is written while it is unchanged.
    pub fn note_version(&self, version: &str) -> Result<(), DesktopError> {
        let mut settings = self.settings.lock().expect("settings poisoned");
        if settings.realm_version.as_deref() == Some(version) {
            return Ok(());
        }
        let mut changed = settings.clone();
        changed.realm_version = Some(version.to_string());
        changed.save(&self.settings_path)?;
        *settings = changed;
        Ok(())
    }

    pub fn progress(&self) -> EnrollProgress {
        self.enroll
            .lock()
            .expect("enrollment poisoned")
            .progress
            .clone()
    }
}
