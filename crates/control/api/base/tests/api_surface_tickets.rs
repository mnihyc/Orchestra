//! Existing terminal tickets must retain their transport plane without being consumed by the other.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]
mod places;
mod rig;
mod support;
mod terminals_support;

use crate::support::FullTicketReply as _;
use futures::StreamExt;
use orch_api_contract::terminals::{AgentTerminalTicketCreate, AgentTerminalTicketCreateBody};
use std::net::SocketAddr;
use terminals_support::{Floor, PATIENCE};
use tokio_tungstenite::tungstenite::{Error, Message, client::IntoClientRequest};

/// A missing route is deliberately not an authentication refusal: BEFORE must expose that gap.
async fn foreign_refusal(address: SocketAddr, path: &str, protocol: &str) -> u16 {
    let mut request = format!("ws://{address}{path}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("sec-websocket-protocol", protocol.parse().unwrap());
    match tokio_tungstenite::connect_async(request).await {
        Err(Error::Http(response)) => response.status().as_u16(),
        Err(error) => panic!("unexpected transport failure: {error}"),
        Ok((mut socket, _)) => tokio::time::timeout(PATIENCE, async {
            loop {
                match socket
                    .next()
                    .await
                    .expect("no authentication close")
                    .unwrap()
                {
                    Message::Close(Some(frame)) => break u16::from(frame.code),
                    Message::Text(text) => panic!("foreign ticket produced a data frame: {text}"),
                    Message::Binary(_) => panic!("foreign ticket produced bytes"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn surface_terminal_foreign_plane_does_not_consume_the_same_valid_ticket() {
    let floor = Floor::new().await;
    let made = floor.shell_of(&floor.scene.dan).await;
    let address = floor.serve().await;
    let refused = foreign_refusal(
        address,
        &format!(
            "/openapi/v1/ws/terminals/{}?ticket={}",
            made.session.id, made.ticket
        ),
        "orch.terminal.v1",
    )
    .await;
    let mut original = floor.screen(made.session.id, &made.ticket).await;
    original.ready().await;
    original.type_("echo surface-ticket-survived\n").await;
    original.until("surface-ticket-survived\r\n").await;
    original.drop_it().await;
    floor.stop().await;
    assert!(
        matches!(refused, 401 | 403 | 4401),
        "foreign path was absent or passed its ticket: {refused}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn surface_agent_terminal_foreign_plane_does_not_consume_its_ticket() {
    let floor = Floor::new().await;
    // The same capability premise used by the existing terminals_sockets agent controls.
    sqlx::query("UPDATE agents SET descriptor=jsonb_set(descriptor,'{capabilities,agent_terminal}',to_jsonb($2::text)) WHERE id=$1")
        .bind(floor.scene.on.agent).bind("interactive").execute(floor.pool()).await.unwrap();
    let ticket = floor
        .scene
        .rig
        .world
        .app
        .call(AgentTerminalTicketCreate, &[&floor.thread])
        .token(&floor.scene.dan.token)
        .body(&AgentTerminalTicketCreateBody {
            control: Some(true),
        })
        .send()
        .await
        .full_ticket()
        .ok()
        .ticket;
    let address = floor.serve().await;
    let refused = foreign_refusal(
        address,
        &format!(
            "/openapi/v1/ws/agent-terminals/{}?ticket={ticket}",
            floor.thread
        ),
        "orch.terminal.v1",
    )
    .await;
    let mut original = floor.agent_screen(&ticket).await;
    original.ready().await;
    original.drop_it().await;
    floor.stop().await;
    assert!(
        matches!(refused, 401 | 403 | 4401),
        "foreign path was absent or passed its ticket: {refused}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn surface_all_five_continuation_routes_keep_their_original_protocols() {
    let floor = Floor::new().await;
    let address = floor.serve().await;
    let id = orch_types::new_id();
    let mut outcomes = Vec::new();
    for (kind, protocol) in [
        ("terminals", "orch.terminal.v1"),
        ("agent-terminals", "orch.terminal.v1"),
        ("browsers", "orch.browser.v1"),
        ("displays", "orch.display.v1"),
        ("ports", "orch.port.v1"),
    ] {
        let status = foreign_refusal(
            address,
            &format!("/openapi/v1/ws/{kind}/{id}?ticket=invalid-control-ticket"),
            protocol,
        )
        .await;
        outcomes.push((kind, status));
    }
    floor.stop().await;
    assert!(
        outcomes
            .iter()
            .all(|(_, status)| matches!(status, 401 | 403 | 410 | 4401)),
        "missing routes or changed continuation protocols: {outcomes:?}"
    );
}
