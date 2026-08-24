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

/// Everything the desktop needs to launch and reach its node. Serialized as the
/// node's env file, so every field maps to a documented aruna config key.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct NodeSettings {
    /// Overrides both the dev env var and the bundled sidecar when set.
    pub binary_path: Option<PathBuf>,
    /// Defaults to `<app data>/node/storage`.
    pub storage_path: Option<PathBuf>,
    /// Portal artifact directory; the portal listener stays off while unset.
    pub portal_dir: Option<PathBuf>,
    pub ports: NodePorts,
    /// Stock aruna still requires `S3_HOST`/`S3_ADDRESS`, so a device profile
    /// without the S3 listener needs the optional-listener work in aruna.
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
    /// Set once the realm has listed this node, so the app opens on status.
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
            portal_dir: None,
            ports: NodePorts::default(),
            s3_enabled: false,
            shutdown_grace_secs: 20,
            onboarding_secret: None,
            seed_url: None,
            realm: None,
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

    /// What the control window is allowed to see. The one-time secret never
    /// crosses into the webview, so the enroll view cannot echo or leak it.
    pub fn redacted(&self) -> Self {
        Self {
            onboarding_secret: None,
            ..self.clone()
        }
    }

    /// Takes an edit from the settings view. The view never carries the fields
    /// the enrollment owns, so they are kept from the stored copy.
    pub fn merge_edits(&self, edited: Self) -> Self {
        Self {
            onboarding_secret: self.onboarding_secret.clone(),
            seed_url: self.seed_url.clone(),
            realm: self.realm.clone(),
            enrolled: self.enrolled,
            ..edited
        }
    }

    /// Arms the next start with an accepted invitation.
    pub fn enroll(&mut self, secret: String, seed: String, realm: Option<String>) {
        self.onboarding_secret = Some(secret);
        self.seed_url = Some(seed);
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
            ("CORS_ALLOWED_ORIGINS".to_string(), self.portal_url()),
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
        assert_eq!(map["CORS_ALLOWED_ORIGINS"], "http://127.0.0.1:34115");
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
    fn hides_the_secret() {
        let mut settings = NodeSettings::default();
        settings.enroll("s1".into(), "https://seed.example".into(), Some("r".into()));
        let shown = settings.redacted();
        assert!(shown.onboarding_secret.is_none());
        assert_eq!(shown.seed_url.as_deref(), Some("https://seed.example"));
        assert_eq!(settings.onboarding_secret.as_deref(), Some("s1"));
    }

    #[test]
    fn keeps_enrollment_fields() {
        // The settings view never sees the secret, so saving must not drop it.
        let mut stored = NodeSettings::default();
        stored.enroll("s1".into(), "https://seed.example".into(), Some("r".into()));

        let edited = NodeSettings {
            auto_start: true,
            ..stored.redacted()
        };
        let merged = stored.merge_edits(edited);
        assert!(merged.auto_start);
        assert_eq!(merged.onboarding_secret.as_deref(), Some("s1"));
        assert_eq!(merged.realm.as_deref(), Some("r"));
        assert_eq!(merged.seed_url.as_deref(), Some("https://seed.example"));
    }

    #[test]
    fn spends_secret_once() {
        let mut settings = NodeSettings::default();
        settings.enroll("s1".into(), "https://seed.example".into(), None);
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
    fn quotes_env_values() {
        assert_eq!(env_line("A", "with space"), "A=\"with space\"\n");
        assert_eq!(env_line("A", "a\"b\\c"), "A=\"a\\\"b\\\\c\"\n");
    }
}
