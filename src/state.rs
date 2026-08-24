use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager};

use crate::enroll::{EnrollProgress, Tracker};
use crate::error::DesktopError;
use crate::node::Supervisor;
use crate::settings::NodeSettings;
use crate::shell::{self, Shell};

pub struct AppState {
    pub settings: Arc<Mutex<NodeSettings>>,
    pub settings_path: PathBuf,
    pub data_dir: PathBuf,
    pub supervisor: Supervisor,
    pub http: reqwest::Client,
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
}
