//! Ordinary-key observations for documented vendors, in vendor units, with bounded read-only requests.
//!
//! The vendors' readers are extensions that the program registers in its table of built-in sources
//! (`orch_provider_contract::builtins`); the connector knows none of them by name. What is here is the transport
//! they read through on the machine that keeps the key: one GET to the vendor's own address, with the key as a
//! bearer token to that address alone, on one connection that follows no redirect, its reply bounded.

use crate::upstream::{Upstream, UpstreamRequest};
use async_trait::async_trait;
use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method};
use orch_plugin_contract::Secret;
use orch_provider_contract::builtins::VendorClient;
use orch_types::Json;
use url::Url;

/// The connector supplies the transport, so the same vendor reader runs beside either credential placement.
#[async_trait]
impl VendorClient for Upstream {
    async fn get(&self, base: &Url, path: &str, key: &Secret, reply_bytes: usize) -> Option<Json> {
        fetch_json(self, base, path, key, reply_bytes).await
    }
}

/// Only this exact address receives the key; Upstream makes one connection and never follows redirects.
async fn fetch_json(
    client: &Upstream,
    base: &Url,
    path: &str,
    key: &Secret,
    reply_bytes: usize,
) -> Option<Json> {
    let mut url = base.clone();
    url.set_path(path);
    url.set_query(None);
    url.set_fragment(None);
    let mut authorization = HeaderValue::from_str(&format!("Bearer {}", key.expose())).ok()?;
    authorization.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(http::header::AUTHORIZATION, authorization);
    let (status, _, bytes) = client
        .fetch(
            UpstreamRequest {
                method: Method::GET,
                url,
                headers,
                body: Bytes::new(),
            },
            reply_bytes,
        )
        .await
        .ok()?;
    if !status.is_success() {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
