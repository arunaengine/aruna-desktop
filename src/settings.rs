use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::DesktopError;
use crate::node::compute::{ComputePlan, Executor};

/// Loopback ports the desktop pins for the local node. They are fixed so the
/// portal origin and the OIDC redirect URI stay registrable up front.
pub const PORTAL_PORT: u16 = 34115;
pub const API_PORT: u16 = 34116;
pub const OPS_PORT: u16 = 34117;
pub const S3_PORT: u16 = 34118;
pub const P2P_PORT: u16 = 34119;

/// Dev override for the node binary, checked before the bundled sidecar.
pub const NODE_BIN_ENV: &str = "ARUNA_DESKTOP_NODE_BIN";

/// Origins the app window can carry. Tauri serves the portal from its own
/// scheme, which is `tauri://localhost` everywhere except Windows and Android;
/// a dev run serves it from the website's vite server instead. The node must
/// allow all of them, or the portal's own API calls are refused.
const APP_ORIGINS: [&str; 3] = [
    "tauri://localhost",
    "http://tauri.localhost",
    "http://localhost:1420",
];

/// The node refuses a shorter grace and falls back to its own default.
pub const MIN_GRACE_SECS: u64 = 16;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodePorts {
    pub portal: u16,
    pub api: u16,
    pub ops: u16,
    pub s3: u16,
    pub p2p: u16,
}

impl Default for NodePorts {
    fn default() -> Self {
        Self {
            portal: PORTAL_PORT,
            api: API_PORT,
            ops: OPS_PORT,
            s3: S3_PORT,
            p2p: P2P_PORT,
        }
    }
}

/// Which executor the node runs the owner's local jobs in. `Auto` takes
/// whatever this machine turns out to have; `Off` runs nothing locally.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ComputeBackend {
    #[default]
    Auto,
    Docker,
    Apptainer,
    Off,
}

/// What the owner lets local jobs use. Every ceiling is optional and unset
/// means unlimited: the machine is the owner's, so the shell states no policy
/// of its own beyond running one job at a time.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct ComputeSettings {
    pub backend: ComputeBackend,
    pub max_concurrent: Option<u32>,
    pub max_cpu_cores: Option<u32>,
    pub max_ram_bytes: Option<u64>,
    pub max_disk_bytes: Option<u64>,
    /// Keeps a failed job's container and workspace for inspection.
    pub keep_failed: bool,
}

impl Default for ComputeSettings {
    fn default() -> Self {
        Self {
            backend: ComputeBackend::Auto,
            max_concurrent: Some(1),
            max_cpu_cores: None,
            max_ram_bytes: None,
            max_disk_bytes: None,
            keep_failed: false,
        }
    }
}

impl ComputeSettings {
    fn validate(&self) -> Result<(), DesktopError> {
        let zero = self.max_concurrent == Some(0)
            || self.max_cpu_cores == Some(0)
            || self.max_ram_bytes == Some(0)
            || self.max_disk_bytes == Some(0);
        match zero {
            true => Err(DesktopError::Settings(
                "a compute limit may not be zero; leave it unset for no limit".into(),
            )),
            false => Ok(()),
        }
    }
}

/// The local control surface the portal renders: where the node stores data,
/// what it may compute, whether it serves S3 and whether it runs. Everything
/// else the desktop keeps is machine configuration the owner never has to see.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalSettings {
    pub storage_path: String,
    pub compute: ComputeSettings,
    pub s3_enabled: bool,
    pub paused: bool,
}

/// A partial edit of [`LocalSettings`]; the portal may send any subset.
/// `compute` is replaced whole, because an absent ceiling means unlimited.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsPatch {
    pub storage_path: Option<String>,
    pub compute: Option<ComputeSettings>,
    pub s3_enabled: Option<bool>,
    pub paused: Option<bool>,
}

