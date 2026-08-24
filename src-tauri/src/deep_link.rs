use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use url::Url;

use crate::error::DesktopError;
use crate::state::AppState;
use crate::windows;

/// Emitted when an `aruna://enroll` link has been stashed for the enroll view.
pub const ENROLL_EVENT: &str = "enroll-invite";

/// The scheme registered for this app, matching `tauri.conf.json`.
pub const SCHEME: &str = "aruna";

/// A parsed enrollment invitation. The secret is one-time and never logged,
/// never emitted to the webview, and never returned by a command; it leaves
/// this process only as the node's onboarding environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnrollLink {
    pub secret: String,
    /// The seed node's public base URL, without a trailing slash.
    pub seed: String,
    pub realm: Option<String>,
}

/// The part of an invitation the control window may see.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnrollInvite {
    pub seed: String,
    pub realm: Option<String>,
}

impl EnrollLink {
    pub fn invite(&self) -> EnrollInvite {
        EnrollInvite {
            seed: self.seed.clone(),
            realm: self.realm.clone(),
        }
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
        ("enroll", _) => stash_enroll(app, &url),
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

fn stash_enroll(app: &AppHandle, url: &Url) {
    let link = match parse_enroll(url) {
        Ok(link) => link,
        Err(error) => {
            // The message never repeats the query, so the secret cannot leak.
            tracing::warn!(%error, "Ignoring a malformed enrollment link");
            return;
        }
    };
    let invite = link.invite();
    let state = app.state::<AppState>();
    *state.pending.lock().expect("pending enrollment poisoned") = Some(link);
    if let Err(error) = app.emit(ENROLL_EVENT, invite) {
        tracing::warn!(%error, "Could not emit the enrollment invitation");
    }
    windows::focus_main(app);
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
        seed: seed_base(&seed)?,
        realm,
    })
}

/// The paste fallback: either a whole `aruna://enroll` link, or a bare secret
/// paired with the seed URL typed beside it.
pub fn parse_paste(text: &str, seed: &str, realm: &str) -> Result<EnrollLink, DesktopError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(DesktopError::DeepLink("the secret is empty".into()));
    }
    if let Some(rest) = strip_scheme(text) {
        let url = Url::parse(rest)
            .map_err(|error| DesktopError::DeepLink(format!("unreadable link: {error}")))?;
        return parse_enroll(&url);
    }
    let seed = non_empty(seed)
        .ok_or_else(|| DesktopError::DeepLink("a bare secret needs the seed URL".into()))?;
    Ok(EnrollLink {
        secret: text.to_string(),
        seed: seed_base(&seed)?,
        realm: non_empty(realm),
    })
}

/// Keeps `Url::parse` from reading a bare secret as an opaque URL, and lets the
/// authority-less form through unchanged.
fn strip_scheme(text: &str) -> Option<&str> {
    let lower = text.to_ascii_lowercase();
    lower.starts_with("aruna:").then_some(text)
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

/// The OIDC redirect belongs to the portal origin, so the callback is handed to
/// the portal window rather than handled here. The origin comes from the window
/// itself: the login was started there, whichever portal it is showing.
fn forward_auth(app: &AppHandle, url: &Url) {
    let Some(base) = windows::portal_origin(app) else {
        tracing::warn!("Dropping an auth callback with no portal window open");
        return;
    };
    // TODO(aruna): the portal's callback path comes from the website auth seam;
    // this mirrors the loopback redirect URI registered with the provider.
    let path = match url.query() {
        Some(query) => format!("/auth/callback?{query}"),
        None => "/auth/callback".to_string(),
    };
    if let Err(error) = windows::open_portal(app, &base, Some(&path)) {
        tracing::warn!(%error, "Could not forward the auth callback");
    }
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
        assert_eq!(link.seed, "https://seed.example");
        assert_eq!(link.realm.as_deref(), Some("r-1"));
    }

    #[test]
    fn keeps_seed_path() {
        // A seed behind a path prefix keeps it; only the trailing slash goes.
        let link = enroll("aruna://enroll?secret=s&seed=https%3A%2F%2Fh.example%2Faruna%2F")
            .expect("a complete link");
        assert_eq!(link.seed, "https://h.example/aruna");
    }

    #[test]
    fn rejects_missing_fields() {
        assert!(enroll("aruna://enroll?seed=https%3A%2F%2Fs.example").is_err());
        assert!(enroll("aruna://enroll?secret=s1").is_err());
        assert!(enroll("aruna://enroll?secret=%20&seed=https%3A%2F%2Fs.example").is_err());
    }

    #[test]
    fn rejects_foreign_seed() {
        assert!(enroll("aruna://enroll?secret=s&seed=file%3A%2F%2F%2Fetc%2Fpasswd").is_err());
        assert!(enroll("aruna://enroll?secret=s&seed=seed.example").is_err());
    }

    #[test]
    fn pastes_whole_link() {
        let link = parse_paste(
            "  aruna://enroll?secret=s1&seed=https%3A%2F%2Fseed.example&realm=r-1  ",
            "",
            "",
        )
        .expect("a complete link");
        assert_eq!(link.secret, "s1");
        assert_eq!(link.realm.as_deref(), Some("r-1"));
    }

    #[test]
    fn pastes_bare_secret() {
        let link = parse_paste("s1", "https://seed.example/", "r-1").expect("a complete link");
        assert_eq!(link.secret, "s1");
        assert_eq!(link.seed, "https://seed.example");

        assert!(parse_paste("s1", "", "").is_err());
        assert!(parse_paste("  ", "https://seed.example", "").is_err());
    }
}
