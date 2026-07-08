// Integration with the companion rustdesk-api backend for RustDesk 1.4.9 Pro
// features:
//   - device deployment gating (NOT_DEPLOYED)
//   - controller-user audit attribution (conn_audit_ref snapshots)
//   - HTTP-over-rendezvous proxy (use-raw-tcp-for-api)
//
// Configuration (environment variables):
//   RUSTDESK_API_SERVER  base URL of rustdesk-api, e.g. http://127.0.0.1:21114
//   HBBS_API_TOKEN       shared secret matching the api's `hbbs.token`
//   DEPLOY_ENABLED       Y/1/TRUE to enable the device deployment gate
//
// All calls fail open: when the integration is unconfigured or the api is
// unreachable, hbbs behaves as before (no gating, no attribution).

use hbb_common::{
    log,
    rendezvous_proto::{HeaderEntry, HttpProxyRequest, HttpProxyResponse},
    tokio,
    uuid::Uuid,
};
use once_cell::sync::Lazy;
use std::env;

static API_SERVER: Lazy<String> = Lazy::new(|| {
    env::var("RUSTDESK_API_SERVER")
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string()
});
static HBBS_TOKEN: Lazy<String> = Lazy::new(|| env::var("HBBS_API_TOKEN").unwrap_or_default());
static DEPLOY_ENABLED: Lazy<bool> = Lazy::new(|| {
    matches!(
        env::var("DEPLOY_ENABLED")
            .unwrap_or_default()
            .to_uppercase()
            .as_str(),
        "Y" | "1" | "TRUE"
    )
});

static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default()
});

/// Whether the device-deployment gate is enabled.
pub fn deploy_enabled() -> bool {
    *DEPLOY_ENABLED
}

/// Whether the hbbs<->api integration is usable (api server + token configured).
pub fn enabled() -> bool {
    !API_SERVER.is_empty() && !HBBS_TOKEN.is_empty()
}

fn auth_header() -> String {
    format!("Bearer {}", *HBBS_TOKEN)
}

/// Query whether a device id has been provisioned via `rustdesk --deploy`.
/// Fails open (returns true) when the integration is disabled or the api errors,
/// so a transient api outage never blocks registration.
pub async fn device_deployed(id: &str) -> bool {
    if !enabled() {
        return true;
    }
    let url = format!("{}/api/hbbs/device-deployed", *API_SERVER);
    let res = CLIENT
        .get(&url)
        .header("Authorization", auth_header())
        .query(&[("id", id)])
        .send()
        .await;
    match res {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            // api envelope: { code, message, data: { deployed } }
            Ok(v) => v
                .get("data")
                .and_then(|d| d.get("deployed"))
                .and_then(|b| b.as_bool())
                .unwrap_or(true),
            Err(err) => {
                log::warn!("device_deployed decode error: {}", err);
                true
            }
        },
        Err(err) => {
            log::warn!("device_deployed request error: {}", err);
            true
        }
    }
}

/// Mint an unguessable conn_audit_ref for a controlling connection and record
/// the ref -> controller-user snapshot with the api (fire-and-forget). Returns
/// None when the integration is disabled or no controller token is present.
pub fn mint_conn_audit_ref(token: &str) -> Option<String> {
    if !enabled() || token.is_empty() {
        return None;
    }
    let reff = Uuid::new_v4().to_string();
    let token = token.to_string();
    let ref_clone = reff.clone();
    tokio::spawn(async move {
        let url = format!("{}/api/hbbs/conn-audit-ref", *API_SERVER);
        let body = serde_json::json!({ "ref": ref_clone, "token": token });
        if let Err(err) = CLIENT
            .post(&url)
            .header("Authorization", auth_header())
            .json(&body)
            .send()
            .await
        {
            log::warn!("conn-audit-ref record error: {}", err);
        }
    });
    Some(reff)
}

/// Forward an HttpProxyRequest to the fixed api server and return the response.
/// The client supplies only the path, never the host, so this cannot be used to
/// reach arbitrary hosts (SSRF is bounded to the configured api server).
pub async fn http_proxy(req: HttpProxyRequest) -> HttpProxyResponse {
    let mut out = HttpProxyResponse::new();
    if !enabled() {
        out.error = "hbbs api integration is not configured".to_owned();
        return out;
    }
    let method = match reqwest::Method::from_bytes(req.method.to_uppercase().as_bytes()) {
        Ok(m) => m,
        Err(_) => {
            out.error = format!("invalid method: {}", req.method);
            return out;
        }
    };
    let url = format!("{}{}", *API_SERVER, req.path);
    let mut builder = CLIENT.request(method, &url);
    for h in req.headers.iter() {
        builder = builder.header(&h.name, &h.value);
    }
    if !req.body.is_empty() {
        builder = builder.body(req.body.to_vec());
    }
    match builder.send().await {
        Ok(resp) => {
            out.status = resp.status().as_u16() as i32;
            let mut headers = Vec::new();
            for (k, v) in resp.headers().iter() {
                if let Ok(val) = v.to_str() {
                    headers.push(HeaderEntry {
                        name: k.as_str().to_owned(),
                        value: val.to_owned(),
                        ..Default::default()
                    });
                }
            }
            out.headers = headers;
            match resp.bytes().await {
                Ok(b) => out.body = b.to_vec().into(),
                Err(err) => out.error = format!("read body error: {}", err),
            }
        }
        Err(err) => {
            out.error = format!("proxy request error: {}", err);
        }
    }
    out
}
