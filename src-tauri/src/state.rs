use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::deep_link::EnrollPayload;
use crate::error::DesktopError;
use crate::node::Supervisor;
use crate::settings::NodeSettings;

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
    /// Payload of the last consumed `aruna://enroll` link, awaiting the view.
    pub pending: Mutex<Option<EnrollPayload>>,
}

impl AppState {
    pub fn build(app: &AppHandle) -> Result<Self, DesktopError> {
        let paths = app.path();
        let settings_path = paths.app_config_dir()?.join("settings.json");
        let data_dir = paths.app_data_dir()?.join("node");
        let resources = paths.resource_dir().ok();

        let settings = Arc::new(Mutex::new(NodeSettings::load(&settings_path)));
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
        })
    }

    pub fn settings(&self) -> NodeSettings {
        self.settings.lock().expect("settings poisoned").clone()
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
