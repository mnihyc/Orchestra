//! A display opens a real screen, preserves safe history and never grants credentials to its readers.
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

use browsers_support::Floor;
use orch_api_contract::displays::*;
use orch_engine::displays::{OpenDisplay, Requester};
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn an_environment_without_a_machine_lists_displays_offline_without_issuing_a_look() {
    let scene = rig::Scene::new().await;
    scene.rig.world.set("vnc.enabled", json!(true)).await;
    sqlx::query("UPDATE environments SET placement='managed',status='offline',machine_id=NULL,vnc=true WHERE id=$1")
        .bind(scene.on.environment).execute(scene.rig.db.pool()).await.unwrap();
    let before = scene.rig.commands.issued_of("screen.look");
    let list = scene
        .rig
        .world
        .app
        .call(EnvironmentDisplayList, &[&scene.on.environment])
        .token(&scene.ada.token)
        .query(&EnvironmentDisplayListQuery { look: Some(true) })
        .send()
        .await
        .ok();
    assert_eq!(
        list.off,
        Some(orch_api_contract::browsers::BrowsersOff::EnvironmentOffline)
    );
    assert!(list.displays.is_empty());
    assert_eq!(list.limit.open, 0);
    assert_eq!(scene.rig.commands.issued_of("screen.look"), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_real_screen_is_listed_with_its_x_address_and_only_managers_see_the_endpoint() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    floor.serve().await;
    let view = floor.names(&floor.scene.bea, "vnc.view").await;
    let opened = floor
        .scene
        .rig
        .world
        .app
        .call(EnvironmentDisplayCreate, &[&floor.environment])
        .token(&floor.scene.ada.token)
        .body(&EnvironmentDisplayCreateBody {
            name: Some("Build preview".to_owned()),
            geometry: Some("800x600".to_owned()),
        })
        .send()
        .await
        .ok();
    assert_eq!(opened.name, "Build preview");
    assert!(opened.x_display.as_ref().unwrap().starts_with(':'));
    assert_eq!(opened.geometry.as_deref(), Some("800x600"));
    assert!(opened.endpoint.is_some());
    let port = opened
        .endpoint
        .as_ref()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .parse::<u16>()
        .unwrap();
    assert_eq!(
        floor.peer.rpc(json!({"generic_port":port}))["error"],
        "this is an internal Orch endpoint"
    );
    let list = floor
        .scene
        .rig
        .world
        .app
        .call(EnvironmentDisplayList, &[&floor.environment])
        .token(&floor.scene.bea.token)
        .query(&EnvironmentDisplayListQuery::default())
        .send()
        .await
        .ok();
    assert_eq!(list.displays[0].id, opened.id);
    assert_eq!(list.displays[0].endpoint, None);
    let denied = floor
        .scene
        .rig
        .world
        .app
        .call(DisplayTicketCreate, &[&opened.id])
        .token(&floor.scene.bea.token)
        .body(&DisplayTicketCreateBody {
            control: Some(true),
        })
        .send()
        .await;
    assert_eq!(denied.status, 403);
    let mut screen =
        displays_support::Screen::attach(&floor, &floor.scene.bea, opened.id, false).await;
    assert_eq!((screen.width, screen.height), (800, 600));
    screen.picture_without_clipboard().await;
    floor.removes(view).await;
    assert_eq!(screen.closed().await, "revoked");
    let mut manager =
        displays_support::Screen::attach(&floor, &floor.scene.ada, opened.id, false).await;
    floor
        .scene
        .rig
        .world
        .app
        .call(DisplayClose, &[&opened.id])
        .token(&floor.scene.ada.token)
        .send()
        .await
        .ok();
    assert_eq!(manager.closed().await, "quit");
    floor
        .scene
        .rig
        .world
        .app
        .call(DisplayClose, &[&opened.id])
        .token(&floor.scene.ada.token)
        .send()
        .await
        .ok();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE object_id=$1 AND action='display.closed'",
    )
    .bind(opened.id)
    .fetch_one(floor.scene.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn one_display_ticket_opens_one_connection_and_replay_cannot_interrupt_its_viewer() {
    use tokio_tungstenite::tungstenite::{Error, client::IntoClientRequest};
    let Some(floor) = Floor::new().await else {
        return;
    };
    floor.serve().await;
    let display = floor
        .scene
        .rig
        .world
        .app
        .call(EnvironmentDisplayCreate, &[&floor.environment])
        .token(&floor.scene.ada.token)
        .body(&EnvironmentDisplayCreateBody::default())
        .send()
        .await
        .ok();
    let ticket = floor
        .scene
        .rig
        .world
        .app
        .call(DisplayTicketCreate, &[&display.id])
        .token(&floor.scene.ada.token)
        .body(&DisplayTicketCreateBody {
            control: Some(false),
        })
        .send()
        .await
        .ok();
    let mut viewer =
        displays_support::Screen::with_ticket(&floor, display.id, &ticket.ticket).await;
    let address = floor.serve().await;
    let mut request = format!(
        "ws://{address}/api/v1/ws/displays/{}?ticket={}",
        display.id, ticket.ticket
    )
    .into_client_request()
    .unwrap();
    request.headers_mut().insert(
        "sec-websocket-protocol",
        DISPLAY_SUBPROTOCOL.parse().unwrap(),
    );
    match tokio_tungstenite::connect_async(request).await {
        Err(Error::Http(response)) => assert_eq!(response.status().as_u16(), 410),
        other => panic!("a redeemed ticket opened another viewer: {other:?}"),
    }
    viewer.picture_without_clipboard().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn forgetting_an_announced_display_leaves_its_original_real_server_running() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    floor.serve().await;
    let thread = floor.scene.thread().await.base.id;
    let original = floor
        .app
        .engine
        .displays()
        .open(OpenDisplay {
            environment: floor.environment,
            requester: Requester::Agent(thread),
            thread: Some(thread),
            name: Some("Owned surface".into()),
            geometry: None,
            note: None,
            origin: Default::default(),
        })
        .await
        .unwrap();
    let announced = floor
        .app
        .engine
        .displays()
        .adopt(
            floor.environment,
            thread,
            original.endpoint.clone(),
            None,
            Some("Emulator console".into()),
        )
        .await
        .unwrap();
    assert_eq!(announced.origin, "announced");
    assert_eq!(announced.note.as_deref(), Some("Emulator console"));
    floor
        .app
        .engine
        .displays()
        .close(
            announced.id,
            "closed_by_agent",
            Requester::Agent(thread),
            &Default::default(),
        )
        .await
        .unwrap();
    let mut original_screen =
        displays_support::Screen::attach(&floor, &floor.scene.ada, original.id, false).await;
    original_screen.picture_without_clipboard().await;
    let list = floor
        .scene
        .rig
        .world
        .app
        .call(EnvironmentDisplayList, &[&floor.environment])
        .token(&floor.scene.ada.token)
        .query(&EnvironmentDisplayListQuery { look: Some(true) })
        .send()
        .await
        .ok();
    assert_eq!(list.limit.open, 1);
    assert!(
        list.displays.iter().any(|d| d.id == announced.id
            && d.state == orch_api_contract::browsers::BrowserState::Closed)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_display_password_never_reaches_a_thread_item_or_its_raw_arguments() {
    use orch_api_contract::threads::{Delivery, ThreadSyncGet, ThreadSyncGetQuery};
    use orch_link_contract::events::{EmittedItemKind, Event, ItemCompleted, ItemStarted};
    let Some(floor) = Floor::new().await else {
        return;
    };
    floor.serve().await;
    let thread = floor.scene.thread().await.base.id;
    floor
        .scene
        .say(
            &floor.scene.ada,
            thread,
            "Show the console",
            Delivery::Queue,
            "display-private",
        )
        .await
        .ok();
    let turn = floor.scene.rig.turn_start(thread, 1).await;
    floor
        .scene
        .rig
        .say(thread, rig::turn_began(turn.turn_id))
        .await;
    let password = "private-display-password-marker";
    let args = json!({"command":"announce","endpoint":"localhost:5900","password":password,"note":"Emulator"});
    let payload = json!({"tool":{"kind":"orch","tool":"use_display","arguments":args,"result":null},"title":"Announce a display","started_at":floor.app.clock.now(),"duration_ms":null,"raw":{"name":"mcp__orch__use_display","input":{"inline":args.to_string(),"total_len":args.to_string().len(),"truncated":false,"blob":null},"output":null,"is_error":false}});
    let payload = orch_link_contract::foreign::ItemPayload::ToolCall(
        serde_json::from_value(payload).unwrap(),
    );
    floor
        .scene
        .rig
        .say(
            thread,
            Event::ItemStarted(ItemStarted {
                turn_id: turn.turn_id,
                native_ref: "display-private".into(),
                stream: "main".into(),
                parent_native_ref: None,
                kind: EmittedItemKind::ToolCall,
                payload: payload.clone(),
            }),
        )
        .await;
    floor
        .scene
        .rig
        .say(
            thread,
            Event::ItemCompleted(ItemCompleted {
                native_ref: "display-private".into(),
                status: orch_link_contract::foreign::ItemStatus::Completed,
                payload,
                output_blob: None,
            }),
        )
        .await;
    let sync = floor
        .scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&floor.scene.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    let kept = serde_json::to_string(&sync.items).unwrap();
    assert!(kept.contains("Emulator"));
    assert!(!kept.contains(password));
    let stored: Vec<serde_json::Value> =
        sqlx::query_scalar("SELECT payload FROM items WHERE thread_id=$1")
            .bind(thread)
            .fetch_all(floor.scene.rig.db.pool())
            .await
            .unwrap();
    assert!(!serde_json::to_string(&stored).unwrap().contains(password));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_cannot_close_another_threads_screen() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    floor.serve().await;
    let a = floor.scene.thread().await.base.id;
    let b = floor.scene.thread().await.base.id;
    let row = floor
        .app
        .engine
        .displays()
        .open(OpenDisplay {
            environment: floor.environment,
            requester: Requester::Agent(a),
            thread: Some(a),
            name: None,
            geometry: None,
            note: None,
            origin: Default::default(),
        })
        .await
        .unwrap();
    assert!(
        floor
            .app
            .engine
            .displays()
            .close(
                row.id,
                "closed_by_agent",
                Requester::Agent(b),
                &Default::default()
            )
            .await
            .is_err()
    );
    let mut conn = floor.scene.rig.db.acquire().await.unwrap();
    assert_eq!(
        orch_db::queries::displays::get(&mut conn, row.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "open"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_display_tool_reports_each_way_of_being_off_and_rejects_geometry_before_launch() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    floor.serve().await;
    let thread = floor.scene.thread().await.base.id;
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM display_sessions WHERE environment_id=$1")
            .bind(floor.environment)
            .fetch_one(floor.scene.rig.db.pool())
            .await
            .unwrap();
    let refused =
        displays_support::call(&floor, thread, json!({"command":"open","geometry":"20x20"})).await;
    assert_eq!(refused.error.unwrap().code, "invalid");
    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM display_sessions WHERE environment_id=$1")
            .bind(floor.environment)
            .fetch_one(floor.scene.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(before, after);
    for (change, reason) in [
        (
            "UPDATE environments SET vnc=false WHERE id=$1",
            "environment_switch_off",
        ),
        (
            "UPDATE environments SET vnc=true,status='offline' WHERE id=$1",
            "environment_offline",
        ),
        (
            "UPDATE environments SET status='running',placement='connected' WHERE id=$1",
            "machine_policy",
        ),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(change.to_owned()))
            .bind(floor.environment)
            .execute(floor.scene.rig.db.pool())
            .await
            .unwrap();
        let list = displays_support::call(&floor, thread, json!({"command":"list"}))
            .await
            .result
            .unwrap();
        assert_eq!(list["on"], false);
        assert_eq!(list["why_off"], reason);
        assert!(list["displays"].as_array().unwrap().is_empty());
        let open = displays_support::call(&floor, thread, json!({"command":"open"})).await;
        assert_eq!(open.error.unwrap().code, "disabled");
    }
    floor.scene.rig.world.set("vnc.enabled", json!(false)).await;
    let list = displays_support::call(&floor, thread, json!({"command":"list"}))
        .await
        .result
        .unwrap();
    assert_eq!(list["why_off"], "disabled_by_policy");
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_opens_share_the_exact_display_limit_including_browser_windows() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    floor.serve().await;
    floor
        .scene
        .rig
        .world
        .set("vnc.max_per_environment", json!(1))
        .await;
    let make = || {
        floor
            .scene
            .rig
            .world
            .app
            .call(EnvironmentDisplayCreate, &[&floor.environment])
            .token(&floor.scene.ada.token)
            .body(&EnvironmentDisplayCreateBody::default())
            .send()
    };
    let (one, two) = tokio::join!(make(), make());
    let statuses = [one.status.as_u16(), two.status.as_u16()];
    assert!(statuses.contains(&201) && statuses.contains(&409));
    let opened = if one.status.as_u16() == 201 {
        one.ok()
    } else {
        two.ok()
    };
    let browser = displays_support::window(&floor).await;
    assert_eq!(browser.display, Some(opened.id));
    let denied = floor
        .scene
        .rig
        .world
        .app
        .call(
            orch_api_contract::browsers::EnvironmentBrowserCreate,
            &[&floor.environment],
        )
        .token(&floor.scene.ada.token)
        .body(&orch_api_contract::browsers::EnvironmentBrowserCreateBody {
            profile: "another-window".into(),
            create: Some(true),
            preset: Some(orch_api_contract::browsers::BrowserPreset::Window),
            url: None,
        })
        .send()
        .await;
    assert_eq!(denied.status, 409);
    let live:i64=sqlx::query_scalar("SELECT count(*) FROM display_sessions WHERE environment_id=$1 AND origin='started' AND state<>'closed'").bind(floor.environment).fetch_one(floor.scene.rig.db.pool()).await.unwrap();
    assert_eq!(live, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_real_password_server_is_adopted_through_sealed_storage_and_asks_no_viewer_for_its_password()
 {
    orch_api_core::routes::resources::sealing::install_master_key(
        orch_db::secrets::MasterKey::new([7; 32]),
    );
    let Some(floor) = Floor::new().await else {
        return;
    };
    floor.serve().await;
    displays_support::password_commands(&floor);
    let thread = floor.scene.thread().await.base.id;
    let password = "vnc-pw7";
    let endpoint = floor.peer.rpc(json!({"launch_protected_display":password}))["ok"]["endpoint"]
        .as_str()
        .unwrap()
        .to_owned();
    let refused = displays_support::call(
        &floor,
        thread,
        json!({"command":"announce","endpoint":endpoint,"password":"wrong"}),
    )
    .await;
    assert_eq!(refused.error.unwrap().code, "not_reachable");
    let reply=displays_support::call(&floor,thread,json!({"command":"announce","endpoint":endpoint,"password":password,"note":"Protected console"})).await.result.unwrap();
    assert!(!reply.to_string().contains(password));
    let display: orch_types::Id = reply["display"].as_str().unwrap().parse().unwrap();
    let row = {
        let mut conn = floor.scene.rig.db.acquire().await.unwrap();
        orch_db::queries::displays::get(&mut conn, display)
            .await
            .unwrap()
            .unwrap()
    };
    let ciphertext = row.password.unwrap();
    assert!(
        !ciphertext
            .windows(password.len())
            .any(|part| part == password.as_bytes())
    );
    assert_eq!(
        orch_db::secrets::MasterKey::new([7; 32])
            .open(
                &ciphertext,
                format!("display/{display}/password").as_bytes()
            )
            .unwrap(),
        password.as_bytes()
    );
    floor.names(&floor.scene.bea, "vnc.view").await;
    let mut screen =
        displays_support::Screen::attach(&floor, &floor.scene.bea, display, false).await;
    assert_eq!((screen.width, screen.height), (640, 480));
    screen.picture_without_clipboard().await;
    let list = floor
        .scene
        .rig
        .world
        .app
        .call(EnvironmentDisplayList, &[&floor.environment])
        .token(&floor.scene.bea.token)
        .query(&EnvironmentDisplayListQuery::default())
        .send()
        .await
        .ok();
    assert!(!serde_json::to_string(&list).unwrap().contains(password));
    assert_eq!(list.displays[0].endpoint, None);
    let tool_list = displays_support::call(&floor, thread, json!({"command":"list"}))
        .await
        .result
        .unwrap();
    assert!(!tool_list.to_string().contains(password));
    floor
        .app
        .engine
        .displays()
        .close(
            display,
            "closed_by_agent",
            Requester::Agent(thread),
            &Default::default(),
        )
        .await
        .unwrap();
    let mut conn = floor.scene.rig.db.acquire().await.unwrap();
    assert!(
        orch_db::queries::displays::get(&mut conn, display)
            .await
            .unwrap()
            .unwrap()
            .password
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owned_screen_reuses_its_original_endpoint_and_x_display_after_registry_recovery() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    floor.serve().await;
    let display = floor
        .scene
        .rig
        .world
        .app
        .call(EnvironmentDisplayCreate, &[&floor.environment])
        .token(&floor.scene.ada.token)
        .body(&EnvironmentDisplayCreateBody::default())
        .send()
        .await
        .ok();
    let recovered = floor.peer.rpc(json!({"recover_display":display.id}))["ok"].clone();
    assert_eq!(recovered["endpoint"], display.endpoint.unwrap());
    assert_eq!(recovered["x_display"], display.x_display.unwrap());
    let mut screen =
        displays_support::Screen::attach(&floor, &floor.scene.ada, display.id, false).await;
    screen.picture_without_clipboard().await;
}