/// Everything the desktop needs to launch and reach its node. Serialized as the
/// node's env file, so every field maps to a documented aruna config key.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct NodeSettings {
    /// Overrides both the dev env var and the bundled sidecar when set.
    pub binary_path: Option<PathBuf>,
    /// Defaults to `<app data>/node/storage`.
    pub storage_path: Option<PathBuf>,
    /// What the owner's local jobs may run in and use.
    pub compute: ComputeSettings,
    /// Keeps the node stopped until the owner resumes it, across launches.
    pub paused: bool,
    /// The device name the owner typed when enrolling. Display only: the realm
    /// names the device itself, and the node takes no label from the desktop.
    pub device_label: Option<String>,
    /// Portal artifact directory; the portal listener stays off while unset.
    pub portal_dir: Option<PathBuf>,
    pub ports: NodePorts,
    /// On by default: the node serves the owner their own S3 endpoint on
    /// loopback. Off writes an empty `S3_HOST` and `S3_ADDRESS` pair instead.
    pub s3_enabled: bool,
    pub shutdown_grace_secs: u64,
    /// One-time enrollment secret; consumed by the node on its next start and
    /// cleared once the realm lists this node. Never leaves the backend.
    pub onboarding_secret: Option<String>,
    /// The seed's base URL, taken from the enrollment link. The node reads its
    /// own copy out of the secret; the desktop keeps it to poll the realm.
    pub seed_url: Option<String>,
    /// The realm the enrollment link named, for display and the realm poll.
    pub realm: Option<String>,
    /// The realm origin the owner connected to. It is what the portal talks to
    /// while no node of this device's own is running.
    pub realm_url: Option<String>,
    /// The API base that realm resolved to; a deployment may serve portal and
    /// API on separate listeners, so it cannot be derived from the origin.
    pub realm_api_url: Option<String>,
    /// The aruna version that realm last reported. A mismatch stays known when
    /// the realm cannot be reached, so an offline start does not run it.
    pub realm_version: Option<String>,
    /// Set once the realm has listed this node.
    pub enrolled: bool,
    /// Counts wipes, realm changes and cached-version updates, in memory only.
    /// A start or a probe that began under another count is out of date.
    #[serde(skip)]
    pub generation: u64,
    /// Escape hatch for node keys the desktop does not model yet.
    pub extra_env: BTreeMap<String, String>,
}

impl Default for NodeSettings {
    fn default() -> Self {
        Self {
            binary_path: None,
            storage_path: None,
            compute: ComputeSettings::default(),
            paused: false,
            device_label: None,
            portal_dir: None,
            ports: NodePorts::default(),
            s3_enabled: true,
            shutdown_grace_secs: 20,
            onboarding_secret: None,
            seed_url: None,
            realm: None,
            realm_url: None,
            realm_api_url: None,
            realm_version: None,
            enrolled: false,
            generation: 0,
            extra_env: BTreeMap::new(),
        }
    }
}

impl NodeSettings {
    pub fn load(path: &Path) -> Self {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::warn!(%error, "Unreadable settings file; using defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), DesktopError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), DesktopError> {
        let mut ports = [
            self.ports.portal,
            self.ports.api,
            self.ports.ops,
            self.ports.s3,
            self.ports.p2p,
        ];
        if ports.contains(&0) {
            return Err(DesktopError::Settings("a port may not be zero".into()));
        }
        ports.sort_unstable();
        if ports.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(DesktopError::Settings("the ports must differ".into()));
        }
        if self.shutdown_grace_secs < MIN_GRACE_SECS {
            return Err(DesktopError::Settings(format!(
                "the node rejects a shutdown grace below {MIN_GRACE_SECS} seconds"
            )));
        }
        self.compute.validate()
    }

    /// An enrolled device (or one mid-enrollment) runs its node with the app.
    pub fn starts_on_launch(&self) -> bool {
        self.joins_realm() && !self.paused
    }

    /// What the portal's local panel shows. The storage path is the effective
    /// one, so an unset default is still a path the owner can see and change.
    pub fn local(&self, data_dir: &Path) -> LocalSettings {
        LocalSettings {
            storage_path: self.storage_dir(data_dir).display().to_string(),
            compute: self.compute,
            s3_enabled: self.s3_enabled,
            paused: self.paused,
        }
    }

    /// Applies a patch from the portal. Absent fields keep their stored value,
    /// and every path has to be absolute: the node runs with its own working
    /// directory, so a relative one would point somewhere else entirely.
    pub fn apply(&mut self, patch: SettingsPatch) -> Result<(), DesktopError> {
        if let Some(raw) = patch.storage_path {
            self.storage_path = match raw.trim().is_empty() {
                true => None,
                false => Some(absolute("the storage location", &raw)?),
            };
        }
        if let Some(compute) = patch.compute {
            self.compute = compute;
        }
        if let Some(enabled) = patch.s3_enabled {
            self.s3_enabled = enabled;
        }
        if let Some(paused) = patch.paused {
            self.paused = paused;
        }
        Ok(())
    }

    /// Forgets this device: the enrollment and where its data was. The machine
    /// configuration stays, so the app is usable afterwards. Synced folders
    /// live in the node and go with `/device/wipe`.
    pub fn wipe(&mut self) {
        self.invalidate();
        self.onboarding_secret = None;
        self.seed_url = None;
        self.realm = None;
        self.realm_version = None;
        self.device_label = None;
        self.enrolled = false;
        self.paused = false;
        self.storage_path = None;
    }

