//! The provider requests the portal cannot make itself: the app window's origin
//! is one no provider accepts for cross-origin calls.

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use reqwest::header::CONTENT_TYPE;
use reqwest::redirect::Policy;
use serde_json::{Map, Value};
use url::{Host, Url, form_urlencoded};

use crate::error::DesktopError;
use crate::realm::{self, Reach};

/// The same room the portal gives its own OIDC requests.
const OIDC_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVERY_PATH: &str = "/.well-known/openid-configuration";

/// A JSON object exactly as the provider answered it.
pub type Document = Map<String, Value>;

/// Reaches OIDC providers for the portal. Redirects are never followed, and a
/// token request only goes to an endpoint a discovery document of this run
/// named, so the shell is no general proxy.
pub struct OidcClient {
    client: reqwest::Client,
    endpoints: Mutex<HashSet<String>>,
}

impl OidcClient {
    pub fn new() -> Result<Self, DesktopError> {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(OIDC_TIMEOUT)
            .build()?;
        Ok(Self {
            client,
            endpoints: Mutex::default(),
        })
    }

    /// Fetches a discovery document and remembers the token endpoint it names.
    pub async fn discover(&self, raw: &str) -> Result<Document, DesktopError> {
        let url = discovery_url(raw)?;
        let response = self
            .client
            .get(url.clone())
            .send()
            .await
            .map_err(|error| unreachable(&error))?;
        let origin = origin_of(response.url());
        let status = response.status();
        if !status.is_success() {
            return Err(DesktopError::Unreachable(
                Reach::Status(status.as_u16()).message(&origin),
            ));
        }
        let document = response.json::<Document>().await.map_err(|_| {
            DesktopError::Unreachable(format!("{origin} answered with no discovery document"))
        })?;
        self.remember(&url, &document);
        Ok(document)
    }

    /// Posts a token request to a remembered endpoint. The form carries the code
    /// and the verifier and the answer carries tokens, so none of it is logged.
    pub async fn request_token(
        &self,
        endpoint: &str,
        form: &BTreeMap<String, String>,
    ) -> Result<Document, DesktopError> {
        self.allowed(endpoint)?;
        let body = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(form)
            .finish();
        let response = self
            .client
            .post(endpoint)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|error| unreachable(&error))?;
        let origin = origin_of(response.url());
        let status = response.status();
        if !status.is_success() {
            let body = response.json::<Value>().await.ok();
            return Err(refusal(status.as_u16(), body.as_ref()));
        }
        response.json::<Document>().await.map_err(|_| {
            DesktopError::Unreachable(format!("{origin} answered the token request with no JSON"))
        })
    }

    /// Keeps the token endpoint a document names, if the shell may post to it.
    /// A loopback one counts only on the discovery URL's own origin, so a remote
    /// provider cannot point the shell at a local service.
    fn remember(&self, discovery: &Url, document: &Document) {
        let Some(endpoint) = document.get("token_endpoint").and_then(Value::as_str) else {
            return;
        };
        let usable = provider_url(endpoint).is_ok_and(|token| match loopback(&token) {
            true => token.origin() == discovery.origin(),
            false => token.scheme() == "https",
        });
        if usable {
            self.endpoints
                .lock()
                .expect("token endpoints poisoned")
                .insert(endpoint.to_string());
        }
    }

    fn allowed(&self, endpoint: &str) -> Result<(), DesktopError> {
        let known = self
            .endpoints
            .lock()
            .expect("token endpoints poisoned")
            .contains(endpoint);
        match known {
            true => Ok(()),
            false => Err(DesktopError::Link(
                "the token endpoint is not one a fetched discovery document named".into(),
            )),
        }
    }
}

