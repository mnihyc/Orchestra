#![recursion_limit = "256"]
//! Existing orch.v1 protocol on explicit browser/key paths, including immutable reauth origin.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod realtime_support;

use futures::{SinkExt, StreamExt};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::identity;
use orch_types::new_id;
use realtime_support::{PATIENCE, Rig};
use serde_json::{Value, json};
use std::net::SocketAddr;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn key(rig: &Rig) -> String {
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *rig.pool.acquire().await.unwrap(),
        &identity::NewApiToken {
            id: new_id(),
            user_id: rig.ada,
            name: "Realtime surface",
            token_hash: &token_hash(&token),
            scopes: &["*".to_owned()],
            node_id: None,
            at: rig.clock.now(),
            expires_at: None,
        },
        None,
    )
    .await
    .unwrap();
    token
}

async fn socket(address: SocketAddr, path: &str) -> Socket {
    let mut request = format!("ws://{address}{path}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("sec-websocket-protocol", "orch.v1".parse().unwrap());
    tokio_tungstenite::connect_async(request)
        .await
        .expect("the actual selected surface must expose the existing websocket")
        .0
}

async fn send(socket: &mut Socket, frame: Value) {
    socket
        .send(Message::Text(frame.to_string().into()))
        .await
        .unwrap();
}

async fn next(socket: &mut Socket) -> Value {
    tokio::time::timeout(PATIENCE, async {
        loop {
            match socket
                .next()
                .await
                .expect("socket ended without a protocol close")
                .unwrap()
            {
                Message::Text(text) => break serde_json::from_str(&text).unwrap(),
                Message::Close(frame) => {
                    break json!({"closed":frame.map(|frame|u16::from(frame.code))});
                }
                Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.unwrap(),
                _ => {}
            }
        }
    })
    .await
    .expect("existing realtime observation guard")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn surface_browser_ws_rejects_key_initial_auth_even_for_the_same_person() {
    let rig = Rig::with_unstarted_engine().await;
    let token = key(&rig).await;
    let mut browser = socket(rig.server.addr, "/api/v1/ws").await;
    send(&mut browser, json!({"op":"auth","token":token})).await;
    let observed = next(&mut browser).await;
    rig.shutdown().await;
    assert_eq!(
        observed["closed"], 4401,
        "foreign initial credential acquired a browser connection"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn surface_key_ws_preserves_key_subscription_and_rejects_session_initial_auth() {
    let rig = Rig::with_unstarted_engine().await;
    let token = key(&rig).await;
    let mut automation = socket(rig.server.addr, "/openapi/v1/ws").await;
    send(&mut automation, json!({"op":"auth","token":token})).await;
    let ready = next(&mut automation).await;
    assert_eq!(ready["op"], "ready");
    assert!(
        ready["access_expires_at"].is_null(),
        "lasting key acquired a fabricated session deadline"
    );
    send(
        &mut automation,
        json!({"op":"sub","id":1,"feed":format!("topic:{}",rig.topic)}),
    )
    .await;
    assert_eq!(
        next(&mut automation).await["op"],
        "subscribed",
        "key feed admission was lost during prefix separation"
    );
    let mut foreign = socket(rig.server.addr, "/openapi/v1/ws").await;
    send(&mut foreign, json!({"op":"auth","token":rig.token})).await;
    let observed = next(&mut foreign).await;
    rig.shutdown().await;
    assert_eq!(observed["closed"], 4401);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn surface_ws_reauth_keeps_both_same_kind_paths_and_refuses_kind_or_key_switches() {
    let rig = Rig::with_unstarted_engine().await;
    let key_one = key(&rig).await;
    let key_two = key(&rig).await;
    let next_session = rig.token_for(rig.ada, chrono::Duration::minutes(15)).await;
    for (path, first, fresh, foreign) in [
        (
            "/api/v1/ws",
            rig.token.as_str(),
            next_session.as_str(),
            key_one.as_str(),
        ),
        (
            "/openapi/v1/ws",
            key_one.as_str(),
            key_one.as_str(),
            rig.token.as_str(),
        ),
    ] {
        let mut connection = socket(rig.server.addr, path).await;
        send(&mut connection, json!({"op":"auth","token":first})).await;
        assert_eq!(next(&mut connection).await["op"], "ready");
        send(&mut connection, json!({"op":"reauth","token":fresh})).await;
        send(&mut connection, json!({"op":"ping","n":1})).await;
        assert_eq!(next(&mut connection).await, json!({"op":"pong","n":1}));
        send(&mut connection, json!({"op":"reauth","token":foreign})).await;
        assert_eq!(
            next(&mut connection).await["closed"],
            4401,
            "same-person cross-kind reauth was admitted on {path}"
        );
    }
    let mut connection = socket(rig.server.addr, "/openapi/v1/ws").await;
    send(&mut connection, json!({"op":"auth","token":key_one})).await;
    assert_eq!(next(&mut connection).await["op"], "ready");
    send(&mut connection, json!({"op":"reauth","token":key_two})).await;
    let observed = next(&mut connection).await;
    rig.shutdown().await;
    assert_eq!(
        observed["closed"], 4401,
        "the accepted key ceiling was discarded"
    );
}
