//! Watching drops actual RFB actions before a real window sees them, including clipboard directions.
#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod browsers_support;
mod displays_support;
mod places;
mod realtime_support;
mod rig;
mod support;

use browsers_support::{Floor, WAIT};
use displays_support::{Screen, key, paste};
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn watching_a_real_program_drops_keys_clicks_paste_and_resize_but_still_draws() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    floor.serve().await;
    let browser = displays_support::window(&floor).await;
    let display = browser.display.unwrap();
    floor.names(&floor.scene.cai, "vnc.view").await;
    floor.peer.observe(browser.id,"document.getElementById('i').focus(); window.actions=0; document.getElementById('i').value=''; 0");
    let mut watch = Screen::attach(&floor, &floor.scene.cai, display, false).await;
    let mut control = Screen::attach(&floor, &floor.scene.ada, display, true).await;
    let before = floor.peer.rpc(json!({"tunnels":true}))["ok"]
        .as_u64()
        .unwrap();
    assert!(before >= 2);
    for action in [
        key(65, true),
        key(65, false),
        vec![5, 1, 0, 100, 0, 150],
        vec![5, 0, 0, 100, 0, 150],
        paste("watch-paste-private"),
        vec![251, 0, 0, 80, 0, 60, 0, 0],
    ] {
        watch.send(&action[..1]).await;
        watch.send(&action[1..]).await;
    }
    watch.picture_without_clipboard().await;
    assert_eq!(floor.peer.observe(browser.id, "window.actions"), json!(0));
    assert_eq!(
        floor
            .peer
            .observe(browser.id, "document.getElementById('i').value"),
        json!("")
    );
    control.send(&key(65, true)).await;
    control.send(&key(65, false)).await;
    tokio::time::timeout(WAIT, async {
        while floor
            .peer
            .observe(browser.id, "window.actions")
            .as_i64()
            .unwrap()
            == 0
        {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    control.picture_without_clipboard().await;
    watch.picture_without_clipboard().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_real_program_receives_paste_but_its_copied_text_is_never_sent_to_any_viewer() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    floor.serve().await;
    let browser = displays_support::window(&floor).await;
    let display = browser.display.unwrap();
    floor.names(&floor.scene.cai, "vnc.view").await;
    floor.peer.observe(
        browser.id,
        "document.getElementById('i').focus();document.getElementById('i').value='';0",
    );
    let mut control = Screen::attach(&floor, &floor.scene.ada, display, true).await;
    let mut watch = Screen::attach(&floor, &floor.scene.cai, display, false).await;
    control.send(&paste("controller-paste-marker")).await;
    for event in [
        key(0xffe3, true),
        key(118, true),
        key(118, false),
        key(0xffe3, false),
    ] {
        control.send(&event).await;
    }
    tokio::time::timeout(WAIT, async {
        while floor
            .peer
            .observe(browser.id, "document.getElementById('i').value")
            != json!("controller-paste-marker")
        {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    floor.peer.observe(browser.id,"document.getElementById('i').value='copied-private-marker';document.getElementById('i').select();0");
    for event in [
        key(0xffe3, true),
        key(99, true),
        key(99, false),
        key(0xffe3, false),
    ] {
        control.send(&event).await;
    }
    control.picture_without_clipboard().await;
    watch.picture_without_clipboard().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_live_controller_loses_manage_without_losing_its_picture_then_loses_view() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    floor.serve().await;
    let browser = displays_support::window(&floor).await;
    let display = browser.display.unwrap();
    let view = floor.names(&floor.scene.bea, "vnc.view").await;
    let manage = floor.names(&floor.scene.bea, "vnc.manage").await;
    let mut screen = Screen::attach(&floor, &floor.scene.bea, display, true).await;
    screen.picture_without_clipboard().await;
    floor.removes(manage).await;
    floor.peer.observe(
        browser.id,
        "document.getElementById('i').focus();window.actions=0;0",
    );
    screen.send(&key(65, true)).await;
    screen.send(&key(65, false)).await;
    screen.picture_without_clipboard().await;
    assert_eq!(floor.peer.observe(browser.id, "window.actions"), json!(0));
    floor.removes(view).await;
    assert_eq!(screen.closed().await, "revoked");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_display_sitting_records_only_the_person_and_times_and_a_retry_keeps_one_line() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    floor.serve().await;
    let browser = displays_support::window(&floor).await;
    let display = browser.display.unwrap();
    floor.peer.observe(
        browser.id,
        "document.getElementById('i').focus();window.actions=0;0",
    );
    let mut screen = Screen::attach(&floor, &floor.scene.ada, display, true).await;
    screen.send(&key(65, true)).await;
    screen.send(&key(65, false)).await;
    tokio::time::timeout(WAIT, async {
        while floor
            .peer
            .observe(browser.id, "window.actions")
            .as_i64()
            .unwrap()
            == 0
        {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    floor.scene.rig.world.advance(chrono::Duration::minutes(6));
    let entries:Vec<serde_json::Value>=tokio::time::timeout(WAIT,async{loop{
        let entries:Vec<serde_json::Value>=sqlx::query_scalar("SELECT details FROM audit_log WHERE action='display.driven' AND object_id=$1 AND actor_id=$2").bind(display).bind(floor.scene.ada.id()).fetch_all(floor.scene.rig.db.pool()).await.unwrap();
        if !entries.is_empty(){break entries;}tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }}).await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].as_object().unwrap().len(), 2);
    assert!(entries[0].get("from").is_some() && entries[0].get("to").is_some());
    let to: chrono::DateTime<chrono::Utc> =
        serde_json::from_value(entries[0]["to"].clone()).unwrap();
    let start = serde_json::from_value(entries[0]["from"].clone()).unwrap();
    floor
        .app
        .engine
        .displays()
        .sitting(display, floor.scene.ada.id(), start, to)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action='display.driven' AND object_id=$1",
    )
    .bind(display)
    .fetch_one(floor.scene.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(count, 1);
}
