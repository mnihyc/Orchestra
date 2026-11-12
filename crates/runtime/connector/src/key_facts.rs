//! Ordinary-key observations for documented vendors, in vendor units, with bounded read-only requests.
//!
//! The vendors' readers are extensions that the program registers in its table of built-in sources
//! (`orch_provider_contract::builtins`); the connector knows none of them by name. What is here is the transport
//! they read through on the machine that keeps the key: one GET to the vendor's own address, with the key as a
//! bearer token to that address alone, on one connection that follows no redirect, its reply bounded.

use crate::upstream::{Upstream, UpstreamRequest};
use async_trait::async_trait;
use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method};
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

    async fn get_headers(
        &self,
        base: &Url,
        path: &str,
        declared: &std::collections::BTreeMap<String, Secret>,
        reply_bytes: usize,
    ) -> Option<Json> {
        let mut headers = HeaderMap::new();
        for (name, value) in declared {
            let name = HeaderName::from_bytes(name.as_bytes()).ok()?;
            if name == http::header::HOST {
                return None;
            }
            let mut value = HeaderValue::from_str(value.expose()).ok()?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }
        fetch_with_headers(self, base, path, headers, reply_bytes).await
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
    let mut authorization = HeaderValue::from_str(&format!("Bearer {}", key.expose())).ok()?;
    authorization.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(http::header::AUTHORIZATION, authorization);
    fetch_with_headers(client, base, path, headers, reply_bytes).await
}

async fn fetch_with_headers(
    client: &Upstream,
    base: &Url,
    path: &str,
    headers: HeaderMap,
    reply_bytes: usize,
) -> Option<Json> {
    // Query cursors remain query data. An extension cannot redirect credentials to another origin.
    if !path.starts_with('/') || path.starts_with("//") {
        return None;
    }
    let url = base.join(path).ok()?;
    if url.origin() != base.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod tests {
    use super::*;
    use axum::{Json as Reply, Router, extract::OriginalUri, routing::get};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn facts_requests_keep_query_pairs_and_role_headers_on_one_origin() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&seen);
        let server = Router::new()
            .route(
                "/facts",
                get(move |OriginalUri(uri): OriginalUri, headers: HeaderMap| {
                    let capture = Arc::clone(&capture);
                    async move {
                        capture.lock().unwrap().push((uri, headers));
                        Reply(serde_json::json!({"ok":true}))
                    }
                }),
            )
            .route(
                "/redirect",
                get(|| async {
                    (
                        http::StatusCode::FOUND,
                        [(http::header::LOCATION, "/facts")],
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!(
            "http://{}/original",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let serving = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
        let client = Upstream::new(Arc::new(orch_types::SystemClock)).unwrap();
        let query = [
            ("group_by[]", "project"),
            ("group_by[]", "line item"),
            ("page", "a+/= token"),
        ];
        let mut url = base.join("/facts").unwrap();
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        let path = format!("{}?{}", url.path(), url.query().unwrap());
        let headers = BTreeMap::from([
            ("vendor-version".into(), Secret::new("test-version")),
            ("x-api-key".into(), Secret::new("test-admin-only")),
        ]);
        assert_eq!(
            client
                .get_headers(&base, &path, &headers, 128)
                .await
                .unwrap(),
            serde_json::json!({"ok":true})
        );
        {
            let seen = seen.lock().unwrap();
            let (uri, headers) = &seen[0];
            let actual = Url::parse(&format!("http://localhost{uri}")).unwrap();
            assert_eq!(actual.path(), "/facts");
            assert_eq!(
                actual
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect::<Vec<_>>(),
                query
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect::<Vec<_>>()
            );
            assert_eq!(headers["x-api-key"], "test-admin-only");
            assert_eq!(headers["vendor-version"], "test-version");
            assert!(!headers.contains_key(http::header::AUTHORIZATION));
        }
        assert!(
            client
                .get_headers(&base, &path, &headers, 2)
                .await
                .is_none(),
            "reply size remains bounded"
        );
        assert!(
            client
                .get_headers(&base, "/redirect", &headers, 128)
                .await
                .is_none()
        );
        assert_eq!(seen.lock().unwrap().len(), 2, "no redirect is followed");
        for path in [
            "https://other.invalid/facts",
            "//other.invalid/facts",
            "/facts#fragment",
        ] {
            assert!(
                client
                    .get_headers(&base, path, &headers, 128)
                    .await
                    .is_none()
            );
        }
        let mut wrong_host = headers.clone();
        wrong_host.insert("host".into(), Secret::new("other.invalid"));
        assert!(
            client
                .get_headers(&base, &path, &wrong_host, 128)
                .await
                .is_none()
        );
        assert_eq!(seen.lock().unwrap().len(), 2);
        serving.abort();
    }
}
