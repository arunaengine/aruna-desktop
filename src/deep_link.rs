use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use url::Url;

use crate::bridge;
use crate::error::DesktopError;
use crate::state::AppState;
use crate::windows;

/// Emitted once an `aruna://enroll` link has been acted on, so the portal can
/// show what happened without ever seeing the secret. It is a hint, not the
/// source of truth: a cold start may deliver it before the portal listens, and
/// `node_status` reports the same enrollment either way.
pub const ENROLL_EVENT: &str = "enroll-invite";

/// The scheme registered for this app, matching `tauri.conf.json`.
pub const SCHEME: &str = "aruna";

/// Where each deep link lands the portal's router.
const DEVICE_ROUTE: &str = "/app/device";
const CALLBACK_ROUTE: &str = "/auth/callback";

/// A parsed enrollment invitation. The secret is one-time and never logged,
/// never emitted to the webview, and never returned by a command; it leaves
/// this process only as the node's onboarding environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnrollLink {
    pub secret: String,
    /// The seed node's public base URL, without a trailing slash. Absent from a
    /// pasted bare code: the node reads its own copy out of the secret, and the
    /// desktop keeps this one only to watch the join.
    pub seed: Option<String>,
    pub realm: Option<String>,
}

/// What the portal is told about an invitation the app followed. The secret is
/// never part of it: it leaves the backend only as the node's environment.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnrollInvite {
    pub seed: Option<String>,
    pub realm: Option<String>,
    /// False when the enrollment could not be armed; `error` says why.
    pub applied: bool,
    pub error: Option<String>,
}

/// Keeps the last invitation the app acted on. A link followed into a cold
/// start emits its event before the portal is listening, so the same payload
/// stays here for the portal to ask for once it is.
#[derive(Debug, Default)]
pub struct LastInvite(std::sync::Mutex<Option<EnrollInvite>>);

impl LastInvite {
    pub fn remember(&self, invite: EnrollInvite) {
        *self.0.lock().expect("last invite poisoned") = Some(invite);
    }

    pub fn last(&self) -> Option<EnrollInvite> {
        self.0.lock().expect("last invite poisoned").clone()
    }

    /// A wiped device has no invitation behind it any more.
    pub fn clear(&self) {
        *self.0.lock().expect("last invite poisoned") = None;
    }
}

impl EnrollLink {
    /// Takes an enrollment the portal handed over. Only the secret is required;
    /// a seed URL beside it has to be an absolute web address.
    pub fn accept(
        secret: String,
        seed: Option<String>,
        realm: Option<String>,
    ) -> Result<Self, DesktopError> {
        let secret = non_empty(&secret)
            .ok_or_else(|| DesktopError::DeepLink("the secret is empty".into()))?;
        let seed = seed
            .as_deref()
            .and_then(non_empty)
            .map(|seed| seed_base(&seed))
            .transpose()?;
        Ok(Self {
            secret,
            seed,
            realm: realm.as_deref().and_then(non_empty),
        })
    }
}

/// Routes one deep link. Unknown links are dropped with a warning that names
/// only the route, never the query.
pub fn handle(app: &AppHandle, raw: &str) {
    let Ok(url) = Url::parse(raw) else {
        tracing::warn!("Ignoring an unparsable deep link");
        return;
    };
    if url.scheme() != SCHEME {
        tracing::warn!(scheme = url.scheme(), "Ignoring a foreign scheme");
        return;
    }

    let (head, tail) = route(&url);
    match (head.as_str(), tail.as_str()) {
        ("enroll", _) => apply_enroll(app, &url),
        ("auth", "callback") => forward_auth(app, &url),
        _ => tracing::warn!(route = %head, "Ignoring an unknown deep link route"),
    }
}

/// Accepts both `aruna://enroll?...` and the authority-less `aruna:enroll?...`.
fn route(url: &Url) -> (String, String) {
    let path = url.path().trim_matches('/').to_string();
    match url.host_str() {
        Some(host) if !host.is_empty() => (host.to_string(), path),
        _ => {
            let mut parts = path.splitn(2, '/');
            (
                parts.next().unwrap_or_default().to_string(),
                parts.next().unwrap_or_default().to_string(),
            )
        }
    }
}

/// Arms the enrollment the link carries and tells the portal how it went. The
/// owner already accepted it by following the link out of the portal, so there
/// is nothing left to confirm here.
fn apply_enroll(app: &AppHandle, url: &Url) {
    let link = match parse_enroll(url) {
        Ok(link) => link,
        Err(error) => {
            // The message never repeats the query, so the secret cannot leak.
            tracing::warn!(%error, "Ignoring a malformed enrollment link");
            return;
        }
    };
    let seed = link.seed.clone();
    let realm = link.realm.clone();
    let app = app.clone();
    // The device page shows the enrollment whether or not the event lands, so
    // a link followed into a cold start still arrives somewhere useful.
    if let Err(error) = windows::navigate_main(&app, DEVICE_ROUTE) {
        tracing::warn!(%error, "Could not open the device page");
    }

    tauri::async_runtime::spawn(async move {
        let joined = bridge::arm(&app, link, None).await;
        let invite = EnrollInvite {
            seed,
            realm: joined
                .as_ref()
                .ok()
                .and_then(|joined| joined.realm.clone())
                .or(realm),
            applied: joined.is_ok(),
            error: joined.err().map(|error| error.to_string()),
        };
        app.state::<AppState>().invite.remember(invite.clone());
        if let Err(error) = app.emit(ENROLL_EVENT, invite) {
            tracing::warn!(%error, "Could not emit the enrollment outcome");
        }
    });
}

