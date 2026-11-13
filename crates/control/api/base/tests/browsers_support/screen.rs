//! Portable real browser-socket client, independent of native environment peers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(
    dead_code,
    reason = "each socket control uses part of this shared client"
)]

use futures::SinkExt;

use futures::StreamExt;

use orch_types::Json;

use serde_json::json;

use std::time::Duration;

use tokio_tungstenite::tungstenite::Message;

use tokio_tungstenite::tungstenite::client::IntoClientRequest;
/// Match the bounded native browser fixture and socket publication waits.
pub const WAIT: Duration = Duration::from_secs(60);

/// A real client whose picture acknowledgements are explicit.
pub struct Screen {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl Screen {
    /// The same real client can target either declared HTTP surface with an already minted ticket.
    pub async fn connect_at(addr: std::net::SocketAddr, path: &str) -> Self {
        let mut request = format!("ws://{addr}{path}").into_client_request().unwrap();
        request
            .headers_mut()
            .insert("sec-websocket-protocol", "orch.browser.v1".parse().unwrap());
        let (socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        Self { socket }
    }
    pub async fn next_wire(&mut self) -> Option<Message> {
        tokio::time::timeout(WAIT, self.socket.next())
            .await
            .unwrap()
            .map(Result::unwrap)
    }
    pub async fn close(mut self) {
        let _ = self.socket.close(None).await;
    }

    /// Sends one frame exactly as a browser client would.
    pub async fn send(&mut self, value: Json) {
        self.socket
            .send(Message::Text(value.to_string().into()))
            .await
            .unwrap();
    }
    /// Takes the requested metadata while acknowledging intervening pictures.
    pub async fn text(&mut self, op: &str) -> Json {
        tokio::time::timeout(WAIT, async {
            loop {
                match self.socket.next().await.unwrap().unwrap() {
                    Message::Text(text) => {
                        let value: Json = serde_json::from_str(&text).unwrap();
                        if value["op"] == "error" && op != "error" {
                            panic!("unexpected browser refusal: {value}");
                        }
                        if value["op"] == op {
                            return value;
                        }
                    }
                    Message::Binary(bytes) => {
                        let n = orch_realtime_contract::browser::PictureHeader::decode(&bytes)
                            .unwrap()
                            .n;
                        self.send(json!({"op":"ack","n":n})).await;
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap()
    }
    /// Takes one picture without acknowledging it, allowing tests to emulate a slow viewer.
    pub async fn picture(&mut self) -> bytes::Bytes {
        tokio::time::timeout(WAIT, async {
            loop {
                if let Message::Binary(bytes) = self.socket.next().await.unwrap().unwrap() {
                    return bytes;
                }
            }
        })
        .await
        .unwrap()
    }
    /// Acknowledges the supplied picture as drawn.
    pub async fn drawn(&mut self, bytes: &[u8]) {
        let n = orch_realtime_contract::browser::PictureHeader::decode(bytes)
            .unwrap()
            .n;
        self.send(json!({"op":"ack","n":n})).await;
    }
}
