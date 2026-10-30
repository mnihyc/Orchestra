//! Display clients use the real shipped-image peer and the same guarded API fixture as browsers.
//! This is a shared test module, matching browsers_support rather than a standalone integration crate.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(
    dead_code,
    reason = "each display case uses part of the shared protocol client"
)]

use crate::browsers_support::{Floor, WAIT};
use crate::places::Who;
use bytes::{Bytes, BytesMut};
use futures::{SinkExt, StreamExt};
use orch_api_contract::{browsers::*, displays::*};
use orch_link_contract::commands::{Command, CommandKind, OrchToolResult};
use orch_types::Id;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

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
    /// Connects a viewer without receiving or supplying any VNC password.
    pub async fn attach(floor: &Floor, by: &Who, display: Id, control: bool) -> Self {
        let ticket = floor
            .scene
            .rig
            .world
            .app
            .call(DisplayTicketCreate, &[&display])
            .token(&by.token)
            .body(&DisplayTicketCreateBody {
                control: Some(control),
            })
            .send()
            .await
            .ok();
        Self::with_ticket(floor, display, &ticket.ticket).await
    }
    /// Completes the real viewer handshake with one already issued ticket.
    pub async fn with_ticket(floor: &Floor, display: Id, ticket: &str) -> Self {
        let address = floor.serve().await;
        let mut request = format!("ws://{address}/api/v1/ws/displays/{display}?ticket={ticket}")
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

/// Opens a real Chromium window on the same Xvnc implementation displays use.
pub async fn window(floor: &Floor) -> Browser {
    floor
        .scene
        .rig
        .world
        .app
        .call(EnvironmentBrowserCreate, &[&floor.environment])
        .token(&floor.scene.ada.token)
        .body(&EnvironmentBrowserCreateBody {
            profile: "display-program".to_owned(),
            create: Some(true),
            preset: Some(BrowserPreset::Window),
            url: Some("http://127.0.0.1:41820/".to_owned()),
        })
        .send()
        .await
        .ok()
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

/// Dispatches the actual display tool, rather than calling its lifecycle helper directly.
pub async fn call(floor: &Floor, thread: Id, arguments: orch_types::Json) -> OrchToolResult {
    let call_ref = orch_types::new_id().to_string();
    floor
        .app
        .engine
        .answer_tool_call(&orch_engine::tools::CallContext {
            machine: floor.scene.rig.machine_id,
            thread,
            turn: orch_types::new_id(),
            call_ref: call_ref.clone(),
            attempt: None,
            tool: orch_agent_contract::events::OrchTool::UseDisplay,
            arguments,
        })
        .await
        .unwrap();
    tokio::time::timeout(WAIT, async {
        loop {
            if let Some(result) = floor
                .scene
                .rig
                .seen
                .of_kind(CommandKind::OrchToolResult)
                .into_iter()
                .find_map(|command| match command {
                    Command::OrchToolResult(result)
                        if result.thread_id == thread && result.call_ref == call_ref =>
                    {
                        Some(result)
                    }
                    _ => None,
                })
            {
                return result;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

/// A test command sink opens only its known fixture key before passing the private value to Xvnc.
pub fn password_commands(floor: &Floor) {
    let peer = std::sync::Arc::clone(&floor.peer);
    let db = floor.scene.rig.db.clone();
    floor
        .scene
        .rig
        .machine
        .handler
        .on(CommandKind::DisplayAdopt, move |command| {
            let password = match &command {
                Command::DisplayAdopt(body) => match body.password {
                    Some(orch_link_contract::commands::Secret::Ref(id)) => {
                        Some(tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current().block_on(async {
                                let mut conn = db.acquire().await.unwrap();
                                let row = orch_db::queries::displays::get(&mut conn, id)
                                    .await
                                    .unwrap()
                                    .unwrap();
                                String::from_utf8(
                                    orch_db::secrets::MasterKey::new([7; 32])
                                        .open(
                                            row.password.as_ref().unwrap(),
                                            format!("display/{id}/password").as_bytes(),
                                        )
                                        .unwrap(),
                                )
                                .unwrap()
                            })
                        }))
                    }
                    None => None,
                    _ => panic!("the test sink needs a stored secret reference"),
                },
                _ => panic!("the test sink only handles display.adopt"),
            };
            let result = peer.rpc(serde_json::json!({"command":command,"password":password}));
            if let Some(error) = result.get("error") {
                return Err(orch_link_contract::commands::CommandError {
                    code: error["code"].as_str().unwrap().to_owned(),
                    detail: error["detail"].as_str().unwrap().to_owned(),
                    retryable: false,
                    details: None,
                });
            }
            Ok(
                serde_json::from_value::<orch_link_contract::commands::CommandResult>(
                    result["ok"].clone(),
                )
                .unwrap(),
            )
        });
}
