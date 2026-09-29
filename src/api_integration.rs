// Integration with the companion rustdesk-api backend for RustDesk 1.4.9 Pro
// features:
//   - device deployment gating (NOT_DEPLOYED)
//   - controller-user audit attribution (conn_audit_ref snapshots)
//   - HTTP-over-rendezvous proxy (use-raw-tcp-for-api)
//
// Configuration (environment variables):
//   RUSTDESK_API_SERVER  base URL of rustdesk-api, e.g. http://127.0.0.1:21114
//   HBBS_API_TOKEN       shared secret matching the api's `hbbs.token`
//   DEPLOY_ENABLED       Y/1/TRUE to consult the api's deploy gate on RegisterPk.
//                        The api's `hbbs.deploy-enabled` decides the answer, so
//                        leaving it off there keeps every device deployed.
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
use std::{
    collections::HashMap,
    env,
    hash::Hash,
    sync::Mutex,
    time::{Duration, Instant},
};

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
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default()
});

// Deploy-gate answers are cached so a registration storm (e.g. every device
// re-registering after an hbbs restart) is not one api request per device.
// "Not deployed" expires quickly so a freshly deployed device gets in on its
// next retry (the client retries every 30s).
const DEPLOYED_TTL: Duration = Duration::from_secs(300);
const NOT_DEPLOYED_TTL: Duration = Duration::from_secs(10);
// A controller retries punch-hole/relay requests; reuse one ref per
// (controller token, target id) for this long instead of minting a new one.
const AUDIT_REF_REUSE: Duration = Duration::from_secs(60);
// Cap on conn_audit_ref records sent to the api per second, so a flood of
// forged requests cannot be amplified into api/database load.
const AUDIT_REF_MAX_PER_SEC: u32 = 50;
const CACHE_MAX_ENTRIES: usize = 10_000;

type TimedCache<K, V> = Mutex<HashMap<K, (V, Instant)>>;
// (device id, base64 uuid, base64 pk) -> deployed
static DEPLOY_CACHE: Lazy<TimedCache<(String, String, String), bool>> = Lazy::new(Default::default);
// (controller token, target peer id) -> conn_audit_ref
static AUDIT_REFS: Lazy<TimedCache<(String, String), String>> = Lazy::new(Default::default);
static AUDIT_REF_RATE: Lazy<Mutex<(Instant, u32)>> = Lazy::new(|| Mutex::new((Instant::now(), 0)));

/// Whether RegisterPk should be checked against the api's deploy gate.
pub fn deploy_gate_active() -> bool {
    *DEPLOY_ENABLED && enabled()
}

/// Whether the hbbs<->api integration is usable (api server + token configured).
pub fn enabled() -> bool {
    !API_SERVER.is_empty() && !HBBS_TOKEN.is_empty()
}

fn auth_header() -> String {
    format!("Bearer {}", *HBBS_TOKEN)
}

/// Drop expired entries once a cache grows past CACHE_MAX_ENTRIES; clear it if
/// that is not enough (only possible under a flood of distinct keys).
fn prune<K: Eq + Hash, V>(map: &mut HashMap<K, (V, Instant)>, ttl: impl Fn(&V) -> Duration) {
    if map.len() < CACHE_MAX_ENTRIES {
        return;
    }
    map.retain(|_, (v, tm)| tm.elapsed() < ttl(v));
    if map.len() >= CACHE_MAX_ENTRIES {
        map.clear();
    }
}

