//! Portable RFB socket client and client messages, without native environment setup.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(
    dead_code,
    reason = "each socket control uses part of this shared client"
)]

use super::WAIT;

use bytes::Bytes;

use bytes::BytesMut;

use futures::SinkExt;

use futures::StreamExt;

use orch_api_contract::displays::DISPLAY_CLOSE_CODE;

use orch_api_contract::displays::DISPLAY_SUBPROTOCOL;

use tokio_tungstenite::tungstenite::Message;

use tokio_tungstenite::tungstenite::client::IntoClientRequest;
/// A real WebSocket RFB client with explicit picture requests.
pub struct Screen {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    pending: BytesMut,
    /// The server's framebuffer width.
    pub width: u16,
    /// The server's framebuffer height.
    pub height: u16,
    /// The server's pixel size, used by the raw-only fixture viewer.
    bytes_per_pixel: usize,
}

impl Screen {
    /// Reuses this exact RFB handshake on an explicit declared surface.
    pub async fn connect_at(address: std::net::SocketAddr, path: &str) -> Self {
        let socket = Self::socket_at(address, path).await;
        let mut screen = Self {
            socket,
            pending: BytesMut::new(),
            width: 0,
            height: 0,
            bytes_per_pixel: 0,
        };
        let version = screen.take(12).await;
        assert_eq!(version.as_ref(), b"RFB 003.008\n");
        screen.send(&version).await;
        assert_eq!(screen.take(2).await.as_ref(), &[1, 1]);
        screen.send(&[1]).await;
        assert_eq!(screen.take(4).await.as_ref(), &[0, 0, 0, 0]);
        screen.send(&[0]).await; // The environment must force shared regardless of this hostile choice.
        let init = screen.take(24).await;
        screen.width = u16::from_be_bytes([init[0], init[1]]);
        screen.height = u16::from_be_bytes([init[2], init[3]]);
        screen.bytes_per_pixel = usize::from(init[4]) / 8;
        let length = u32::from_be_bytes([init[20], init[21], init[22], init[23]]) as usize;
        screen.take(length).await;
        screen.send(&[2, 0, 0, 1, 0, 0, 0, 0]).await; // Request only the standard raw encoding.
        screen
    }
    pub async fn socket_at(
        address: std::net::SocketAddr,
        path: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let mut request = format!("ws://{address}{path}")
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            DISPLAY_SUBPROTOCOL.parse().unwrap(),
        );
        let (socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
        assert_eq!(
            response.headers()["sec-websocket-protocol"],
            DISPLAY_SUBPROTOCOL
        );
        socket
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

    /// Sends binary RFB bytes; WebSocket boundaries deliberately need not be message boundaries.
    pub async fn send(&mut self, bytes: &[u8]) {
        self.socket
            .send(Message::Binary(Bytes::copy_from_slice(bytes)))
            .await
            .unwrap();
    }
    async fn take(&mut self, length: usize) -> Bytes {
        tokio::time::timeout(WAIT, async {
            while self.pending.len() < length {
                match self.socket.next().await.unwrap().unwrap() {
                    Message::Binary(bytes) => self.pending.extend_from_slice(&bytes),
                    Message::Ping(bytes) => self.socket.send(Message::Pong(bytes)).await.unwrap(),
                    Message::Close(reason) => panic!("display ended: {reason:?}"),
                    _ => {}
                }
            }
            self.pending.split_to(length).freeze()
        })
        .await
        .unwrap()
    }
    /// Requests a new one-pixel update, asserting that no clipboard was returned before it.
    pub async fn picture_without_clipboard(&mut self) {
        self.send(&[3, 0, 0, 0, 0, 0, 0, 1, 0, 1]).await;
        loop {
            match self.take(1).await[0] {
                0 => {
                    let header = self.take(3).await;
                    let rectangles = u16::from_be_bytes([header[1], header[2]]);
                    for _ in 0..rectangles {
                        let rectangle = self.take(12).await;
                        let width = usize::from(u16::from_be_bytes([rectangle[4], rectangle[5]]));
                        let height = usize::from(u16::from_be_bytes([rectangle[6], rectangle[7]]));
                        assert_eq!(
                            i32::from_be_bytes([
                                rectangle[8],
                                rectangle[9],
                                rectangle[10],
                                rectangle[11]
                            ]),
                            0
                        );
                        self.take(width * height * self.bytes_per_pixel).await;
                    }
                    return;
                }
                2 => {}
                3 => panic!("a display opened by Orch sent copied text to a viewer"),
                other => panic!("unexpected RFB server message {other}"),
            }
        }
    }
    /// Takes the application close reason while discarding outstanding picture bytes.
    pub async fn closed(&mut self) -> String {
        tokio::time::timeout(WAIT, async {
            loop {
                if let Message::Close(Some(frame)) = self.socket.next().await.unwrap().unwrap() {
                    assert_eq!(u16::from(frame.code), DISPLAY_CLOSE_CODE);
                    return frame.reason.to_string();
                }
            }
        })
        .await
        .unwrap()
    }
}

/// A classic key event, whose bytes must be absent from a watching stream.
pub fn key(symbol: u32, down: bool) -> Vec<u8> {
    let mut bytes = vec![4, u8::from(down), 0, 0];
    bytes.extend(symbol.to_be_bytes());
    bytes
}

/// Classic pasted text, whose bytes must be absent from a watching stream.
pub fn paste(value: &str) -> Vec<u8> {
    let mut bytes = vec![6, 0, 0, 0];
    bytes.extend((value.len() as u32).to_be_bytes());
    bytes.extend(value.as_bytes());
    bytes
}
