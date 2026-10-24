use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use url::Url;

use crate::error::DesktopError;

/// A realm portal is reached over the internet, so it gets more room than the
/// loopback probes.
const REACH_TIMEOUT: Duration = Duration::from_secs(10);

/// The runtime configuration the portal listener serves at its own root.
const CONFIG_PATH: &str = "/portal-config.json";

/// What a realm address turned out to be. `portal` is false when only the API
/// answered: the address is an aruna node, but it serves no portal itself.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RealmTarget {
    pub origin: String,
    /// The API base that actually answered; a deployment may serve the portal
    /// and the API on separate listeners, so it is not derivable from `origin`.
    pub api_base_url: String,
    pub realm: Option<String>,
    pub api_version: Option<String>,
    pub portal: bool,
    /// The address the owner typed, once the shell followed it to one of the
    /// management nodes that address named.
    pub redirected_from: Option<String>,
}

/// Why a realm address cannot be used, in terms the owner can act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reach {
    Refused,
    Dns,
    Tls(String),
    Timeout,
    Transport(String),
    Status(u16),
    NotAruna,
}

impl Reach {
    /// True while the failure is about the connection itself, which no other
    /// path on the same origin can get past.
    pub fn transport(&self) -> bool {
        matches!(
            self,
            Self::Refused | Self::Dns | Self::Tls(_) | Self::Timeout | Self::Transport(_)
        )
    }

    pub fn message(&self, origin: &str) -> String {
        match self {
            Self::Refused => format!("{origin} refused the connection; nothing is listening there"),
            Self::Dns => format!("{origin} could not be resolved; check the host name"),
            Self::Tls(detail) => format!("the TLS connection to {origin} failed: {detail}"),
            Self::Timeout => format!("{origin} did not answer in time"),
            Self::Transport(detail) => format!("{origin} could not be reached: {detail}"),
            Self::Status(code) => format!("{origin} answered with HTTP {code}"),
            Self::NotAruna => format!("{origin} answered, but it is not an Aruna portal"),
        }
    }
}