    /// Whether this device belongs to a realm or is joining one.
    pub fn joins_realm(&self) -> bool {
        self.enrolled || self.onboarding_secret.is_some()
    }

    /// Remembers the realm the portal window opens, where its API answers and
    /// the aruna version it reported.
    pub fn remember_realm(&mut self, url: &str, api: &str, version: Option<&str>) {
        let before = (self.realm_url.clone(), self.realm_version.clone());
        if self.realm_url.as_deref() != Some(url) {
            self.realm_version = None;
        }
        if let Some(version) = version {
            self.realm_version = Some(version.to_string());
        }
        self.realm_url = Some(url.to_string());
        if before != (self.realm_url.clone(), self.realm_version.clone()) {
            self.invalidate();
        }
        self.realm_api_url = Some(api.to_string());
    }

    /// Makes every start and probe that began before this call out of date.
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Stores a version a probe of `realm_url` reported. `generation` is what
    /// the probe began under: answers `None` and stores nothing when that, or
    /// the realm, has changed since, otherwise whether the version changed.
    pub fn learn_version(
        &mut self,
        realm_url: &str,
        generation: u64,
        version: &str,
    ) -> Option<bool> {
        if self.realm_url.as_deref() != Some(realm_url) || self.generation != generation {
            return None;
        }
        let changed = self.realm_version.as_deref() != Some(version);
        if changed {
            self.realm_version = Some(version.to_string());
            self.invalidate();
        }
        Some(changed)
    }

    /// Arms the next start with an accepted invitation. The seed doubles as the
    /// realm to open when none is remembered yet, so a device enrolled straight
    /// from a deep link still has a portal to log in to.
    pub fn enroll(&mut self, secret: String, seed: Option<String>, realm: Option<String>) {
        if self.realm_url.is_none()
            && let Some(seed) = seed.as_deref()
            && let Ok(origin) = crate::realm::normalize(seed)
        {
            self.realm_url = Some(origin);
            self.invalidate();
        }
        self.onboarding_secret = Some(secret);
        self.seed_url = seed;
        self.realm = realm;
        self.enrolled = false;
    }

    /// Spends the secret once the realm lists this node, so no later start can
    /// replay it.
    pub fn joined(&mut self, realm: Option<String>) {
        self.onboarding_secret = None;
        self.enrolled = true;
        if realm.is_some() {
            self.realm = realm;
        }
    }

    pub fn storage_dir(&self, data_dir: &Path) -> PathBuf {
        self.storage_path
            .clone()
            .unwrap_or_else(|| data_dir.join("storage"))
    }

