//! Viewer tickets are rechecked against the current session that issued them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;
mod terminals_support;

use futures::StreamExt;
use orch_api_contract::browsers::{BrowserTicketCreate, BrowserTicketCreateBody};
use orch_api_contract::displays::{DisplayTicketCreate, DisplayTicketCreateBody};
use orch_types::new_id;
use serde_json::json;
use terminals_support::Floor;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

#[tokio::test(flavor = "multi_thread")]
async fn switching_admin_mode_off_invalidates_existing_viewer_tickets() {
    let floor = Floor::new().await;
    floor
        .scene
        .rig
        .world
        .set("browser.enabled", json!(true))
        .await;
    floor.scene.rig.world.set("vnc.enabled", json!(true)).await;
    let user = &floor.scene.bea;
    sqlx::query("UPDATE users SET is_system_admin=true WHERE id=$1")
        .bind(user.id())
        .execute(floor.pool())
        .await
        .unwrap();
    places::admin_mode(&floor.scene.rig.world, user, true).await;
    sqlx::query("UPDATE environments SET browser=true,vnc=true WHERE id=$1")
        .bind(floor.environment)
        .execute(floor.pool())
        .await
        .unwrap();
    let browser = new_id();
    let display = new_id();
    sqlx::query("INSERT INTO browser_sessions(id,environment_id,env_generation,origin,endpoint,state) VALUES($1,$2,1,'announced','127.0.0.1:41010','open')")
        .bind(browser).bind(floor.environment).execute(floor.pool()).await.unwrap();
    sqlx::query("INSERT INTO display_sessions(id,environment_id,env_generation,name,origin,endpoint,state) VALUES($1,$2,1,'screen','announced','127.0.0.1:41011','open')")
        .bind(display).bind(floor.environment).execute(floor.pool()).await.unwrap();
    let browser_ticket = floor
        .scene
        .rig
        .world
        .app
        .call(BrowserTicketCreate, &[&browser])
        .token(&user.token)
        .body(&BrowserTicketCreateBody {
            control: Some(true),
        })
        .send()
        .await
        .ok()
        .ticket;
    let display_ticket = floor
        .scene
        .rig
        .world
        .app
        .call(DisplayTicketCreate, &[&display])
        .token(&user.token)
        .body(&DisplayTicketCreateBody {
            control: Some(true),
        })
        .send()
        .await
        .ok()
        .ticket;
    places::admin_mode(&floor.scene.rig.world, user, false).await;

    let address = floor.serve().await;
    let mut request =
        format!("ws://{address}/api/v1/ws/browsers/{browser}?ticket={browser_ticket}")
            .into_client_request()
            .unwrap();
    request
        .headers_mut()
        .insert("sec-websocket-protocol", "orch.browser.v1".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Text(message) = message else {
        panic!("expected browser closure")
    };
    let closed: serde_json::Value = serde_json::from_str(message.as_str()).unwrap();
    assert_eq!(
        (closed["op"].as_str(), closed["reason"].as_str()),
        (Some("closed"), Some("revoked"))
    );

    let mut request =
        format!("ws://{address}/api/v1/ws/displays/{display}?ticket={display_ticket}")
            .into_client_request()
            .unwrap();
    request
        .headers_mut()
        .insert("sec-websocket-protocol", "orch.display.v1".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let message = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Close(Some(frame)) = message else {
        panic!("expected display closure")
    };
    assert_eq!(frame.reason.as_str(), "revoked");
    floor.stop().await;
}
