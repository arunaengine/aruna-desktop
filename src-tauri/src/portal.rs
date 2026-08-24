use serde::{Deserialize, Serialize};

use crate::error::DesktopError;
use crate::settings::NodeSettings;

/// Which portal the portal window shows.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PortalTarget {
    /// The realm's own portal, remembered from the welcome landing.
    Remote,
    /// The embedded node's portal listener, for working without a network.
    Local,
}

/// The portal a window is about to open.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PortalPlan {
    pub target: PortalTarget,
    pub url: String,
}

/// The portals the app could open right now, so a view can label its button and
/// an unreachable target can offer the other one.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PortalOptions {
    pub remote: Option<String>,
    pub local: Option<String>,
    /// What an unqualified open would pick, absent when neither is usable.
    pub chosen: Option<PortalTarget>,
}

/// The local portal is only real once the node belongs to a realm, is ready and
/// serves a portal artifact of its own.
fn local_url(settings: &NodeSettings, ready: bool) -> Option<String> {
    (settings.enrolled && ready && settings.portal_dir.is_some()).then(|| settings.portal_url())
}

pub fn options(settings: &NodeSettings, ready: bool) -> PortalOptions {
    PortalOptions {
        remote: settings.realm_url.clone(),
        local: local_url(settings, ready),
        chosen: plan(settings, ready, None).ok().map(|plan| plan.target),
    }
}

/// Picks the portal to open. The remembered realm is the default; the local
/// portal needs the owner to ask for it, either in the settings or after the
/// realm turned out to be unreachable.
pub fn plan(
    settings: &NodeSettings,
    ready: bool,
    wanted: Option<PortalTarget>,
) -> Result<PortalPlan, DesktopError> {
    let local = local_url(settings, ready);
    let remote = settings.realm_url.clone();

    let target = match wanted {
        Some(target) => target,
        None if settings.prefer_local_portal && local.is_some() => PortalTarget::Local,
        None => PortalTarget::Remote,
    };

    let url = match target {
        PortalTarget::Remote => remote.ok_or_else(|| {
            DesktopError::Settings("no realm is connected yet; connect to one first".into())
        })?,
        PortalTarget::Local => local.ok_or_else(|| {
            DesktopError::Settings(
                "the local portal needs an enrolled node that is running with a portal directory"
                    .into(),
            )
        })?,
    };
    Ok(PortalPlan { target, url })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn device() -> NodeSettings {
        let mut settings = NodeSettings {
            portal_dir: Some(PathBuf::from("/opt/portal")),
            ..NodeSettings::default()
        };
        settings.remember_realm("https://aruna.example");
        settings.joined(Some("r-1".into()));
        settings
    }

    #[test]
    fn opens_remembered_realm() {
        let plan = plan(&device(), true, None).expect("a remembered realm");
        assert_eq!(plan.target, PortalTarget::Remote);
        assert_eq!(plan.url, "https://aruna.example");
    }

    #[test]
    fn honours_offline_choice() {
        let settings = NodeSettings {
            prefer_local_portal: true,
            ..device()
        };
        let plan = plan(&settings, true, None).expect("a local portal");
        assert_eq!(plan.target, PortalTarget::Local);
        assert_eq!(plan.url, "http://127.0.0.1:34115");
    }

    #[test]
    fn keeps_realm_unready() {
        // The offline choice must not strand the owner while the node is down.
        let settings = NodeSettings {
            prefer_local_portal: true,
            ..device()
        };
        assert_eq!(
            plan(&settings, false, None).expect("the realm").target,
            PortalTarget::Remote
        );
    }

    #[test]
    fn refuses_absent_local() {
        // Asking for the local portal explicitly still needs one to exist.
        let mut settings = device();
        assert!(plan(&settings, false, Some(PortalTarget::Local)).is_err());
        settings.portal_dir = None;
        assert!(plan(&settings, true, Some(PortalTarget::Local)).is_err());
        settings.portal_dir = Some(PathBuf::from("/opt/portal"));
        settings.enrolled = false;
        assert!(plan(&settings, true, Some(PortalTarget::Local)).is_err());
    }

    #[test]
    fn refuses_unknown_realm() {
        let settings = NodeSettings::default();
        assert!(plan(&settings, false, None).is_err());
        assert!(plan(&settings, false, Some(PortalTarget::Remote)).is_err());
    }

    #[test]
    fn offers_both_portals() {
        let found = options(&device(), true);
        assert_eq!(found.remote.as_deref(), Some("https://aruna.example"));
        assert_eq!(found.local.as_deref(), Some("http://127.0.0.1:34115"));
        assert_eq!(found.chosen, Some(PortalTarget::Remote));

        let bare = options(&NodeSettings::default(), false);
        assert_eq!(bare, PortalOptions::default());
    }
}