/// Ask the api whether the device (id + base64 uuid/pk, as the client sends
/// them to /api/devices/deploy) is provisioned via `rustdesk --deploy`.
/// Fails open (returns true) when the integration is disabled or the api
/// errors, so a transient api outage never blocks registration.
pub async fn device_deployed(id: &str, uuid: &str, pk: &str) -> bool {
    if !enabled() {
        return true;
    }
    let key = (id.to_owned(), uuid.to_owned(), pk.to_owned());
    if let Some((deployed, tm)) = DEPLOY_CACHE.lock().unwrap().get(&key) {
        let ttl = if *deployed {
            DEPLOYED_TTL
        } else {
            NOT_DEPLOYED_TTL
        };
        if tm.elapsed() < ttl {
            return *deployed;
        }
    }
    let url = format!("{}/api/hbbs/device-deployed", *API_SERVER);
    let res = CLIENT
        .get(&url)
        .header("Authorization", auth_header())
        .query(&[("id", id), ("uuid", uuid), ("pk", pk)])
        .send()
        .await;
    let deployed = match res {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            // api envelope: { code, message, data: { deployed } }
            Ok(v) => match v.pointer("/data/deployed").and_then(|b| b.as_bool()) {
                Some(deployed) => deployed,
                None => {
                    log::warn!("device_deployed: unexpected api response {}", v);
                    return true;
                }
            },
            Err(err) => {
                log::warn!("device_deployed decode error: {}", err);
                return true;
            }
        },
        Err(err) => {
            log::warn!("device_deployed request error: {}", err);
            return true;
        }
    };
    let mut cache = DEPLOY_CACHE.lock().unwrap();
    prune(&mut cache, |deployed| {
        if *deployed {
            DEPLOYED_TTL
        } else {
            NOT_DEPLOYED_TTL
        }
    });
    cache.insert(key, (deployed, Instant::now()));
    deployed
}

/// Returns false once AUDIT_REF_MAX_PER_SEC records were sent this second.
fn audit_ref_rate_ok() -> bool {
    let mut rate = AUDIT_REF_RATE.lock().unwrap();
    if rate.0.elapsed() >= Duration::from_secs(1) {
        *rate = (Instant::now(), 0);
    }
    if rate.1 >= AUDIT_REF_MAX_PER_SEC {
        return false;
    }
    rate.1 += 1;
    true
}

/// Mint an unguessable conn_audit_ref for a controlling connection to `peer_id`
/// and record the ref -> controller-user snapshot with the api (fire-and-
/// forget). Retries within AUDIT_REF_REUSE get the same ref. Returns None when
/// the integration is disabled, no usable controller token is present, or the
/// record rate limit is hit (the connection then simply has no attribution).
pub fn mint_conn_audit_ref(token: &str, peer_id: &str) -> Option<String> {
    if !enabled() || token.is_empty() {
        return None;
    }
    // Same check MUST_LOGIN uses: with the api's JWT key configured, tokens
    // that do not verify locally never reach the api.
    if !crate::jwt::SECRET.is_empty() && crate::jwt::verify_token(token).is_err() {
        return None;
    }
    let key = (token.to_owned(), peer_id.to_owned());
    let mut refs = AUDIT_REFS.lock().unwrap();
    if let Some((audit_ref, tm)) = refs.get(&key) {
        if tm.elapsed() < AUDIT_REF_REUSE {
            return Some(audit_ref.clone());
        }
    }
    if !audit_ref_rate_ok() {
        log::warn!("conn-audit-ref rate limit hit, skipping attribution");
        return None;
    }
    let audit_ref = Uuid::new_v4().to_string();
    prune(&mut refs, |_| AUDIT_REF_REUSE);
    refs.insert(key, (audit_ref.clone(), Instant::now()));
    drop(refs);
    let body = serde_json::json!({ "ref": audit_ref, "token": token });
    tokio::spawn(async move {
        let url = format!("{}/api/hbbs/conn-audit-ref", *API_SERVER);
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
    Some(audit_ref)
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
    // Build the target URL by parsing the fixed api server and overwriting only
    // the path/query components. String concatenation is unsafe here: a client
    // path like "@evil.com/x" would turn "http://api:21114" into
    // "http://api:21114@evil.com/x", moving the host to evil.com (SSRF). Using
    // set_path/set_query keeps the host locked to RUSTDESK_API_SERVER.
    let url = match reqwest::Url::parse(&API_SERVER) {
        Ok(mut base) => {
            let (path, query) = match req.path.split_once('?') {
                Some((p, q)) => (p, Some(q)),
                None => (req.path.as_str(), None),
            };
            base.set_path(path);
            base.set_query(query);
            base
        }
        Err(err) => {
            out.error = format!("invalid api server url: {}", err);
            return out;
        }
    };
    let mut builder = CLIENT.request(method, url);
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
