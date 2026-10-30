//! Browser gates, joining, tickets, view-only input and host-only sitting history over the real browser.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod browsers_support;
mod places;
mod realtime_support;
mod rig;
mod support;

use browsers_support::Floor;
use orch_api_contract::browsers::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn threads_with_the_same_time_prefix_have_separate_sign_ins_and_join_only_their_own_browser()
{
    let Some(floor) = Floor::new().await else {
        return;
    };
    floor.serve().await;
    let (a, b) = tokio::time::timeout(browsers_support::WAIT, async {
        let mut a = floor
            .scene
            .thread_by(&floor.scene.ada, "Own browser one")
            .await;
        loop {
            let b = floor
                .scene
                .thread_by(&floor.scene.ada, "Own browser two")
                .await;
            if a.base.id.to_string()[..8] == b.base.id.to_string()[..8] {
                break (a, b);
            }
            // Crossing the UUID clock prefix boundary does not weaken the fixture's premise.
            a = b;
        }
    })
    .await
    .unwrap();
    let arguments = json!({"command":"open", "url":"http://127.0.0.1:41820/"});
    let first = browsers_support::fake::call(&floor, a.base.id, arguments.clone())
        .await
        .result
        .unwrap();
    let second = browsers_support::fake::call(&floor, b.base.id, arguments.clone())
        .await
        .result
        .unwrap();
    let first_id: orch_types::Id = first["browser"].as_str().unwrap().parse().unwrap();
    let second_id: orch_types::Id = second["browser"].as_str().unwrap().parse().unwrap();
    assert_ne!(first_id, second_id);
    assert_eq!(first["profile"], format!("thread-{}", a.base.id.simple()));
    assert_eq!(second["profile"], format!("thread-{}", b.base.id.simple()));
    assert_eq!(first["started"], true);
    assert_eq!(second["started"], true);
    tokio::time::timeout(browsers_support::WAIT, async {
        while floor.peer.observe(first_id, "location.hostname") != "127.0.0.1" {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    floor.peer.observe(
        first_id,
        "document.cookie='own_sign_in=first; path=/'; document.cookie",
    );
    let has_login = |id| {
        floor.peer.rpc(json!({"cookies":id}))["ok"]["cookies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cookie| cookie["name"] == "own_sign_in" && cookie["value"] == "first")
    };
    assert!(has_login(first_id));
    assert!(!has_login(second_id));
    let joined = browsers_support::fake::call(&floor, a.base.id, arguments)
        .await
        .result
        .unwrap();
    assert_eq!(joined["browser"], first["browser"]);
    assert_eq!(joined["started"], false);
    assert!(has_login(first_id));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_whole_agent_note_reaches_tool_http_and_feed_lists_and_an_overlong_note_opens_nothing() {
    use orch_realtime_contract::feeds::Feed;
    let Some(floor) = Floor::new().await else {
        return;
    };
    let addr = floor.serve().await;
    let thread = floor.scene.thread().await.base.id;
    let note = "界".repeat(200);
    let opened =
        browsers_support::fake::call(&floor, thread, json!({"command":"open","note":note}))
            .await
            .result
            .unwrap();
    let browser: orch_types::Id = opened["browser"].as_str().unwrap().parse().unwrap();
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM browser_sessions WHERE environment_id=$1")
            .bind(floor.environment)
            .fetch_one(floor.scene.rig.db.pool())
            .await
            .unwrap();
    for command in ["open", "announce"] {
        let mut args = json!({"command":command,"note":"界".repeat(201)});
        if command == "announce" {
            args["endpoint"] = json!("http://127.0.0.1:41821");
        }
        let refused = browsers_support::fake::call(&floor, thread, args).await;
        assert_eq!(refused.error.unwrap().code, "invalid");
    }
    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM browser_sessions WHERE environment_id=$1")
            .bind(floor.environment)
            .fetch_one(floor.scene.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(before, after);
    let cli = floor.peer.rpc(json!({"launch_cli":true}));
    let announced = browsers_support::fake::call(&floor, thread,
        json!({"command":"announce","endpoint":cli["ok"]["endpoint"],"note":"Checking the payment screen"})).await.result.unwrap();
    let announced_id: orch_types::Id = announced["browser"].as_str().unwrap().parse().unwrap();
    let listed = browsers_support::fake::call(&floor, thread, json!({"command":"list"}))
        .await
        .result
        .unwrap();
    for (id, expected) in [
        (browser, note.as_str()),
        (announced_id, "Checking the payment screen"),
    ] {
        assert!(
            listed["browsers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["browser"] == id.to_string() && row["note"] == expected)
        );
    }
    let http = floor
        .scene
        .rig
        .world
        .app
        .call(EnvironmentBrowserList, &[&floor.environment])
        .token(&floor.scene.ada.token)
        .query(&EnvironmentBrowserListQuery::default())
        .send()
        .await
        .ok();
    assert!(
        http.browsers
            .iter()
            .any(|row| row.id == browser && row.note.as_deref() == Some(note.as_str()))
    );
    assert!(
        http.browsers.iter().any(|row| row.id == announced_id
            && row.note.as_deref() == Some("Checking the payment screen"))
    );
    let mut client = realtime_support::Client::signed_in(addr, &floor.scene.ada.token).await;
    client
        .sub(1, &Feed::EnvBrowsers(floor.environment).to_string(), None)
        .await;
    client.expect("subscribed").await;
    let event = client.expect("event").await;
    let rows = event["data"]["browsers"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|row| row["id"] == browser.to_string() && row["note"] == note)
    );
    assert!(
        rows.iter().any(|row| row["id"] == announced_id.to_string()
            && row["note"] == "Checking the payment screen")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_first_address_and_note_are_thread_content_but_a_browser_sitting_keeps_hosts_only()
 {
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
            "Check this screen",
            Delivery::Queue,
            "browser-content",
        )
        .await
        .ok();
    let turn = floor.scene.rig.turn_start(thread, 1).await;
    floor
        .scene
        .rig
        .say(thread, rig::turn_began(turn.turn_id))
        .await;
    let url = "http://127.0.0.1:41820/private-path?private-first-address-marker=1";
    let note = "private-tool-note-marker";
    let arguments = json!({"command":"open","url":url,"note":note});
    let payload = json!({"tool":{"kind":"orch","tool":"use_browser","arguments":arguments,"result":null},
        "title":"Open a browser", "started_at":floor.app.clock.now(),"duration_ms":null,
        "raw":{"name":"mcp__orch__use_browser","input":{"inline":arguments.to_string(),"total_len":arguments.to_string().len(),"truncated":false,"blob":null},"output":null,"is_error":false}});
    let native = "private-browser-call";
    floor
        .scene
        .rig
        .say(
            thread,
            Event::ItemStarted(ItemStarted {
                turn_id: turn.turn_id,
                native_ref: native.to_owned(),
                stream: "main".to_owned(),
                parent_native_ref: None,
                kind: EmittedItemKind::ToolCall,
                payload: serde_json::from_value(payload.clone()).unwrap(),
            }),
        )
        .await;
    let reply = browsers_support::fake::call_on(&floor, thread, turn.turn_id, arguments.clone())
        .await
        .result
        .unwrap();
    let mut done = payload;
    done["tool"]["result"] = reply.clone();
    floor
        .scene
        .rig
        .say(
            thread,
            Event::ItemCompleted(ItemCompleted {
                native_ref: native.to_owned(),
                status: orch_link_contract::foreign::ItemStatus::Completed,
                payload: serde_json::from_value(done).unwrap(),
                output_blob: None,
            }),
        )
        .await;
    let sync = tokio::time::timeout(browsers_support::WAIT, async {
        loop {
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
            if serde_json::to_string(&sync.items).unwrap().contains(note) {
                break sync;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let items = serde_json::to_value(&sync.items).unwrap();
    assert!(
        items
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["payload"]["tool"]["arguments"] == arguments)
    );
    assert!(serde_json::to_string(&sync.items).unwrap().contains(url));
    let browser = reply["browser"].as_str().unwrap().parse().unwrap();
    let ticket = floor.ticket(&floor.scene.ada, browser, true).await;
    let mut screen = floor.screen(browser, &ticket.ticket).await;
    let ready = screen.text("ready").await;
    let tab = ready["tabs"][0]["id"].as_str().unwrap();
    screen
        .send(json!({"op":"watch","tab":tab,"width":800,"height":600,"dpr":1}))
        .await;
    screen
        .send(json!({"op":"navigate","tab":tab,"url":url}))
        .await;
    tokio::time::timeout(browsers_support::WAIT, async {
        while floor.peer.observe(browser, "location.search") != "?private-first-address-marker=1" {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    floor.scene.rig.world.advance(chrono::Duration::minutes(6));
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let details: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE environment_id=$1 AND action='browser.driven'",
    )
    .bind(floor.environment)
    .fetch_all(floor.scene.rig.db.pool())
    .await
    .unwrap();
    assert!(details.iter().any(|row| {
        row["hosts"]
            .as_array()
            .is_some_and(|hosts| hosts.iter().any(|host| host == "127.0.0.1"))
    }));
    let audit: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE environment_id=$1 AND action LIKE 'browser.%'",
    )
    .bind(floor.environment)
    .fetch_all(floor.scene.rig.db.pool())
    .await
    .unwrap();
    let kept = serde_json::to_string(&audit).unwrap();
    for private in [url, "private-path", "private-first-address-marker", note] {
        assert!(
            !kept.contains(private),
            "browser history must omit {private}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_manage_taken_back_while_a_body_arrives_prevents_profile_creation() {
    use axum::body::Body;
    use bytes::Bytes;
    use futures::stream;
    use http::{Request, StatusCode};
    use orch_api_core::registry::Registry;
    use tower::ServiceExt;

    let Some(floor) = Floor::new().await else {
        return;
    };
    let entry = floor.names(&floor.scene.bea, "browser.manage").await;
    let pool = floor.scene.rig.db.pool().clone();
    let owner = floor.scene.ada.id();
    let now = floor.app.clock.now();
    // The body is polled after the initial declared-permission check.
    let body = Body::from_stream(stream::once(async move {
        let mut conn = pool.acquire().await.unwrap();
        orch_db::queries::access::remove_entry(&mut conn, entry, owner, now)
            .await
            .unwrap();
        orch_db::queries::access::bump_epoch(&mut conn)
            .await
            .unwrap();
        Ok::<_, std::io::Error>(Bytes::from_static(br#"{"name":"after-body"}"#))
    }));
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let router = registry.into_router().with_state(floor.app.clone());
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/environments/{}/browser-profiles",
                    floor.environment
                ))
                .header("authorization", format!("Bearer {}", floor.scene.bea.token))
                .header("content-type", "application/json")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        !floor
            .peer
            .dir
            .path()
            .join("home/.orch/browsers/after-body")
            .exists()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_is_joined_and_the_endpoint_is_only_shown_to_a_browser_manager() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    let browser = floor.open(&floor.scene.ada, "team").await;
    let joined = floor.open(&floor.scene.dan, "team").await;
    assert_eq!(browser.id, joined.id);
    let list = floor
        .scene
        .rig
        .world
        .app
        .call(EnvironmentBrowserList, &[&floor.environment])
        .token(&floor.scene.cai.token)
        .query(&EnvironmentBrowserListQuery { look: None })
        .send()
        .await
        .ok();
    assert_eq!(list.browsers[0].endpoint, None);
    assert_eq!(list.profiles_dir, None);
    assert_eq!(list.limit.open, 1);
    let used = floor
        .scene
        .rig
        .world
        .app
        .call(BrowserProfileDelete, &[&floor.environment, &"team"])
        .token(&floor.scene.ada.token)
        .query(&BrowserProfileDeleteQuery {
            confirm: "team".to_owned(),
        })
        .send()
        .await;
    assert_eq!(used.status, 409);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_spent_ticket_cannot_return_and_permissions_taken_back_apply_to_a_live_socket() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    let browser = floor.open(&floor.scene.ada, "permissions").await;
    let view = floor.names(&floor.scene.bea, "browser.view").await;
    let manage = floor.names(&floor.scene.bea, "browser.manage").await;
    let ticket = floor.ticket(&floor.scene.bea, browser.id, true).await;
    let mut screen = floor.screen(browser.id, &ticket.ticket).await;
    assert_eq!(screen.text("ready").await["control"], true);
    floor
        .a_spent_ticket_is_refused(browser.id, &ticket.ticket)
        .await;
    floor.removes(manage).await;
    assert_eq!(screen.text("ready").await["control"], false);
    screen
        .send(json!({"op":"text","tab":"none","text":"should-not-arrive"}))
        .await;
    assert_eq!(screen.text("error").await["code"], "view_only");
    assert_eq!(
        floor
            .peer
            .observe(browser.id, "document.getElementById('i').value"),
        ""
    );
    floor.removes(view).await;
    assert_eq!(screen.text("closed").await["reason"], "revoked");
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_control_alone_cannot_navigate_to_a_file_or_answer_a_file_chooser() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    let browser = floor.open(&floor.scene.ada, "files").await;
    floor.names(&floor.scene.bea, "browser.manage").await;
    let ticket = floor.ticket(&floor.scene.bea, browser.id, true).await;
    let mut screen = floor.screen(browser.id, &ticket.ticket).await;
    let tab = screen.text("ready").await["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for frame in [
        json!({"op":"navigate","tab":tab,"url":"file:///peer/workspace/file.txt"}),
        json!({"op":"files","tab":tab,"paths":["file.txt"]}),
    ] {
        screen.send(frame).await;
        assert_eq!(screen.text("error").await["code"], "not_allowed");
    }
    assert_eq!(floor.peer.observe(browser.id, "document.title"), "api-page");
    assert_eq!(
        floor
            .peer
            .observe(browser.id, "document.getElementById('f').files.length"),
        0
    );
    floor.names(&floor.scene.bea, "files.view").await;
    screen
        .send(json!({"op":"navigate","tab":tab,"url":"file:///peer/workspace/file.txt"}))
        .await;
    tokio::time::timeout(browsers_support::WAIT, async {
        while floor
            .peer
            .observe(browser.id, "document.body.innerText")
            .as_str()
            != Some("workspace file")
        {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn fake_browser_directives_join_across_threads_name_the_driver_and_close_only_their_own() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    let a = floor.scene.thread_by(&floor.scene.ada, "Browser one").await;
    let b = floor.scene.thread_by(&floor.scene.ada, "Browser two").await;
    let mut first = browsers_support::fake::Agent::start(&floor, a.base.id).await;
    let mut second = browsers_support::fake::Agent::start(&floor, b.base.id).await;
    let opened = first.run(&floor, "browser open agents").await;
    assert!(opened.error.is_none(), "{:?}", opened.error);
    let opened = opened.result.unwrap();
    let joined = second
        .run(&floor, "browser open agents")
        .await
        .result
        .unwrap();
    assert_eq!(opened["browser"], joined["browser"]);
    assert_eq!(opened["started"], true);
    assert_eq!(joined["started"], false);
    assert!(
        opened["attach"]
            .as_str()
            .unwrap()
            .starts_with("playwright-cli attach --cdp=")
    );
    let close = format!("browser close {}", opened["browser"].as_str().unwrap());
    assert_eq!(
        second.run(&floor, &close).await.error.unwrap().code,
        "not_yours"
    );
    let list = first.run(&floor, "browser list").await.result.unwrap();
    assert_eq!(list["browsers"][0]["mine"], true);
    assert_eq!(
        first.run(&floor, &close).await.result.unwrap()["closed"],
        true
    );
    let default = first.run(&floor, "browser open").await.result.unwrap();
    assert_eq!(default["profile"], format!("thread-{}", a.base.id.simple()));
    let nonlocal = first
        .run(&floor, "browser announce http://192.0.2.1:41821")
        .await
        .error
        .unwrap();
    assert_eq!(nonlocal.code, "not_local");
    let absent = first
        .run(&floor, "browser announce http://127.0.0.1:41821")
        .await
        .error
        .unwrap();
    assert_eq!(absent.code, "not_reachable");
    assert!(absent.message.contains("--remote-debugging-port=0"));
    first
        .run(
            &floor,
            &format!("browser close {}", default["browser"].as_str().unwrap()),
        )
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn viewers_share_one_tunnel_get_their_own_tabs_and_a_slow_viewer_gets_only_the_latest_picture()
 {
    use orch_realtime_contract::browser::PictureHeader;
    let Some(floor) = Floor::new().await else {
        return;
    };
    let browser = floor.open(&floor.scene.ada, "pictures").await;
    let ticket = floor.ticket(&floor.scene.ada, browser.id, true).await;
    let mut fast = floor.screen(browser.id, &ticket.ticket).await;
    let tab = fast.text("ready").await["tabs"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ticket = floor.ticket(&floor.scene.cai, browser.id, false).await;
    let mut slow = floor.screen(browser.id, &ticket.ticket).await;
    slow.text("ready").await;
    fast.send(json!({"op":"watch","tab":tab,"width":400,"height":300,"dpr":1}))
        .await;
    slow.send(json!({"op":"watch","tab":tab,"width":900,"height":600,"dpr":1}))
        .await;
    floor.peer.observe(browser.id,"window.pictureTick=setInterval(()=>{document.body.style.background=`rgb(${Math.floor(Math.random()*255)},20,20)`},100);0");
    let behind = slow.picture().await;
    let behind_n = PictureHeader::decode(&behind).unwrap().n;
    let mut latest = 0;
    let mut largest = 0;
    for _ in 0..8 {
        let bytes = fast.picture().await;
        let header = PictureHeader::decode(&bytes).unwrap();
        latest = header.n;
        largest = largest.max(header.image.width);
        fast.drawn(&bytes).await;
    }
    assert!(
        largest >= 900,
        "the larger panel determines the shared picture size"
    );
    assert!(
        latest > behind_n + 1,
        "the fast viewer keeps receiving pictures without the other one's ACK"
    );
    slow.drawn(&behind).await;
    let caught_up = slow.picture().await;
    let caught_up_n = PictureHeader::decode(&caught_up).unwrap().n;
    assert!(
        caught_up_n >= latest,
        "the resumed viewer gets the current picture"
    );
    slow.drawn(&caught_up).await;
    fast.send(json!({"op":"tab","do":"new","url":"data:text/html,<title>tab-two</title><body style=background:blue>another-tab"})).await;
    let other = loop {
        let tabs = fast.text("tabs").await;
        if let Some(tab) = tabs["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["title"] == "tab-two")
        {
            break tab["id"].as_str().unwrap().to_owned();
        }
    };
    let ticket = floor.ticket(&floor.scene.dan, browser.id, false).await;
    let mut third = floor.screen(browser.id, &ticket.ticket).await;
    third.text("ready").await;
    third
        .send(json!({"op":"watch","tab":other,"width":700,"height":500,"dpr":1}))
        .await;
    let picture = third.picture().await;
    assert_ne!(
        orch_types::Hash::of(&picture[12..]),
        orch_types::Hash::of(&caught_up[12..])
    );
    third.drawn(&picture).await;
    assert_eq!(floor.peer.rpc(json!({"tunnels":true}))["ok"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn announced_cli_browsers_do_not_count_toward_the_limit_and_each_gate_disables_the_tool() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    let thread = floor.scene.thread().await;
    let mut agent = browsers_support::fake::Agent::start(&floor, thread.base.id).await;
    let no_port = floor.peer.rpc(json!({"launch_cli":false}));
    assert_eq!(no_port["ok"]["endpoint"], serde_json::Value::Null);
    let refusal = agent
        .run(&floor, "browser announce http://127.0.0.1:41821")
        .await
        .error
        .unwrap();
    assert_eq!(refusal.code, "not_reachable");
    assert!(refusal.message.contains("--remote-debugging-port=0"));
    let started = floor.peer.rpc(json!({"launch_cli":true}));
    let endpoint = started["ok"]["endpoint"].as_str().unwrap();
    let announced = agent
        .run(&floor, &format!("browser announce {endpoint}"))
        .await;
    assert!(announced.error.is_none(), "{:?}", announced.error);
    let announced = announced.result.unwrap();
    assert_eq!(announced["profile"], "cli-announced");
    let repeated = agent
        .run(&floor, &format!("browser announce {endpoint}"))
        .await
        .result
        .unwrap();
    assert_eq!(repeated["browser"], announced["browser"]);
    floor
        .scene
        .rig
        .world
        .set("browser.max_per_environment", json!(1))
        .await;
    let own = agent
        .run(&floor, "browser open owned")
        .await
        .result
        .unwrap();
    let limit = agent
        .run(&floor, "browser open too-many")
        .await
        .error
        .unwrap();
    assert_eq!(limit.code, "too_many");
    assert!(limit.message.contains(own["browser"].as_str().unwrap()));
    let joined = agent
        .run(&floor, "browser open cli-announced")
        .await
        .result
        .unwrap();
    assert_eq!(joined["browser"], announced["browser"]);
    assert_eq!(joined["started"], false);
    agent
        .run(
            &floor,
            &format!("browser close {}", own["browser"].as_str().unwrap()),
        )
        .await;
    for off in [
        "disabled_by_policy",
        "environment_switch_off",
        "machine_policy",
    ] {
        match off {
            "disabled_by_policy" => {
                floor
                    .scene
                    .rig
                    .world
                    .set("browser.enabled", json!(false))
                    .await
            }
            "environment_switch_off" => {
                sqlx::query("UPDATE environments SET browser=false WHERE id=$1")
                    .bind(floor.environment)
                    .execute(floor.scene.rig.db.pool())
                    .await
                    .unwrap();
            }
            _ => {
                sqlx::query("UPDATE environments SET placement='connected' WHERE id=$1")
                    .bind(floor.environment)
                    .execute(floor.scene.rig.db.pool())
                    .await
                    .unwrap();
                sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy}',jsonb_build_object('environment.browser',false),true) WHERE id=$1").bind(floor.scene.rig.machine_id).execute(floor.scene.rig.db.pool()).await.unwrap();
            }
        }
        assert_eq!(
            agent
                .run(&floor, "browser open disabled")
                .await
                .error
                .unwrap()
                .code,
            "disabled"
        );
        let list = agent.run(&floor, "browser list").await.result.unwrap();
        assert_eq!(list["on"], false);
        assert_eq!(list["why_off"], off);
        let mut conn = floor.scene.rig.db.acquire().await.unwrap();
        assert!(
            !orch_engine::tools::browser::offered(&mut conn, Some(floor.environment))
                .await
                .unwrap()
        );
        drop(conn);
        floor
            .scene
            .rig
            .world
            .set("browser.enabled", json!(true))
            .await;
        sqlx::query("UPDATE environments SET browser=true,placement='managed' WHERE id=$1")
            .bind(floor.environment)
            .execute(floor.scene.rig.db.pool())
            .await
            .unwrap();
    }
    agent
        .run(
            &floor,
            &format!("browser close {}", announced["browser"].as_str().unwrap()),
        )
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_idle_rule_is_optional_and_a_stopped_or_new_generation_environment_closes_its_rows() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    let browser = floor.open(&floor.scene.ada, "idle").await;
    floor.scene.rig.world.advance(chrono::Duration::hours(1));
    floor.scene.rig.engine.browsers().sweep().await.unwrap();
    let state: String = sqlx::query_scalar("SELECT state FROM browser_sessions WHERE id=$1")
        .bind(browser.id)
        .fetch_one(floor.scene.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(state, "open");
    floor
        .scene
        .rig
        .world
        .set("browser.idle_minutes", json!(1))
        .await;
    floor.scene.rig.world.advance(chrono::Duration::minutes(2));
    floor.scene.rig.engine.browsers().sweep().await.unwrap();
    let reason: String =
        sqlx::query_scalar("SELECT close_reason FROM browser_sessions WHERE id=$1")
            .bind(browser.id)
            .fetch_one(floor.scene.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(reason, "idle");
    floor
        .scene
        .rig
        .world
        .set("browser.idle_minutes", json!(0))
        .await;
    let browser = floor
        .scene
        .rig
        .world
        .app
        .call(EnvironmentBrowserCreate, &[&floor.environment])
        .token(&floor.scene.ada.token)
        .body(&EnvironmentBrowserCreateBody {
            profile: "idle".to_owned(),
            create: Some(true),
            preset: Some(BrowserPreset::Window),
            url: None,
        })
        .send()
        .await
        .ok();
    let display = browser.display.expect("a window has its display");
    sqlx::query("UPDATE environments SET generation=generation+1 WHERE id=$1")
        .bind(floor.environment)
        .execute(floor.scene.rig.db.pool())
        .await
        .unwrap();
    floor.scene.rig.engine.browsers().sweep().await.unwrap();
    let reason: String =
        sqlx::query_scalar("SELECT close_reason FROM browser_sessions WHERE id=$1")
            .bind(browser.id)
            .fetch_one(floor.scene.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(reason, "environment_stopped");
    let display_state: (String, String) =
        sqlx::query_as("SELECT state,close_reason FROM display_sessions WHERE id=$1")
            .bind(display)
            .fetch_one(floor.scene.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(
        display_state,
        ("closed".to_owned(), "environment_stopped".to_owned())
    );
    let profiles = floor.peer.dir.path().join("home/.orch/browsers/idle");
    assert!(profiles.is_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_is_kept_when_its_unit_stops_starts_and_is_recreated() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    let first = floor.open(&floor.scene.ada, "kept").await;
    tokio::time::timeout(browsers_support::WAIT, async {
        loop {
            let cookies = floor.peer.rpc(json!({"cookies":first.id}));
            if cookies["ok"]["cookies"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["name"] == "login" && c["value"] == "kept")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    let mut current = first;
    for (generation, recreate) in [(1, false), (2, true)] {
        sqlx::query("UPDATE environments SET status='stopped' WHERE id=$1")
            .bind(floor.environment)
            .execute(floor.scene.rig.db.pool())
            .await
            .unwrap();
        floor.scene.rig.engine.browsers().sweep().await.unwrap();
        let reason: String =
            sqlx::query_scalar("SELECT close_reason FROM browser_sessions WHERE id=$1")
                .bind(current.id)
                .fetch_one(floor.scene.rig.db.pool())
                .await
                .unwrap();
        assert_eq!(reason, "environment_stopped");
        let detached = floor
            .peer
            .rpc(json!({"command":{"k":"env.detach","b":{"environment_id":floor.environment}}}));
        assert!(detached.get("ok").is_some());
        floor
            .peer
            .cycle(floor.environment, generation, recreate)
            .await;
        sqlx::query("UPDATE environments SET status='running',generation=$2 WHERE id=$1")
            .bind(floor.environment)
            .bind(generation)
            .execute(floor.scene.rig.db.pool())
            .await
            .unwrap();
        floor
            .scene
            .rig
            .engine
            .browsers()
            .reconcile(floor.environment)
            .await
            .unwrap();
        current = floor
            .scene
            .rig
            .world
            .app
            .call(EnvironmentBrowserCreate, &[&floor.environment])
            .token(&floor.scene.ada.token)
            .body(&EnvironmentBrowserCreateBody {
                profile: "kept".to_owned(),
                create: Some(false),
                preset: None,
                url: None,
            })
            .send()
            .await
            .ok();
        let cookies = floor.peer.rpc(json!({"cookies":current.id}));
        assert!(
            cookies["ok"]["cookies"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["name"] == "login" && c["value"] == "kept"),
            "the cookie must exist before any new visit"
        );
        assert!(
            floor
                .peer
                .dir
                .path()
                .join("home/.orch/browsers/kept")
                .is_dir()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn everything_a_view_only_socket_sends_is_dropped_and_never_reaches_the_page() {
    let Some(floor) = Floor::new().await else {
        return;
    };
    let browser = floor.open(&floor.scene.ada, "watch").await;
    let ticket = floor.ticket(&floor.scene.cai, browser.id, false).await;
    let mut screen = floor.screen(browser.id, &ticket.ticket).await;
    let ready = screen.text("ready").await;
    assert_eq!(ready["control"], false);
    let tab = ready["tabs"][0]["id"].as_str().unwrap().to_owned();
    screen
        .send(json!({"op":"watch","tab":tab,"width":640,"height":400,"dpr":1}))
        .await;
    let frames = vec![
        json!({"op":"pointer","tab":tab,"kind":"down","x":10,"y":10,"button":"left"}),
        json!({"op":"key","tab":tab,"kind":"down","key":"a","code":"KeyA","text":"a"}),
        json!({"op":"text","tab":tab,"text":"private-input-marker"}),
        json!({"op":"navigate","tab":tab,"url":"data:text/html,<title>forbidden</title>"}),
        json!({"op":"history","tab":tab,"to":"back"}),
        json!({"op":"tab","do":"new","url":"about:blank"}),
        json!({"op":"dialog","tab":tab,"accept":true}),
        json!({"op":"files","tab":tab,"paths":["file.txt"]}),
        json!({"op":"viewport","width":900,"height":700}),
        json!({"op":"copy","tab":tab}),
    ];
    for frame in frames {
        screen.send(frame).await;
        assert_eq!(screen.text("error").await["code"], "view_only");
        assert_eq!(floor.peer.observe(browser.id, "window.actions"), 0);
        assert_eq!(floor.peer.observe(browser.id, "document.title"), "api-page");
        assert_eq!(
            floor
                .peer
                .observe(browser.id, "document.getElementById('i').value"),
            ""
        );
    }
    let second = floor.ticket(&floor.scene.ada, browser.id, true).await;
    let mut controller = floor.screen(browser.id, &second.ticket).await;
    let ready = controller.text("ready").await;
    let tab = ready["tabs"][0]["id"].as_str().unwrap();
    controller
        .send(json!({"op":"watch","tab":tab,"width":800,"height":600,"dpr":1}))
        .await;
    controller
        .send(json!({"op":"text","tab":tab,"text":"private-input-marker"}))
        .await;
    tokio::time::timeout(browsers_support::WAIT, async {
        loop {
            if floor
                .peer
                .observe(browser.id, "document.getElementById('i').value")
                == "private-input-marker"
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    controller.send(json!({"op":"navigate","tab":tab,"url":"http://127.0.0.1:41820/?private-address-marker"})).await;
    tokio::time::timeout(browsers_support::WAIT, async {
        while floor.peer.observe(browser.id, "location.search") != "?private-address-marker" {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    controller.send(json!({"op":"navigate","tab":tab,"url":"http://localhost:41820/?private-second-address-marker"})).await;
    tokio::time::timeout(browsers_support::WAIT, async {
        while floor.peer.observe(browser.id, "location.hostname") != "localhost" {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    let picture = controller.picture().await;
    controller.drawn(&picture).await;
    let picture_marker = hex::encode(&picture[12..28]);
    floor.scene.rig.world.advance(chrono::Duration::minutes(6));
    // Wait for the actual summary row, not for a collector pass to fit into two seconds.
    crate::rig::eventually(
        "the browser control summary to retain both public host names",
        || async {
            let details: Vec<serde_json::Value> = sqlx::query_scalar(
                "SELECT details FROM audit_log WHERE environment_id=$1 AND action='browser.driven'",
            )
            .bind(floor.environment)
            .fetch_all(floor.scene.rig.db.pool())
            .await
            .unwrap();
            ["127.0.0.1", "localhost"].iter().all(|host| {
                details.iter().any(|detail| {
                    detail["hosts"].as_array().is_some_and(|hosts| {
                        hosts.iter().any(|value| value.as_str() == Some(*host))
                    })
                })
            })
        },
    )
    .await;
    let details: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT details FROM audit_log WHERE environment_id=$1 AND action='browser.driven'",
    )
    .bind(floor.environment)
    .fetch_all(floor.scene.rig.db.pool())
    .await
    .unwrap();
    assert!(details.iter().any(|d| {
        d["hosts"]
            .as_array()
            .is_some_and(|h| h.iter().any(|v| v == "127.0.0.1"))
    }));
    assert!(details.iter().any(|d| {
        d["hosts"]
            .as_array()
            .is_some_and(|hosts| hosts.iter().any(|host| host == "localhost"))
    }));
    assert!(
        !serde_json::to_string(&details)
            .unwrap()
            .contains("private-input-marker")
    );
    assert!(!serde_json::to_string(&details).unwrap().contains("http://"));
    let tables:Vec<String>=sqlx::query_scalar("SELECT table_name FROM information_schema.tables WHERE table_schema='public' AND table_type='BASE TABLE'").fetch_all(floor.scene.rig.db.pool()).await.unwrap();
    for table in tables {
        let table = table.replace('"', "\"\"");
        let query =
            format!("SELECT EXISTS(SELECT 1 FROM \"{table}\" t WHERE to_jsonb(t)::text LIKE $1)");
        for marker in [
            "private-input-marker",
            "private-address-marker",
            "private-second-address-marker",
            picture_marker.as_str(),
        ] {
            // The identifier comes from this fresh database's catalogue and is double-quoted with every embedded quote escaped.
            let kept: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(query.as_str()))
                .bind(format!("%{marker}%"))
                .fetch_one(floor.scene.rig.db.pool())
                .await
                .unwrap();
            assert!(!kept, "page material was kept in {table}");
        }
    }
}