    pub fn portal_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.ports.portal)
    }

    /// Every origin the node has to answer: the app window in each of its
    /// forms, and the node's own portal listener.
    pub fn allowed_origins(&self) -> String {
        let mut origins = APP_ORIGINS.map(str::to_string).to_vec();
        origins.push(self.portal_url());
        origins.join(",")
    }

    pub fn api_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.ports.api)
    }

    /// The node's whole configuration surface, in a stable order. The compute
    /// plan comes from the machine probe, so it is passed in rather than stored.
    pub fn env_pairs(&self, data_dir: &Path, compute: &ComputePlan) -> Vec<(String, String)> {
        let mut pairs = vec![
            (
                "STORAGE_PATH".to_string(),
                self.storage_dir(data_dir).display().to_string(),
            ),
            ("SOCKET_ADDRESS".to_string(), loopback(self.ports.api)),
            ("OPS_SOCKET_ADDRESS".to_string(), loopback(self.ports.ops)),
            // Iroh needs a routable bind, so this one is deliberately not loopback.
            (
                "P2P_SOCKET_ADDRESS".to_string(),
                format!("0.0.0.0:{}", self.ports.p2p),
            ),
            ("API_PUBLIC_URL".to_string(), self.api_url()),
            ("CORS_ALLOWED_ORIGINS".to_string(), self.allowed_origins()),
            (
                "ARUNA_SHUTDOWN_GRACE_SECS".to_string(),
                self.shutdown_grace_secs.to_string(),
            ),
        ];

        match &self.portal_dir {
            Some(dir) => {
                pairs.push(("PORTAL_MODE".to_string(), "artifact".to_string()));
                pairs.push(("PORTAL_DIR".to_string(), dir.display().to_string()));
                pairs.push((
                    "PORTAL_SOCKET_ADDRESS".to_string(),
                    loopback(self.ports.portal),
                ));
            }
            None => pairs.push(("PORTAL_MODE".to_string(), "disabled".to_string())),
        }

        if self.s3_enabled {
            pairs.push(("S3_ADDRESS".to_string(), loopback(self.ports.s3)));
            pairs.push(("S3_HOST".to_string(), loopback(self.ports.s3)));
            pairs.push((
                "S3_PUBLIC_URL".to_string(),
                format!("http://127.0.0.1:{}", self.ports.s3),
            ));
        } else {
            // An empty value is how the node is told to serve no S3 listener,
            // and it has to be written to override an inherited one.
            pairs.push(("S3_ADDRESS".to_string(), String::new()));
            pairs.push(("S3_HOST".to_string(), String::new()));
        }

        if let Some(secret) = self.onboarding_secret.as_ref().filter(|s| !s.is_empty()) {
            pairs.push(("ONBOARDING_SECRET".to_string(), secret.clone()));
        }
        pairs.append(&mut self.compute_pairs(data_dir, compute));

        for (key, value) in &self.extra_env {
            pairs.push((key.clone(), value.clone()));
        }
        pairs
    }

    /// What the node needs to run the owner's own jobs and nothing else. All of
    /// it is omitted while no executor was resolved, which leaves compute off.
    fn compute_pairs(&self, data_dir: &Path, compute: &ComputePlan) -> Vec<(String, String)> {
        let Some(executor) = compute.executor else {
            // Written rather than omitted: the node must be told compute is off
            // even when the environment the app was launched from says otherwise.
            return vec![("ARUNA_COMPUTE_EXECUTOR".to_string(), "off".to_string())];
        };
        let mut pairs = vec![
            (
                "ARUNA_COMPUTE_EXECUTOR".to_string(),
                executor.as_str().to_string(),
            ),
            // The node stays up when the backend is missing, and it runs only
            // what the owner submits here: never realm work, never a listener.
            ("ARUNA_COMPUTE_OPTIONAL".to_string(), "1".to_string()),
            ("ARUNA_COMPUTE_LOCAL_ONLY".to_string(), "1".to_string()),
        ];
        for (key, value) in [
            (
                "ARUNA_COMPUTE_MAX_CONCURRENT",
                self.compute.max_concurrent.map(u64::from),
            ),
            (
                "ARUNA_COMPUTE_MAX_CPU_CORES",
                self.compute.max_cpu_cores.map(u64::from),
            ),
            ("ARUNA_COMPUTE_MAX_RAM_BYTES", self.compute.max_ram_bytes),
            ("ARUNA_COMPUTE_MAX_DISK_BYTES", self.compute.max_disk_bytes),
        ] {
            if let Some(value) = value {
                pairs.push((key.to_string(), value.to_string()));
            }
        }
        if self.compute.keep_failed {
            pairs.push(("ARUNA_COMPUTE_KEEP_FAILED".to_string(), "1".to_string()));
        }
        if let Some(host) = &compute.docker_host {
            pairs.push(("DOCKER_HOST".to_string(), host.clone()));
        }
        if let Some(root) = &compute.cgroup_root {
            pairs.push((
                "ARUNA_COMPUTE_APPTAINER_CGROUP_ROOT".to_string(),
                root.clone(),
            ));
        }

        let state_root = compute_dir(data_dir);
        pairs.push((
            "ARUNA_COMPUTE_STATE_ROOT".to_string(),
            state_root.display().to_string(),
        ));
        if executor == Executor::Apptainer {
            pairs.push((
                "ARUNA_COMPUTE_APPTAINER_STATE_ROOT".to_string(),
                state_root.join("apptainer").display().to_string(),
            ));
            pairs.push((
                "ARUNA_COMPUTE_APPTAINER_SIF_CACHE".to_string(),
                state_root.join("sif").display().to_string(),
            ));
        }
        pairs
    }

    /// The node loads its config through dotenvy and aborts without this file,
    /// so it is written on every start next to the node's working directory.
    pub fn write_env(
        &self,
        data_dir: &Path,
        compute: &ComputePlan,
    ) -> Result<PathBuf, DesktopError> {
        fs::create_dir_all(data_dir)?;
        fs::create_dir_all(self.storage_dir(data_dir))?;
        if compute.executor.is_some() {
            fs::create_dir_all(compute_dir(data_dir))?;
        }
        let body: String = self
            .env_pairs(data_dir, compute)
            .into_iter()
            .map(|(key, value)| env_line(&key, &value))
            .collect();
        let path = data_dir.join(".env");
        fs::write(&path, body)?;
        Ok(path)
    }
}

