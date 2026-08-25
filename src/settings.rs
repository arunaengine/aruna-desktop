use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::DesktopError;

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

/// The local control surface the portal renders: where the node stores data
/// and whether it runs. Everything else the desktop keeps is machine
/// configuration the owner never has to see.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalSettings {
    pub storage_path: String,
    pub paused: bool,
    pub auto_start: bool,
}

/// A partial edit of [`LocalSettings`]; the portal may send any subset.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsPatch {
    pub storage_path: Option<String>,
    pub paused: Option<bool>,
    pub auto_start: Option<bool>,
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
    /// Keeps the node stopped until the owner resumes it, across launches.
    pub paused: bool,
    /// The device name the owner typed when enrolling. Display only: the realm
    /// names the device itself, and the node takes no label from the desktop.
    pub device_label: Option<String>,
    /// Portal artifact directory; the portal listener stays off while unset.
    pub portal_dir: Option<PathBuf>,
    pub ports: NodePorts,
    /// Off by default: a device profile serves no S3 listener, which the node
    /// is told through an empty `S3_HOST` and `S3_ADDRESS` pair.
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
    /// Set once the realm has listed this node.
    pub enrolled: bool,
    pub auto_start: bool,
    /// Escape hatch for node keys the desktop does not model yet.
    pub extra_env: BTreeMap<String, String>,
}

impl Default for NodeSettings {
    fn default() -> Self {
        Self {
            binary_path: None,
            storage_path: None,
            paused: false,
            device_label: None,
            portal_dir: None,
            ports: NodePorts::default(),
            s3_enabled: false,
            shutdown_grace_secs: 20,
            onboarding_secret: None,
            seed_url: None,
            realm: None,
            realm_url: None,
            realm_api_url: None,
            enrolled: false,
            auto_start: false,
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
        Ok(())
    }

    /// What the portal's local panel shows. The storage path is the effective
    /// one, so an unset default is still a path the owner can see and change.
    pub fn local(&self, data_dir: &Path) -> LocalSettings {
        LocalSettings {
            storage_path: self.storage_dir(data_dir).display().to_string(),
            paused: self.paused,
            auto_start: self.auto_start,
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
        if let Some(paused) = patch.paused {
            self.paused = paused;
        }
        if let Some(auto_start) = patch.auto_start {
            self.auto_start = auto_start;
        }
        Ok(())
    }

    /// Forgets this device: the enrollment and where its data was. The machine
    /// configuration stays, so the app is usable afterwards. Synced folders
    /// live in the node and go with `/device/wipe`.
    pub fn wipe(&mut self) {
        self.onboarding_secret = None;
        self.seed_url = None;
        self.realm = None;
        self.device_label = None;
        self.enrolled = false;
        self.paused = false;
        self.storage_path = None;
    }

    /// Remembers the realm the portal window opens and where its API answers.
    pub fn remember_realm(&mut self, url: &str, api: &str) {
        self.realm_url = Some(url.to_string());
        self.realm_api_url = Some(api.to_string());
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

    /// The node's whole configuration surface, in a stable order.
    pub fn env_pairs(&self, data_dir: &Path) -> Vec<(String, String)> {
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

        for (key, value) in &self.extra_env {
            pairs.push((key.clone(), value.clone()));
        }
        pairs
    }

    /// The node loads its config through dotenvy and aborts without this file,
    /// so it is written on every start next to the node's working directory.
    pub fn write_env(&self, data_dir: &Path) -> Result<PathBuf, DesktopError> {
        fs::create_dir_all(data_dir)?;
        fs::create_dir_all(self.storage_dir(data_dir))?;
        let body: String = self
            .env_pairs(data_dir)
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
        settings.env_pairs(Path::new("/data")).into_iter().collect()
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
        assert!(!map.contains_key("ONBOARDING_SECRET"));
    }

    #[test]
    fn disables_s3_explicitly() {
        // Both keys must be written empty: half a pair aborts the node, and an
        // omitted key would leave an inherited one in place.
        let map = pairs(&NodeSettings::default());
        assert_eq!(map["S3_ADDRESS"], "");
        assert_eq!(map["S3_HOST"], "");
        assert!(!map.contains_key("S3_PUBLIC_URL"));

        let enabled = NodeSettings {
            s3_enabled: true,
            ..NodeSettings::default()
        };
        assert_eq!(pairs(&enabled)["S3_HOST"], "127.0.0.1:34118");
    }

    #[test]
    fn remembers_seed_realm() {
        // A device enrolled from a deep link has no realm address of its own.
        let mut settings = NodeSettings::default();
        settings.enroll("s1".into(), Some("https://seed.example/aruna".into()), None);
        assert_eq!(settings.realm_url.as_deref(), Some("https://seed.example"));

        let mut connected = NodeSettings::default();
        connected.remember_realm("https://aruna.example", "https://aruna.example/api/v1");
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
        // A settings file written before synced folders moved into the node
        // still loads: offered directories are simply not read any more.
        let stored: NodeSettings = serde_json::from_str(
            r#"{"offeredDirectories":["/home/o/data"],"autoStart":true,"paused":true}"#,
        )
        .expect("an older settings file");
        assert!(stored.auto_start);
        assert!(stored.paused);
    }

    #[test]
    fn applies_partial_edits() {
        let mut settings = NodeSettings {
            auto_start: true,
            ..NodeSettings::default()
        };
        settings
            .apply(SettingsPatch {
                paused: Some(true),
                ..SettingsPatch::default()
            })
            .expect("a usable patch");
        assert!(settings.paused);
        assert!(settings.auto_start);
        assert!(settings.storage_path.is_none());
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
        settings.remember_realm("https://aruna.example", "https://aruna.example/api/v1");
        settings.enroll("s1".into(), Some("https://seed.example".into()), None);
        settings.joined(Some("r-1".into()));

        settings.wipe();
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
    fn quotes_env_values() {
        assert_eq!(env_line("A", "with space"), "A=\"with space\"\n");
        assert_eq!(env_line("A", "a\"b\\c"), "A=\"a\\\"b\\\\c\"\n");
    }
}
