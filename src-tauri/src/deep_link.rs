use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use url::Url;

use crate::state::AppState;
use crate::windows;

/// Emitted when an `aruna://enroll` link has been stashed for the enroll view.
pub const ENROLL_EVENT: &str = "enroll-payload";

/// The scheme registered for this app, matching `tauri.conf.json`.
pub const SCHEME: &str = "aruna";

/// What a portal enrollment link carries. The secret is one-time and is never
/// logged; it only travels to the enroll view and into the node's env file.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollPayload {
    pub secret: Option<String>,
    pub realm: Option<String>,
    pub name: Option<String>,
    pub endpoints: Vec<String>,
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
    let payload = parse_enroll(url);
    let state = app.state::<AppState>();
    *state.pending.lock().expect("pending enrollment poisoned") = Some(payload.clone());
    if let Err(error) = app.emit(ENROLL_EVENT, payload) {
        tracing::warn!(%error, "Could not emit the enrollment payload");
    }
    windows::focus_main(app);
}

fn parse_enroll(url: &Url) -> EnrollPayload {
    let mut payload = EnrollPayload::default();
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "secret" | "code" => payload.secret = Some(value.into_owned()),
            "realm" => payload.realm = Some(value.into_owned()),
            "name" => payload.name = Some(value.into_owned()),
            "endpoint" => payload.endpoints.push(value.into_owned()),
            "endpoints" => payload
                .endpoints
                .extend(value.split(',').map(|item| item.trim().to_string())),
            _ => {}
        }
    }
    payload.endpoints.retain(|item| !item.is_empty());
    payload
}

/// The OIDC redirect belongs to the portal origin, so the callback is handed to
/// the portal window rather than handled here.
fn forward_auth(app: &AppHandle, url: &Url) {
    let base = app.state::<AppState>().settings().portal_url();
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

    #[test]
    fn routes_both_forms() {
        assert_eq!(parse("aruna://enroll?secret=x").0, "enroll");
        assert_eq!(parse("aruna:enroll?secret=x").0, "enroll");
        assert_eq!(parse("aruna://auth/callback?code=x"), ("auth".into(), "callback".into()));
        assert_eq!(parse("aruna:auth/callback?code=x"), ("auth".into(), "callback".into()));
    }

    #[test]
    fn reads_enroll_query() {
        let url = Url::parse("aruna://enroll?secret=s1&realm=r&name=laptop&endpoint=a&endpoint=b")
            .expect("valid url");
        let payload = parse_enroll(&url);
        assert_eq!(payload.secret.as_deref(), Some("s1"));
        assert_eq!(payload.realm.as_deref(), Some("r"));
        assert_eq!(payload.name.as_deref(), Some("laptop"));
        assert_eq!(payload.endpoints, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn splits_endpoint_list() {
        let url = Url::parse("aruna://enroll?endpoints=a,%20b,").expect("valid url");
        assert_eq!(
            parse_enroll(&url).endpoints,
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
