//! A single loopback protocol connection to a browser, with ordered calls and live events.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use orch_types::Json;
use serde_json::json;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// Five seconds bounds a local endpoint check inside the adoption command's fifteen seconds.
const CONNECT_WAIT: Duration = Duration::from_secs(5);
/// Protocol calls may wait for a page question, but cannot wait beyond the question's own minute.
const CALL_WAIT: Duration = Duration::from_secs(60);
/// Only one browser's immediate protocol work waits here; no image history is kept.
const CALL_QUEUE: usize = 64;
/// A short burst of metadata and the two screencast frames in flight fits here.
const EVENTS_QUEUE: usize = 32;

/// A browser could not be reached or did not understand a call.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CdpError {
    /// The endpoint is not loopback HTTP.
    #[error(
        "the endpoint must be http://127.0.0.1:<port>, localhost, or [::1], inside the environment"
    )]
    NotLocal,
    /// No browser answered there.
    #[error(
        "no browser answered; start it with --remote-debugging-port=0 and give the port in DevToolsActivePort"
    )]
    NotReachable,
    /// The protocol call failed.
    #[error("the browser refused {0}")]
    Protocol(String),
    /// The connection ended.
    #[error("the browser connection ended")]
    Closed,
    /// A call did not finish in time.
    #[error("the browser did not answer {0} in time")]
    Timeout(String),
}

/// Validates and normalizes an address without a hostname lookup or a redirect.
pub fn local_endpoint(endpoint: &str) -> Result<reqwest::Url, CdpError> {
    let mut url = reqwest::Url::parse(endpoint).map_err(|_| CdpError::NotLocal)?;
    if url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.port_or_known_default().is_none_or(|p| p == 0)
    {
        return Err(CdpError::NotLocal);
    }
    match url.host_str() {
        Some("localhost") => url
            .set_host(Some("127.0.0.1"))
            .map_err(|_| CdpError::NotLocal)?,
        Some("127.0.0.1" | "[::1]") => {}
        _ => return Err(CdpError::NotLocal),
    }
    Ok(url)
}

/// A browser's version and its own protocol socket.
#[derive(Debug, Clone)]
pub struct BrowserAddress {
    /// Its normalized HTTP endpoint.
    pub endpoint: String,
    /// Its own name and version.
    pub program: String,
    /// The socket, checked against the endpoint's loopback address and port.
    pub socket: String,
}

/// Asks an endpoint for its identity without following redirects or environment proxies.
pub async fn identify(endpoint: &str) -> Result<BrowserAddress, CdpError> {
    let url = local_endpoint(endpoint)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(CONNECT_WAIT)
        .build()
        .map_err(|_| CdpError::NotReachable)?;
    let reply = client
        .get(url.join("json/version").map_err(|_| CdpError::NotLocal)?)
        .send()
        .await
        .map_err(|_| CdpError::NotReachable)?;
    if !reply.status().is_success() {
        return Err(CdpError::NotReachable);
    }
    let version: Json = reply.json().await.map_err(|_| CdpError::NotReachable)?;
    let program = version["Browser"]
        .as_str()
        .filter(|p| p.contains("Chrome/") || p.contains("Chromium/") || p.contains("Edg/"))
        .ok_or(CdpError::NotReachable)?
        .to_owned();
    let mut socket = reqwest::Url::parse(
        version["webSocketDebuggerUrl"]
            .as_str()
            .ok_or(CdpError::NotReachable)?,
    )
    .map_err(|_| CdpError::NotReachable)?;
    if socket.host_str() == Some("localhost") {
        socket
            .set_host(Some("127.0.0.1"))
            .map_err(|_| CdpError::NotLocal)?;
    }
    if socket.scheme() != "ws"
        || socket.host_str() != url.host_str()
        || socket.port_or_known_default() != url.port_or_known_default()
        || !socket.path().starts_with("/devtools/browser/")
        || !socket.username().is_empty()
        || socket.password().is_some()
        || socket.query().is_some()
        || socket.fragment().is_some()
    {
        return Err(CdpError::NotLocal);
    }
    Ok(BrowserAddress {
        endpoint: url.as_str().trim_end_matches('/').to_owned(),
        program,
        socket: socket.to_string(),
    })
}