fn absolute(label: &str, raw: &str) -> Result<PathBuf, DesktopError> {
    let path = PathBuf::from(raw.trim());
    match path.is_absolute() {
        true => Ok(path),
        false => Err(DesktopError::Settings(format!(
            "{label} must be an absolute path"
        ))),
    }
}

/// Job workspaces, images and executor state, all under the node's own data.
fn compute_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("compute")
}

fn loopback(port: u16) -> String {
    format!("127.0.0.1:{port}")
}

/// dotenvy reads double-quoted values with backslash escapes, which keeps paths
/// containing spaces intact.
fn env_line(key: &str, value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("{key}=\"{escaped}\"\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(settings: &NodeSettings) -> BTreeMap<String, String> {
        planned(settings, &ComputePlan::default())
    }

    fn planned(settings: &NodeSettings, plan: &ComputePlan) -> BTreeMap<String, String> {
        settings
            .env_pairs(Path::new("/data"), plan)
            .into_iter()
            .collect()
    }

    #[test]
    fn maps_default_ports() {
        let map = pairs(&NodeSettings::default());
        assert_eq!(map["SOCKET_ADDRESS"], "127.0.0.1:34116");
        assert_eq!(map["OPS_SOCKET_ADDRESS"], "127.0.0.1:34117");
        assert_eq!(map["API_PUBLIC_URL"], "http://127.0.0.1:34116");
        assert_eq!(
            map["CORS_ALLOWED_ORIGINS"],
            "tauri://localhost,http://tauri.localhost,http://localhost:1420,http://127.0.0.1:34115"
        );
        assert_eq!(map["STORAGE_PATH"], "/data/storage");
        assert_eq!(map["PORTAL_MODE"], "disabled");
        assert_eq!(map["S3_ADDRESS"], "127.0.0.1:34118");
        assert_eq!(map["S3_HOST"], "127.0.0.1:34118");
        assert_eq!(map["S3_PUBLIC_URL"], "http://127.0.0.1:34118");
        assert!(!map.contains_key("ONBOARDING_SECRET"));
    }

    #[test]
    fn disables_s3_explicitly() {
        // Both keys must be written empty: half a pair aborts the node, and an
        // omitted key would leave an inherited one in place.
        let map = pairs(&NodeSettings {
            s3_enabled: false,
            ..NodeSettings::default()
        });
        assert_eq!(map["S3_ADDRESS"], "");
        assert_eq!(map["S3_HOST"], "");
        assert!(!map.contains_key("S3_PUBLIC_URL"));
    }

    #[test]
    fn remembers_seed_realm() {
        // A device enrolled from a deep link has no realm address of its own.
        let mut settings = NodeSettings::default();
        settings.enroll("s1".into(), Some("https://seed.example/aruna".into()), None);
        assert_eq!(settings.realm_url.as_deref(), Some("https://seed.example"));

        let mut connected = NodeSettings::default();
        connected.remember_realm(
            "https://aruna.example",
            "https://aruna.example/api/v1",
            None,
        );
        connected.enroll("s1".into(), Some("https://seed.example".into()), None);
        assert_eq!(
            connected.realm_url.as_deref(),
            Some("https://aruna.example")
        );
    }

    #[test]
    fn spends_secret_once() {
        let mut settings = NodeSettings::default();
        settings.enroll("s1".into(), Some("https://seed.example".into()), None);
        assert!(!settings.enrolled);
        settings.joined(Some("r-1".into()));
        assert!(settings.onboarding_secret.is_none());
        assert!(settings.enrolled);
        assert_eq!(settings.realm.as_deref(), Some("r-1"));
        assert!(!pairs(&settings).contains_key("ONBOARDING_SECRET"));
    }

    #[test]
    fn enables_portal_listener() {
        let settings = NodeSettings {
            portal_dir: Some(PathBuf::from("/opt/portal")),
            ..NodeSettings::default()
        };
        let map = pairs(&settings);
        assert_eq!(map["PORTAL_MODE"], "artifact");
        assert_eq!(map["PORTAL_DIR"], "/opt/portal");
        assert_eq!(map["PORTAL_SOCKET_ADDRESS"], "127.0.0.1:34115");
    }

    #[test]
    fn carries_enrollment_secret() {
        let settings = NodeSettings {
            onboarding_secret: Some("secret-value".into()),
            ..NodeSettings::default()
        };
        assert_eq!(pairs(&settings)["ONBOARDING_SECRET"], "secret-value");
    }

    #[test]
    fn rejects_duplicate_ports() {
        let mut settings = NodeSettings::default();
        settings.ports.ops = settings.ports.api;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn rejects_short_grace() {
        let settings = NodeSettings {
            shutdown_grace_secs: MIN_GRACE_SECS - 1,
            ..NodeSettings::default()
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn shows_effective_paths() {
        // The panel edits what the node uses, so an unset path is not blank.
        let view = NodeSettings::default().local(Path::new("/data"));
        assert_eq!(view.storage_path, "/data/storage");
    }

    #[test]
    fn ignores_dropped_fields() {
        // Old autoStart and offered directory fields are ignored.
        let stored: NodeSettings = serde_json::from_str(
            r#"{"offeredDirectories":["/home/o/data"],"autoStart":true,"paused":true}"#,
        )
        .expect("an older settings file");
        assert!(stored.paused);
    }

    #[test]
    fn applies_partial_edits() {
        let mut settings = NodeSettings::default();
        settings
            .apply(SettingsPatch {
                paused: Some(true),
                ..SettingsPatch::default()
            })
            .expect("a usable patch");
        assert!(settings.paused);
        assert!(settings.s3_enabled);
        assert!(settings.storage_path.is_none());
    }

    #[test]
    fn starts_enrolled_node() {
        let mut settings = NodeSettings::default();
        assert!(!settings.starts_on_launch());

        settings.enrolled = true;
        assert!(settings.starts_on_launch());

        // An armed secret covers the interval before enrollment completes.
        settings.enrolled = false;
        settings.onboarding_secret = Some("secret".into());
        assert!(settings.starts_on_launch());

        settings.onboarding_secret = None;
        settings.enrolled = true;
        settings.paused = true;
        assert!(!settings.starts_on_launch());
    }

    #[test]
    fn toggles_local_s3() {
        // The panel's toggle is the field the env pair is written from.
        let mut settings = NodeSettings::default();
        assert!(settings.local(Path::new("/data")).s3_enabled);

        settings
            .apply(SettingsPatch {
                s3_enabled: Some(false),
                ..SettingsPatch::default()
            })
            .expect("a usable patch");
        assert!(!settings.local(Path::new("/data")).s3_enabled);
        assert_eq!(pairs(&settings)["S3_HOST"], "");
    }

    #[test]
    fn turns_compute_off() {
        // No executor resolved is stated, not left out: the node has to be told
        // even when the launching environment claims otherwise.
        let map = pairs(&NodeSettings::default());
        assert_eq!(map["ARUNA_COMPUTE_EXECUTOR"], "off");
        assert_eq!(
            map.keys()
                .filter(|key| key.starts_with("ARUNA_COMPUTE_"))
                .count(),
            1
        );
    }

    #[test]
    fn writes_compute_limits() {
        let settings = NodeSettings {
            compute: ComputeSettings {
                backend: ComputeBackend::Docker,
                max_concurrent: Some(2),
                max_cpu_cores: Some(4),
                max_ram_bytes: Some(8_589_934_592),
                keep_failed: true,
                ..ComputeSettings::default()
            },
            ..NodeSettings::default()
        };
        let plan = ComputePlan {
            executor: Some(Executor::Docker),
            ..ComputePlan::default()
        };
        let map = planned(&settings, &plan);
        assert_eq!(map["ARUNA_COMPUTE_EXECUTOR"], "docker");
        assert_eq!(map["ARUNA_COMPUTE_OPTIONAL"], "1");
        assert_eq!(map["ARUNA_COMPUTE_LOCAL_ONLY"], "1");
        assert_eq!(map["ARUNA_COMPUTE_MAX_CONCURRENT"], "2");
        assert_eq!(map["ARUNA_COMPUTE_MAX_CPU_CORES"], "4");
        assert_eq!(map["ARUNA_COMPUTE_MAX_RAM_BYTES"], "8589934592");
        assert_eq!(map["ARUNA_COMPUTE_KEEP_FAILED"], "1");
        assert_eq!(map["ARUNA_COMPUTE_STATE_ROOT"], "/data/compute");
        // An unset ceiling is no ceiling, so its key stays out of the file.
        assert!(!map.contains_key("ARUNA_COMPUTE_MAX_DISK_BYTES"));
        assert!(!map.contains_key("ARUNA_COMPUTE_APPTAINER_CGROUP_ROOT"));

        let unbounded = NodeSettings {
            compute: ComputeSettings {
                max_concurrent: None,
                ..settings.compute
            },
            ..settings
        };
        assert!(!planned(&unbounded, &plan).contains_key("ARUNA_COMPUTE_MAX_CONCURRENT"));
    }

    #[test]
    fn passes_docker_host() {
        let plan = ComputePlan {
            executor: Some(Executor::Docker),
            docker_host: Some("unix:///Users/me/.colima/default/docker.sock".into()),
            ..ComputePlan::default()
        };
        let map = planned(&NodeSettings::default(), &plan);
        assert_eq!(
            map["DOCKER_HOST"],
            "unix:///Users/me/.colima/default/docker.sock"
        );
        assert!(!pairs(&NodeSettings::default()).contains_key("DOCKER_HOST"));
    }

    #[test]
    fn roots_apptainer_state() {
        let settings = NodeSettings {
            compute: ComputeSettings {
                backend: ComputeBackend::Apptainer,
                ..ComputeSettings::default()
            },
            ..NodeSettings::default()
        };
        let plan = ComputePlan {
            executor: Some(Executor::Apptainer),
            cgroup_root: Some("/sys/fs/cgroup/user.slice/user@1000.service".into()),
            ..ComputePlan::default()
        };
        let keys: Vec<String> = settings
            .env_pairs(Path::new("/data"), &plan)
            .into_iter()
            .map(|(key, _)| key)
            .filter(|key| key.starts_with("ARUNA_COMPUTE_"))
            .collect();
        assert_eq!(
            keys,
            [
                "ARUNA_COMPUTE_EXECUTOR",
                "ARUNA_COMPUTE_OPTIONAL",
                "ARUNA_COMPUTE_LOCAL_ONLY",
                "ARUNA_COMPUTE_MAX_CONCURRENT",
                "ARUNA_COMPUTE_APPTAINER_CGROUP_ROOT",
                "ARUNA_COMPUTE_STATE_ROOT",
                "ARUNA_COMPUTE_APPTAINER_STATE_ROOT",
                "ARUNA_COMPUTE_APPTAINER_SIF_CACHE",
            ]
        );
        let map = planned(&settings, &plan);
        assert_eq!(
            map["ARUNA_COMPUTE_APPTAINER_CGROUP_ROOT"],
            "/sys/fs/cgroup/user.slice/user@1000.service"
        );
        assert_eq!(
            map["ARUNA_COMPUTE_APPTAINER_STATE_ROOT"],
            "/data/compute/apptainer"
        );
        assert_eq!(
            map["ARUNA_COMPUTE_APPTAINER_SIF_CACHE"],
            "/data/compute/sif"
        );
    }

    #[test]
    fn replaces_compute_settings() {
        // The portal sends the whole object: an omitted ceiling is "no limit",
        // which a per-field merge could not tell from "unchanged".
        let mut settings = NodeSettings::default();
        settings
            .apply(SettingsPatch {
                compute: Some(
                    serde_json::from_value(serde_json::json!({
                        "backend": "docker",
                        "maxConcurrent": 2,
                        "maxRamBytes": 8_589_934_592u64,
                    }))
                    .expect("the portal's shape"),
                ),
                ..SettingsPatch::default()
            })
            .expect("a usable patch");
        assert_eq!(settings.compute.backend, ComputeBackend::Docker);
        assert_eq!(settings.compute.max_concurrent, Some(2));
        assert_eq!(settings.compute.max_ram_bytes, Some(8_589_934_592));
        assert!(settings.compute.max_cpu_cores.is_none());
        assert!(!settings.compute.keep_failed);
    }

    #[test]
    fn rejects_empty_limits() {
        let mut settings = NodeSettings::default();
        settings.compute.max_concurrent = Some(0);
        assert!(settings.validate().is_err());

        settings.compute = ComputeSettings {
            max_cpu_cores: Some(0),
            ..ComputeSettings::default()
        };
        assert!(settings.validate().is_err());

        settings.compute.max_cpu_cores = Some(4);
        settings.validate().expect("a usable ceiling");
    }

    #[test]
    fn rejects_relative_paths() {
        // The node runs in its own working directory, so a relative path there
        // would name a different place than the one the owner picked.
        let mut settings = NodeSettings::default();
        assert!(
            settings
                .apply(SettingsPatch {
                    storage_path: Some("node/storage".into()),
                    ..SettingsPatch::default()
                })
                .is_err()
        );
        settings
            .apply(SettingsPatch {
                storage_path: Some("  ".into()),
                ..SettingsPatch::default()
            })
            .expect("an empty path clears the override");
        assert!(settings.storage_path.is_none());
    }

    #[test]
    fn wipes_device_state() {
        // The machine configuration survives; the identity and its data do not.
        let mut settings = NodeSettings {
            binary_path: Some(PathBuf::from("/opt/aruna")),
            storage_path: Some(PathBuf::from("/data/elsewhere")),
            paused: true,
            ..NodeSettings::default()
        };
        settings.remember_realm(
            "https://aruna.example",
            "https://aruna.example/api/v1",
            None,
        );
        settings.enroll("s1".into(), Some("https://seed.example".into()), None);
        settings.joined(Some("r-1".into()));
        settings.realm_version = Some("3.0.0-alpha.1".into());

        settings.wipe();
        assert!(settings.realm_version.is_none());
        assert!(!settings.enrolled);
        assert!(settings.realm.is_none());
        assert!(settings.seed_url.is_none());
        assert!(settings.onboarding_secret.is_none());
        assert!(settings.storage_path.is_none());
        assert!(!settings.paused);
        assert_eq!(settings.realm_url.as_deref(), Some("https://aruna.example"));
        assert_eq!(settings.binary_path, Some(PathBuf::from("/opt/aruna")));
    }

    #[test]
    fn forgets_version_of_other_realm() {
        let mut settings = NodeSettings::default();
        settings.remember_realm(
            "https://aruna.example",
            "https://aruna.example/api/v1",
            None,
        );
        settings.realm_version = Some("3.0.0-alpha.1".into());

        // Asking again about the same realm keeps what it last said.
        settings.remember_realm(
            "https://aruna.example",
            "https://aruna.example/api/v1",
            None,
        );
        assert_eq!(settings.realm_version.as_deref(), Some("3.0.0-alpha.1"));

        settings.remember_realm(
            "https://other.example",
            "https://other.example/api/v1",
            None,
        );
        assert!(settings.realm_version.is_none());
    }

    #[test]
    fn keeps_validated_version() {
        // A realm that was reached once is remembered with its version, so a
        // later offline join still knows what it would join.
        let mut settings = NodeSettings::default();
        settings.remember_realm(
            "https://aruna.example",
            "https://aruna.example/api/v1",
            Some("3.0.0-alpha.1"),
        );
        assert_eq!(settings.realm_version.as_deref(), Some("3.0.0-alpha.1"));

        // A check that reports no version keeps the one on record.
        settings.remember_realm(
            "https://aruna.example",
            "https://aruna.example/api/v1",
            None,
        );
        assert_eq!(settings.realm_version.as_deref(), Some("3.0.0-alpha.1"));
    }

    #[test]
    fn drops_stale_probe() {
        // A probe that finishes after the device moved to another realm must
        // not write its answer into that realm's settings.
        let mut settings = NodeSettings::default();
        settings.remember_realm("https://b.example", "https://b.example/api/v1", None);

        assert_eq!(
            settings.learn_version("https://a.example", settings.generation, "3.0.0-alpha.1"),
            None
        );
        assert!(settings.realm_version.is_none());

        assert_eq!(
            settings.learn_version("https://b.example", settings.generation, "3.0.0-alpha.2"),
            Some(true)
        );
        assert_eq!(
            settings.learn_version("https://b.example", settings.generation, "3.0.0-alpha.2"),
            Some(false)
        );
        assert_eq!(settings.realm_version.as_deref(), Some("3.0.0-alpha.2"));
    }

    #[test]
    fn drops_overlapping_probe() {
        // A start probe that saw the old version must not overwrite what a
        // probe started after it already cached.
        let mut settings = NodeSettings::default();
        settings.remember_realm(
            "https://aruna.example",
            "https://aruna.example/api/v1",
            Some("3.0.0-alpha.131"),
        );
        let begun = settings.generation;

        assert_eq!(
            settings.learn_version("https://aruna.example", begun, "3.0.0-alpha.132"),
            Some(true)
        );
        assert_eq!(
            settings.learn_version("https://aruna.example", begun, "3.0.0-alpha.131"),
            None
        );
        assert_eq!(settings.realm_version.as_deref(), Some("3.0.0-alpha.132"));
    }

    #[test]
    fn counts_only_changes() {
        let mut settings = NodeSettings::default();
        let url = "https://aruna.example";
        settings.remember_realm(url, "https://aruna.example/api/v1", Some("3.0.0-alpha.1"));
        let known = settings.generation;

        // Hearing the same thing again moves nothing.
        settings.remember_realm(url, "https://aruna.example/api/v1", Some("3.0.0-alpha.1"));
        assert_eq!(
            settings.learn_version(url, known, "3.0.0-alpha.1"),
            Some(false)
        );
        assert_eq!(settings.generation, known);

        settings.wipe();
        assert!(settings.generation > known);
    }

    #[test]
    fn quotes_env_values() {
        assert_eq!(env_line("A", "with space"), "A=\"with space\"\n");
        assert_eq!(env_line("A", "a\"b\\c"), "A=\"a\\\"b\\\\c\"\n");
    }
}