/// Reads `aruna://enroll?secret=…&seed=…&realm=…`. The secret and the seed are
/// required; the realm is a label the seed can also report.
pub fn parse_enroll(url: &Url) -> Result<EnrollLink, DesktopError> {
    let mut secret = None;
    let mut seed = None;
    let mut realm = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "secret" => secret = non_empty(&value),
            "seed" => seed = non_empty(&value),
            "realm" => realm = non_empty(&value),
            _ => {}
        }
    }

    let secret = secret.ok_or_else(|| DesktopError::DeepLink("the secret is missing".into()))?;
    let seed = seed.ok_or_else(|| DesktopError::DeepLink("the seed URL is missing".into()))?;
    Ok(EnrollLink {
        secret,
        seed: Some(seed_base(&seed)?),
        realm,
    })
}

/// The seed must be an absolute HTTP(S) origin; a trailing slash is dropped so
/// the polled paths can be appended directly.
fn seed_base(raw: &str) -> Result<String, DesktopError> {
    let url = Url::parse(raw)
        .map_err(|error| DesktopError::DeepLink(format!("unreadable seed URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(DesktopError::DeepLink(
            "the seed URL must be http or https".into(),
        ));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(DesktopError::DeepLink("the seed URL has no host".into()));
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The portal owns the OIDC exchange, so the callback is handed to its router:
/// the app window is sent to the callback route with everything the provider
/// returned, and the page that already runs there finishes the sign-in.
fn forward_auth(app: &AppHandle, url: &Url) {
    if let Err(error) = windows::navigate_main(app, &callback_route(url)) {
        tracing::warn!(%error, "Could not forward the auth callback");
    }
}

/// The portal route an `aruna://auth/callback` link becomes. Both the query and
/// the fragment are kept: a provider may answer in either.
fn callback_route(url: &Url) -> String {
    let mut route = String::from(CALLBACK_ROUTE);
    if let Some(query) = url.query() {
        route.push('?');
        route.push_str(query);
    }
    if let Some(fragment) = url.fragment() {
        route.push('#');
        route.push_str(fragment);
    }
    route
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> (String, String) {
        route(&Url::parse(raw).expect("valid url"))
    }

    fn enroll(raw: &str) -> Result<EnrollLink, DesktopError> {
        parse_enroll(&Url::parse(raw).expect("valid url"))
    }

    #[test]
    fn routes_both_forms() {
        assert_eq!(parse("aruna://enroll?secret=x").0, "enroll");
        assert_eq!(parse("aruna:enroll?secret=x").0, "enroll");
        assert_eq!(
            parse("aruna://auth/callback?code=x"),
            ("auth".into(), "callback".into())
        );
        assert_eq!(
            parse("aruna:auth/callback?code=x"),
            ("auth".into(), "callback".into())
        );
    }

    #[test]
    fn reads_enroll_query() {
        let link = enroll("aruna://enroll?secret=s1&seed=https%3A%2F%2Fseed.example%2F&realm=r-1")
            .expect("a complete link");
        assert_eq!(link.secret, "s1");
        assert_eq!(link.seed.as_deref(), Some("https://seed.example"));
        assert_eq!(link.realm.as_deref(), Some("r-1"));
    }

    #[test]
    fn keeps_seed_path() {
        // A seed behind a path prefix keeps it; only the trailing slash goes.
        let link = enroll("aruna://enroll?secret=s&seed=https%3A%2F%2Fh.example%2Faruna%2F")
            .expect("a complete link");
        assert_eq!(link.seed.as_deref(), Some("https://h.example/aruna"));
    }

    #[test]
    fn rejects_missing_fields() {
        assert!(enroll("aruna://enroll?seed=https%3A%2F%2Fs.example").is_err());
        assert!(enroll("aruna://enroll?secret=s1").is_err());
        assert!(enroll("aruna://enroll?secret=%20&seed=https%3A%2F%2Fs.example").is_err());
    }

    #[test]
    fn keeps_the_callback_answer() {
        // A provider answers in the query or in the fragment; both have to
        // reach the portal's router untouched.
        let route = |raw: &str| callback_route(&Url::parse(raw).expect("valid url"));
        assert_eq!(
            route("aruna://auth/callback?code=abc&state=xyz"),
            "/auth/callback?code=abc&state=xyz"
        );
        assert_eq!(
            route("aruna://auth/callback#access_token=abc"),
            "/auth/callback#access_token=abc"
        );
        assert_eq!(route("aruna://auth/callback"), "/auth/callback");
    }

    #[test]
    fn keeps_the_last_invite() {
        // A portal that subscribed too late asks for the payload it missed.
        let kept = LastInvite::default();
        assert!(kept.last().is_none());

        let invite = EnrollInvite {
            seed: Some("https://seed.example".into()),
            realm: Some("r-1".into()),
            applied: true,
            error: None,
        };
        kept.remember(invite.clone());
        assert_eq!(kept.last(), Some(invite));

        let refused = EnrollInvite {
            seed: None,
            realm: None,
            applied: false,
            error: Some("the seed refused the secret".into()),
        };
        kept.remember(refused.clone());
        assert_eq!(kept.last(), Some(refused));

        kept.clear();
        assert!(kept.last().is_none());
    }

    #[test]
    fn hides_the_secret() {
        // The invitation the portal is told about carries no secret at all.
        let invite = EnrollInvite {
            seed: Some("https://seed.example".into()),
            realm: Some("r-1".into()),
            applied: true,
            error: None,
        };
        let json = serde_json::to_string(&invite).expect("a serializable invite");
        assert!(!json.contains("secret"));
        assert!(json.contains("\"applied\":true"));
    }

    #[test]
    fn rejects_foreign_seed() {
        assert!(enroll("aruna://enroll?secret=s&seed=file%3A%2F%2F%2Fetc%2Fpasswd").is_err());
        assert!(enroll("aruna://enroll?secret=s&seed=seed.example").is_err());
    }
}
