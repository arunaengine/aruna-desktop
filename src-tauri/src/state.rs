use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::deep_link::EnrollLink;
use crate::enroll::{EnrollProgress, Tracker};
use crate::error::DesktopError;
use crate::node::Supervisor;
use crate::settings::NodeSettings;
use crate::shell::{self, Shell};

/// Paths the settings view shows so the owner can find the node's data.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Paths {
    pub config_file: String,
    pub data_dir: String,
    pub storage_dir: String,
    pub env_file: String,
}

pub struct AppState {
    pub settings: Arc<Mutex<NodeSettings>>,
    pub settings_path: PathBuf,
    pub data_dir: PathBuf,
    pub supervisor: Supervisor,
    pub http: reqwest::Client,
    /// The last `aruna://enroll` link, waiting for the owner to accept it. It
    /// carries the one-time secret, so it never leaves the backend.
    pub pending: Mutex<Option<EnrollLink>>,
    pub enroll: Mutex<Tracker>,
    /// The app window and the context its webview was opened with.
    pub shell: Mutex<Shell>,
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
            pending: Mutex::new(None),
            enroll: Mutex::new(tracker),
            shell: Mutex::new(Shell::new(context)),
        })
    }

    pub fn main_label(&self) -> String {
        self.shell.lock().expect("shell poisoned").label.clone()
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

    pub fn progress(&self) -> EnrollProgress {
        self.enroll
            .lock()
            .expect("enrollment poisoned")
            .progress
            .clone()
    }

    pub fn paths(&self) -> Paths {
        let settings = self.settings();
        Paths {
            config_file: self.settings_path.display().to_string(),
            data_dir: self.data_dir.display().to_string(),
            storage_dir: settings.storage_dir(&self.data_dir).display().to_string(),
            env_file: self.data_dir.join(".env").display().to_string(),
        }
    }
}
