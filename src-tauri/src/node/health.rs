use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

/// The ops listener answers fast on loopback; a slow answer means trouble.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The realm info may come from a seed across the internet, so it gets more
/// room than a loopback probe.
const REALM_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub ok: bool,
    pub status: Option<u16>,
    pub body: Option<Value>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    pub health: Probe,
    pub ready: Probe,
}

/// Polls the node's ops endpoints. Both probes are plain loopback HTTP, so this
/// never leaves the machine.
pub async fn probe(client: &reqwest::Client, ops_port: u16) -> HealthReport {
    HealthReport {
        health: probe_path(client, ops_port, "healthz").await,
        ready: probe_path(client, ops_port, "readyz").await,
    }
}

/// Reads the realm a node serves. The route answers without a token, but it
/// only fills its node list for a caller holding a realm token, so the realm id
/// is the one field an anonymous desktop can rely on.
pub async fn realm_id(client: &reqwest::Client, base: &str) -> Option<String> {
    let url = format!("{}/api/v1/info/realm", base.trim_end_matches('/'));
    let response = client.get(&url).timeout(REALM_TIMEOUT).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.json::<Value>().await.ok()?;
    body.get("realm_id")?.as_str().map(str::to_string)
}

async fn probe_path(client: &reqwest::Client, port: u16, path: &str) -> Probe {
    let url = format!("http://127.0.0.1:{port}/{path}");
    match client.get(&url).timeout(PROBE_TIMEOUT).send().await {
        Ok(response) => {
            let status = response.status();
            let body = response.json::<Value>().await.ok();
            Probe {
                ok: status.is_success(),
                status: Some(status.as_u16()),
                body,
                error: None,
            }
        }
        Err(error) => Probe {
            ok: false,
            status: None,
            body: None,
            error: Some(error.to_string()),
        },
    }
}