type Reply = oneshot::Sender<Result<Json, CdpError>>;

/// The one connection through which Orch learns of tabs and watches their pictures.
pub struct Cdp {
    next: AtomicU64,
    outgoing: mpsc::Sender<Json>,
    pending: Arc<Mutex<HashMap<u64, Reply>>>,
    events: broadcast::Sender<Json>,
}

impl Cdp {
    /// Opens a checked browser socket and begins its ordered protocol pump.
    pub async fn connect(address: &BrowserAddress) -> Result<Arc<Self>, CdpError> {
        let (socket, _) = tokio::time::timeout(
            CONNECT_WAIT,
            tokio_tungstenite::connect_async(&address.socket),
        )
        .await
        .map_err(|_| CdpError::NotReachable)?
        .map_err(|_| CdpError::NotReachable)?;
        let (outgoing, mut outgoing_rx) = mpsc::channel::<Json>(CALL_QUEUE);
        let (events, _) = broadcast::channel(EVENTS_QUEUE);
        let pending: Arc<Mutex<HashMap<u64, Reply>>> = Arc::default();
        let cdp = Arc::new(Self {
            next: AtomicU64::new(1),
            outgoing,
            pending: Arc::clone(&pending),
            events: events.clone(),
        });
        let (mut writer, mut reader) = socket.split();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    out = outgoing_rx.recv() => {
                        let Some(out) = out else { break; };
                        if writer.send(Message::Text(out.to_string().into())).await.is_err() { break; }
                    }
                    incoming = reader.next() => {
                        match incoming {
                            Some(Ok(Message::Text(text))) => {
                                let Ok(value) = serde_json::from_str::<Json>(&text) else { continue; };
                                if let Some(id) = value["id"].as_u64() {
                                    if let Some(reply) = pending.lock().unwrap_or_else(PoisonError::into_inner).remove(&id) {
                                        let result = if value.get("error").is_some() { Err(CdpError::Protocol("protocol call".to_owned())) }
                                            else { Ok(value["result"].clone()) };
                                        let _ = reply.send(result);
                                    }
                                } else { let _ = events.send(value); }
                            }
                            Some(Ok(Message::Ping(data))) => { if writer.send(Message::Pong(data)).await.is_err() { break; } }
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                            _ => {},
                        }
                    }
                }
            }
            for (_, reply) in pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .drain()
            {
                let _ = reply.send(Err(CdpError::Closed));
            }
            let _ = events.send(json!({"method":"Orch.connectionClosed"}));
        });
        Ok(cdp)
    }

    /// Watches live protocol events; none are spooled to disk.
    pub fn events(&self) -> broadcast::Receiver<Json> {
        self.events.subscribe()
    }

    /// Sends one call, to the browser or to a flattened target session.
    pub async fn call(
        &self,
        method: &str,
        params: Json,
        session: Option<&str>,
    ) -> Result<Json, CdpError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, tx);
        let mut request = json!({"id":id,"method":method,"params":params});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        if self.outgoing.send(request).await.is_err() {
            self.pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
            return Err(CdpError::Closed);
        }
        let result = tokio::time::timeout(CALL_WAIT, rx).await;
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        let result = result
            .map_err(|_| CdpError::Timeout(method.to_owned()))?
            .map_err(|_| CdpError::Closed)?;
        result.map_err(|error| match error {
            CdpError::Protocol(_) => CdpError::Protocol(method.to_owned()),
            other => other,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_announced_endpoint_can_only_name_a_loopback_browser() {
        for accepted in [
            "http://127.0.0.1:41873",
            "http://localhost:9222/",
            "http://[::1]:9222",
        ] {
            assert!(local_endpoint(accepted).is_ok(), "{accepted}");
        }
        for refused in [
            "http://127.0.0.2:9222",
            "http://example.com:9222",
            "http://127.0.0.1:0",
            "https://127.0.0.1:9222",
            "http://user@127.0.0.1:9222",
            "http://127.0.0.1:9222/other",
            "http://127.0.0.1:9222?url=x",
        ] {
            assert!(local_endpoint(refused).is_err(), "{refused}");
        }
    }
}