/// An https address, or plain http on this machine, where a dev realm's
/// provider runs.
fn provider_url(raw: &str) -> Result<Url, DesktopError> {
    let url =
        Url::parse(raw).map_err(|error| DesktopError::Link(format!("unreadable URL: {error}")))?;
    match url.scheme() {
        "https" => Ok(url),
        "http" if loopback(&url) => Ok(url),
        _ => Err(DesktopError::Link(
            "a provider is reached over https, or over http on this machine only".into(),
        )),
    }
}

/// Whether a URL names this machine under any alias: every `.localhost` name
/// (RFC 6761), also with a trailing dot, 127.0.0.0/8, `::1`, and IPv4 in IPv6.
fn loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(name)) => {
            let name = name.trim_end_matches('.');
            name == "localhost" || name.ends_with(".localhost")
        }
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback() || ip.to_ipv4().is_some_and(|ip| ip.is_loopback()),
        None => false,
    }
}

fn discovery_url(raw: &str) -> Result<Url, DesktopError> {
    let url = provider_url(raw)?;
    if !url.path().ends_with(DISCOVERY_PATH) {
        return Err(DesktopError::Link(format!(
            "a discovery address ends with {DISCOVERY_PATH}"
        )));
    }
    Ok(url)
}

/// A refused token request, with the provider's own reason when it gave one.
fn refusal(status: u16, body: Option<&Value>) -> DesktopError {
    let reason = ["error_description", "error"].into_iter().find_map(|key| {
        body?
            .get(key)?
            .as_str()
            .filter(|text| !text.trim().is_empty())
    });
    DesktopError::Provider {
        status,
        reason: reason.unwrap_or("no reason given").to_string(),
    }
}

/// Says why a provider could not be reached, in the words a realm probe uses.
fn unreachable(error: &reqwest::Error) -> DesktopError {
    let origin = error
        .url()
        .map_or_else(|| "the provider".to_string(), origin_of);
    let reach = realm::classify(&realm::chain(error), error.is_timeout());
    DesktopError::Unreachable(reach.message(&origin))
}