/// Turns what an owner types into the origin the portal window opens.
///
/// A bare hostname becomes `https://`, an explicit `http://` is kept, and a
/// pasted portal URL is reduced to its origin: the portal, its configuration
/// and the API prefix all live at the root of that origin.
pub fn normalize(input: &str) -> Result<String, DesktopError> {
    let text = input.trim();
    if text.is_empty() {
        return Err(DesktopError::Address("the realm address is empty".into()));
    }

    let lower = text.to_ascii_lowercase();
    let absolute = lower.starts_with("http://") || lower.starts_with("https://");
    if !absolute && text.contains("://") {
        return Err(DesktopError::Address(
            "the realm address must be http or https".into(),
        ));
    }

    let raw = match absolute {
        true => text.to_string(),
        false => format!("https://{text}"),
    };
    let url = Url::parse(&raw)
        .map_err(|error| DesktopError::Address(format!("unreadable realm address: {error}")))?;
    if url.host_str().is_none_or(str::is_empty) {
        return Err(DesktopError::Address(
            "the realm address has no host".into(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(DesktopError::Address(
            "the realm address must carry no credentials".into(),
        ));
    }
    Ok(url.origin().ascii_serialization())
}

/// Asks an origin whether it serves a realm, then follows any other node kind
/// to a management node: only that kind enrolls a device or signs it in.
pub async fn check(client: &reqwest::Client, origin: &str) -> Result<RealmTarget, Reach> {
    let target = reach(client, origin).await?;
    Ok(follow(client, target).await)
}

/// Every management node the realm published, from a node's `/system/realm`;
/// `None` when the node is one itself or names none.
fn management_urls(realm: &Value) -> Option<Vec<String>> {
    if realm.get("is_management_node").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let urls: Vec<String> = realm
        .get("management_urls")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    (!urls.is_empty()).then_some(urls)
}

/// The first management node that answers replaces the target; a realm that
/// names none reachable keeps the node the owner typed, and the portal explains.
async fn follow(client: &reqwest::Client, target: RealmTarget) -> RealmTarget {
    let realm = format!("{}/system/realm", target.api_base_url);
    let Some(urls) = get_json(client, &realm)
        .await
        .ok()
        .as_ref()
        .and_then(management_urls)
    else {
        return target;
    };
    for url in urls {
        let Ok(origin) = normalize(&url) else {
            continue;
        };
        if origin == target.origin {
            continue;
        }
        let found = match reach(client, &origin).await {
            Ok(found) => found,
            Err(_) => match reach_api(client, &url).await {
                Ok(found) => found,
                Err(_) => continue,
            },
        };
        return RealmTarget {
            redirected_from: Some(target.origin),
            ..found
        };
    }
    target
}

/// A published api base that does not sit at the root of its origin.
async fn reach_api(client: &reqwest::Client, url: &str) -> Result<RealmTarget, Reach> {
    let api = url.trim_end_matches('/');
    let origin = normalize(api).map_err(|_| Reach::NotAruna)?;
    let info = get_json(client, &format!("{api}/system/info")).await?;
    target(&origin, api, &info, false).ok_or(Reach::NotAruna)
}

/// Asks an origin whether it serves a realm: the portal's runtime config first,
/// then the two places a node's `/system/info` route can sit.
async fn reach(client: &reqwest::Client, origin: &str) -> Result<RealmTarget, Reach> {
    let config = get_json(client, &format!("{origin}{CONFIG_PATH}")).await;
    if let Err(reach) = &config
        && reach.transport()
    {
        return Err(reach.clone());
    }

    let portal = config.is_ok();
    let mut urls = Vec::new();
    if let Ok(value) = &config
        && let Some(base) = value
            .get("apiBaseUrl")
            .and_then(Value::as_str)
            .and_then(|raw| api_base(origin, raw))
    {
        urls.push(format!("{base}/system/info"));
    }
    urls.push(format!("{origin}/api/v1/system/info"));
    urls.push(format!("{origin}/system/info"));

    let mut failure = config.err();
    for url in urls {
        match get_json(client, &url).await {
            Ok(info) => match target(origin, api_of(&url), &info, portal) {
                Some(target) => return Ok(target),
                None => failure = Some(Reach::NotAruna),
            },
            Err(reach) if reach.transport() => return Err(reach),
            Err(reach) => {
                failure.get_or_insert(reach);
            }
        }
    }
    Err(failure.unwrap_or(Reach::NotAruna))
}

/// The portal names its API base absolutely, but a proxied deployment may send
/// a path, which only means anything against the origin it came from.
fn api_base(origin: &str, raw: &str) -> Option<String> {
    let base = match raw.starts_with("http://") || raw.starts_with("https://") {
        true => Url::parse(raw).ok()?,
        false => Url::parse(origin).ok()?.join(raw).ok()?,
    };
    Some(base.as_str().trim_end_matches('/').to_string())
}

/// The API base a `/system/info` probe URL stands for.
fn api_of(url: &str) -> &str {
    url.trim_end_matches("/system/info").trim_end_matches('/')
}

/// A `/system/info` body without a realm id is some other service answering on the
/// same address, not an aruna node.
fn target(origin: &str, api_base: &str, info: &Value, portal: bool) -> Option<RealmTarget> {
    let realm = info.pointer("/node/realm_id").and_then(Value::as_str)?;
    Some(RealmTarget {
        origin: origin.to_string(),
        api_base_url: api_base.to_string(),
        realm: Some(realm.to_string()),
        api_version: info
            .get("api_version")
            .and_then(Value::as_str)
            .map(str::to_string),
        portal,
        redirected_from: None,
    })
}

async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value, Reach> {
    let response = client
        .get(url)
        .timeout(REACH_TIMEOUT)
        .send()
        .await
        .map_err(|error| classify(&chain(&error), error.is_timeout()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(Reach::Status(status.as_u16()));
    }
    response.json::<Value>().await.map_err(|_| Reach::NotAruna)
}

/// reqwest keeps its transport failures behind opaque sources, so the chain is
/// read as text; the wording comes from hyper, rustls and `std::io`.
pub fn chain(error: &reqwest::Error) -> Vec<String> {
    let mut parts = vec![error.to_string()];
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        parts.push(cause.to_string());
        source = cause.source();
    }
    parts
}

pub fn classify(chain: &[String], timeout: bool) -> Reach {
    if timeout {
        return Reach::Timeout;
    }
    let text = chain.join(": ").to_ascii_lowercase();
    let detail = chain.last().cloned().unwrap_or_default();
    if text.contains("connection refused") {
        Reach::Refused
    } else if text.contains("dns error")
        || text.contains("failed to lookup address")
        || text.contains("name or service not known")
    {
        Reach::Dns
    } else if text.contains("certificate") || text.contains("tls") || text.contains("handshake") {
        Reach::Tls(detail)
    } else {
        Reach::Transport(detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn origin(input: &str) -> String {
        normalize(input).expect("a usable address")
    }

    #[test]
    fn adds_default_scheme() {
        assert_eq!(origin("aruna.example"), "https://aruna.example");
        assert_eq!(origin("  ARUNA.Example/  "), "https://aruna.example");
        assert_eq!(origin("aruna.example:8443"), "https://aruna.example:8443");
    }

    #[test]
    fn keeps_explicit_scheme() {
        // A loopback or LAN realm is reached over plain http on purpose.
        assert_eq!(origin("http://127.0.0.1:34115"), "http://127.0.0.1:34115");
        assert_eq!(origin("HTTPS://Aruna.Example:443"), "https://aruna.example");
    }

    #[test]
    fn reduces_to_origin() {
        assert_eq!(
            origin("https://aruna.example/datasets/abc?q=1#top"),
            "https://aruna.example"
        );
        assert_eq!(
            origin("https://aruna.example:8443/"),
            "https://aruna.example:8443"
        );
    }

    #[test]
    fn names_management_nodes() {
        // A management node is never followed; any other kind is, to every
        // url it names, and a node that names none keeps the owner's address.
        let server = json!({
            "is_management_node": false,
            "management_urls": ["https://a.example/api/v1", "https://b.example/api/v1"]
        });
        assert_eq!(
            management_urls(&server).as_deref(),
            Some(
                &[
                    "https://a.example/api/v1".to_string(),
                    "https://b.example/api/v1".to_string()
                ][..]
            )
        );
        let management =
            json!({ "is_management_node": true, "management_urls": ["https://a.example/api/v1"] });
        assert_eq!(management_urls(&management), None);
        assert_eq!(
            management_urls(&json!({ "is_management_node": false, "management_urls": [] })),
            None
        );
        assert_eq!(management_urls(&json!({ "realm_id": "r" })), None);
    }

    #[test]
    fn rejects_unusable_addresses() {
        assert!(normalize("").is_err());
        assert!(normalize("   ").is_err());
        assert!(normalize("ftp://aruna.example").is_err());
        assert!(normalize("https://").is_err());
        assert!(normalize("https://user:pw@aruna.example").is_err());
    }

    #[test]
    fn resolves_api_base() {
        assert_eq!(
            api_base("https://aruna.example", "https://api.aruna.example/api/v1"),
            Some("https://api.aruna.example/api/v1".into())
        );
        assert_eq!(
            api_base("https://aruna.example", "/api/v1/"),
            Some("https://aruna.example/api/v1".into())
        );
    }

    #[test]
    fn reads_info_body() {
        let info = json!({
            "node": {"status": "available", "realm_id": "r-1"},
            "api_version": "3.0.0",
        });
        let api = api_of("https://api.example/api/v1/system/info");
        let found = target("https://aruna.example", api, &info, true).expect("an aruna node");
        assert_eq!(found.realm.as_deref(), Some("r-1"));
        assert_eq!(found.api_base_url, "https://api.example/api/v1");
        assert_eq!(found.api_version.as_deref(), Some("3.0.0"));
        assert!(found.portal);
        assert!(target("https://aruna.example", api, &json!({"ok": true}), true).is_none());
    }

    #[test]
    fn classifies_refusal() {
        let chain = vec![
            "error sending request".to_string(),
            "client error (Connect)".to_string(),
            "tcp connect error: Connection refused (os error 111)".to_string(),
        ];
        assert_eq!(classify(&chain, false), Reach::Refused);
    }

    #[test]
    fn classifies_bad_name() {
        let chain = vec![
            "error sending request".to_string(),
            "dns error: failed to lookup address information: Name or service not known"
                .to_string(),
        ];
        assert_eq!(classify(&chain, false), Reach::Dns);
    }

    #[test]
    fn classifies_tls_failure() {
        let chain = vec![
            "error sending request".to_string(),
            "invalid peer certificate: UnknownIssuer".to_string(),
        ];
        assert_eq!(
            classify(&chain, false),
            Reach::Tls("invalid peer certificate: UnknownIssuer".into())
        );
        assert_eq!(classify(&chain, true), Reach::Timeout);
    }

    #[test]
    fn names_origin() {
        // Every message has to say which address failed.
        for reach in [
            Reach::Refused,
            Reach::Dns,
            Reach::Timeout,
            Reach::Status(502),
            Reach::NotAruna,
        ] {
            assert!(
                reach
                    .message("https://aruna.example")
                    .contains("aruna.example")
            );
        }
        assert!(!Reach::Status(502).transport());
        assert!(Reach::Refused.transport());
    }
}