fn origin_of(url: &Url) -> String {
    url.origin().ascii_serialization()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn document(value: Value) -> Document {
        value.as_object().cloned().expect("a JSON object")
    }

    #[test]
    fn checks_discovery_address() {
        let ok = |raw: &str| discovery_url(raw).is_ok();
        assert!(ok(
            "https://idp.example/realms/aruna/.well-known/openid-configuration"
        ));
        assert!(ok(
            "https://idp.example:8443/.well-known/openid-configuration"
        ));
        // A dev realm's provider runs on this machine over plain http.
        assert!(ok(
            "http://127.0.0.1:8080/realms/dev/.well-known/openid-configuration"
        ));
        assert!(ok("http://localhost:8080/.well-known/openid-configuration"));
        assert!(ok("http://[::1]:8080/.well-known/openid-configuration"));

        assert!(!ok("http://idp.example/.well-known/openid-configuration"));
        assert!(!ok("http://192.168.1.10/.well-known/openid-configuration"));
        assert!(!ok("ftp://idp.example/.well-known/openid-configuration"));
        assert!(!ok(
            "https://idp.example/realms/aruna/protocol/openid-connect/token"
        ));
        assert!(!ok(
            "https://idp.example/.well-known/openid-configuration/x"
        ));
        assert!(!ok("not a url"));
    }

    #[test]
    fn allows_known_endpoints() {
        let client = OidcClient::new().expect("an HTTP client");
        let endpoint = "https://idp.example/realms/aruna/protocol/openid-connect/token";
        assert!(client.allowed(endpoint).is_err());

        let remote =
            discovery_url("https://idp.example/realms/aruna/.well-known/openid-configuration")
                .expect("a discovery address");
        client.remember(
            &remote,
            &document(json!({
                "issuer": "https://idp.example/realms/aruna",
                "token_endpoint": endpoint,
            })),
        );
        assert!(client.allowed(endpoint).is_ok());
        assert!(client.allowed("https://idp.example/other").is_err());

        // A remote document can name neither plain http nor a service on this machine.
        for token in [
            "http://192.168.1.1/token",
            "http://127.0.0.1:8080/token",
            "https://localhost/token",
            "https://[::1]:8443/token",
        ] {
            client.remember(&remote, &document(json!({ "token_endpoint": token })));
            assert!(client.allowed(token).is_err(), "{token}");
        }
    }

    #[test]
    fn keeps_local_origin() {
        // A dev provider on this machine may name a token endpoint on its own origin only.
        let client = OidcClient::new().expect("an HTTP client");
        let local =
            discovery_url("http://127.0.0.1:8080/realms/dev/.well-known/openid-configuration")
                .expect("a discovery address");
        let own = "http://127.0.0.1:8080/realms/dev/protocol/openid-connect/token";
        client.remember(&local, &document(json!({ "token_endpoint": own })));
        assert!(client.allowed(own).is_ok());

        for token in ["http://127.0.0.1:9000/token", "http://localhost:8080/token"] {
            client.remember(&local, &document(json!({ "token_endpoint": token })));
            assert!(client.allowed(token).is_err(), "{token}");
        }
        let remote = "https://idp.example/token";
        client.remember(&local, &document(json!({ "token_endpoint": remote })));
        assert!(client.allowed(remote).is_ok());
    }

    #[test]
    fn spots_loopback_aliases() {
        let local = |raw: &str| loopback(&Url::parse(raw).expect("a URL"));
        for alias in [
            "https://localhost/",
            "https://localhost./",
            "https://login.localhost/",
            "https://login.localhost./",
            "https://127.1.2.3/",
            "https://[::1]/",
            "https://[::ffff:127.0.0.1]/",
            "https://[::127.0.0.1]/",
        ] {
            assert!(local(alias), "{alias}");
        }
        for remote in [
            "https://idp.example/",
            "https://localhost.example/",
            "https://notlocalhost/",
            "https://128.0.0.1/",
            "https://[::2]/",
            "https://[::ffff:10.0.0.1]/",
        ] {
            assert!(!local(remote), "{remote}");
        }
    }

    #[test]
    fn refuses_loopback_aliases() {
        // A remote document cannot name this machine under any alias; a local one
        // still names its own origin, and only that.
        let client = OidcClient::new().expect("an HTTP client");
        let remote = discovery_url("https://idp.example/.well-known/openid-configuration")
            .expect("a discovery address");
        let other = "http://127.0.0.1:9000/token";
        for host in [
            "localhost.",
            "login.localhost",
            "127.1.2.3",
            "[::ffff:127.0.0.1]",
            "[::127.0.0.1]",
        ] {
            let token = format!("https://{host}/token");
            client.remember(&remote, &document(json!({ "token_endpoint": token })));
            assert!(client.allowed(&token).is_err(), "{token}");

            let local = discovery_url(&format!(
                "http://{host}:8080/.well-known/openid-configuration"
            ))
            .expect("a local discovery address");
            let own = format!("http://{host}:8080/token");
            client.remember(&local, &document(json!({ "token_endpoint": own })));
            assert!(client.allowed(&own).is_ok(), "{own}");
            client.remember(&local, &document(json!({ "token_endpoint": other })));
            assert!(client.allowed(other).is_err(), "{host}");
        }
    }

    #[test]
    fn keeps_provider_reason() {
        let described = refusal(
            400,
            Some(&json!({ "error": "invalid_grant", "error_description": "Code not valid" })),
        );
        assert!(matches!(
            &described,
            DesktopError::Provider { status: 400, reason } if reason == "Code not valid"
        ));
        assert_eq!(
            described.to_string(),
            "the provider refused the token request (HTTP 400): Code not valid"
        );

        let coded = refusal(401, Some(&json!({ "error": "invalid_client" })));
        assert!(coded.to_string().ends_with("(HTTP 401): invalid_client"));

        let bare = refusal(502, None);
        assert!(bare.to_string().ends_with("(HTTP 502): no reason given"));
    }
}
