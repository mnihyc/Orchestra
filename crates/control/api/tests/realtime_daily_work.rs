//! Approvals, questions, delivered inputs and renamed or newly created threads reach an already open page over its real socket.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use orch_api_contract::binding::EnvironmentChoice;
use orch_api_contract::nodes::{
    CreateThread, CreateThreadBinding, NodeGet, NodeUpdate, ThreadCreate,
};
use orch_api_core::{registry::Registry, testing::TestApp};
use orch_db::queries::hierarchy;
use orch_link_contract::events::{ApprovalRequested, Event};
use orch_testkit::clock;
use orch_types::{Id, Json};
use realtime_support::{Client, Heard, MODEL, PATIENCE, Rig, Server, quick};
use serde_json::json;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

struct Page {
    seq: i64,
    thread: Json,
    items: HashMap<Id, Json>,
    inputs: HashMap<Id, Json>,
    nodes: HashMap<Id, Json>,
}
impl Page {
    fn at(seq: i64) -> Self {
        Self {
            seq,
            thread: Json::Null,
            items: HashMap::new(),
            inputs: HashMap::new(),
            nodes: HashMap::new(),
        }
    }
    async fn follow(&mut self, client: &mut Client, done: impl Fn(&Self) -> bool) {
        let began = Instant::now();
        while !done(self) {
            assert!(
                began.elapsed() < PATIENCE,
                "the open page missed live facts after {}",
                self.seq
            );
            let Some(Heard::Frame(frame)) = client.hear_within(Duration::from_millis(200)).await
            else {
                continue;
            };
            assert!(
                !matches!(frame["op"].as_str(), Some("resync" | "closed" | "error")),
                "{frame}"
            );
            if frame["op"] != "sync" {
                continue;
            }
            assert_eq!(
                frame["prev"], self.seq,
                "the page must not need a repair read: {frame}"
            );
            self.seq = frame["seq"].as_i64().unwrap();
            let data = &frame["data"];
            if data["thread"].is_object() {
                self.thread = data["thread"].clone();
            }
            for (name, held) in [
                ("items", &mut self.items),
                ("inputs", &mut self.inputs),
                ("nodes", &mut self.nodes),
            ] {
                for row in data[name].as_array().into_iter().flatten() {
                    held.insert(row["id"].as_str().unwrap().parse().unwrap(), row.clone());
                }
            }
        }
    }
    fn approval(&self) -> Option<&Json> {
        self.items.values().find(|item| item["kind"] == "approval")
    }
}

async fn watching(rig: &Rig, server: &Server, thread: Id) -> (Client, Page) {
    let before = rig.rows(thread).await;
    let mut client = Client::signed_in(server.addr, &rig.token).await;
    client
        .sub(1, &format!("thread:{thread}"), Some(before.seq))
        .await;
    client.expect("subscribed").await;
    client.expect("presence").await;
    (client, Page::at(before.seq))
}

async fn a_request_is_live(question: bool) {
    let rig = Rig::new().await;
    // The other API process has no local callback from the writer. Delivery must use the committed feed.
    let other = Server::over(
        &rig.pool,
        &rig.clock,
        quick(&orch_config::ServerConfig::default()),
    )
    .await;
    let thread = rig.thread("A live decision").await;
    let turn = rig.send(thread, "Ask a person").await;
    let (mut local, mut page) = watching(&rig, &rig.server, thread).await;
    let (mut remote, mut second) = watching(&rig, &other, thread).await;
    rig.begin_message(thread, &turn, "request-result").await;
    let request = if question {
        json!({"request_kind":"user_input","options":[],"detail":{"questions":[
            {"id":"scope","header":"Scope","question":"Which scope?","multiple":false,"options":null,"allow_other":true}
        ]}})
    } else {
        json!({"request_kind":"command","detail":{"command":"./deploy staging","cwd":"/workspace","reason":null},
        "options":[{"id":"allow_once","label":"Allow once","style":"allow","scope":"once"}]})
    };
    rig.say(
        thread,
        Event::ApprovalRequested(ApprovalRequested {
            turn_id: turn.turn_id,
            request_ref: "live-request".into(),
            title: if question {
                "Choose the scope"
            } else {
                "Allow deployment"
            }
            .into(),
            request: serde_json::from_value(request).unwrap(),
        }),
    )
    .await;
    for (client, view) in [(&mut local, &mut page), (&mut remote, &mut second)] {
        view.follow(client, |view| {
            view.thread["state"] == "waiting_approval"
                && view.thread["pending_approvals"] == 1
                && view.approval().is_some_and(|item| {
                    item["status"] == "pending"
                        && item["payload"]["approval"]["state"] == "pending"
                        && item["payload"]["approval"]["can_decide"] == true
                })
        })
        .await;
        assert_eq!(
            view.approval().unwrap()["payload"]["approval"]["request"]["request_kind"],
            if question { "user_input" } else { "command" }
        );
        assert!(
            view.thread["can"]["export"].is_boolean(),
            "a live thread is a complete authorized view"
        );
    }
    let id = page.approval().unwrap()["payload"]["approval_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let answer = if question {
        json!({"answers":{"scope":["Login"]},"note":null})
    } else {
        json!({"option_id":"allow_once","note":null})
    };
    let response = reqwest::Client::new()
        .post(format!(
            "http://{}/api/v1/approvals/{id}/decision",
            rig.server.addr
        ))
        .bearer_auth(&rig.token)
        .json(&answer)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    // A question is answered, a request for an action approved: the item and its approval say which.
    let settled = if question { "answered" } else { "approved" };
    for (client, view) in [(&mut local, &mut page), (&mut remote, &mut second)] {
        view.follow(client, |view| {
            view.thread["state"] == "running"
                && view.thread["pending_approvals"] == 0
                && view.approval().is_some_and(|item| {
                    item["status"] == settled
                        && item["payload"]["approval"]["state"] == settled
                        && !item["payload"]["approval"]["decided_by"].is_null()
                })
        })
        .await;
        if question {
            assert_eq!(
                view.approval().unwrap()["payload"]["approval"]["decision"]["answers"]["scope"],
                json!(["Login"])
            );
        }
        assert!(
            view.thread["can"]["export"].is_boolean(),
            "deciding never replaces the thread with a thin reference"
        );
    }
    other.stop().await;
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_approval_appears_when_created_and_settles_when_decided_on_both_open_pages() {
    a_request_is_live(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_question_appears_when_asked_and_settles_with_its_answers_on_both_open_pages() {
    a_request_is_live(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delivered_input_leaves_the_sending_queue_on_the_live_feed() {
    let rig = Rig::new().await;
    let thread = rig.thread("Delivery leaves the queue").await;
    let turn = rig.send(thread, "Delivered words").await;
    let before = rig.rows(thread).await;
    let input = before
        .inputs
        .iter()
        .find(|input| input.turn_id == Some(turn.turn_id))
        .unwrap();
    assert_eq!(serde_json::to_value(input).unwrap()["state"], "delivering");
    let id = input.id;
    let (mut client, mut page) = watching(&rig, &rig.server, thread).await;
    rig.begin_message(thread, &turn, "delivered-result").await;
    page.follow(&mut client, |page| {
        page.inputs
            .get(&id)
            .is_some_and(|input| input["state"] == "delivered" && !input["delivered_at"].is_null())
            && page.items.values().any(|item| {
                item["kind"] == "user_message" && item["payload"]["input_id"] == id.to_string()
            })
    })
    .await;
    assert_eq!(page.inputs[&id]["state_detail"], Json::Null);
    rig.server.stop().await;
}

fn app(rig: &Rig) -> TestApp {
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    TestApp::over(rig.server.app.clone(), clock::manual(), registry)
}

async fn watching_tree(rig: &Rig) -> (Client, Page) {
    let seq = hierarchy::tree_seq(&mut rig.pool.acquire().await.unwrap(), rig.root)
        .await
        .unwrap();
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &format!("org:{}", rig.root), Some(seq)).await;
    client.expect("subscribed").await;
    (client, Page::at(seq))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_rename_updates_the_existing_tree_without_another_navigation() {
    let rig = Rig::new().await;
    let thread = rig.thread("The old name").await;
    let app = app(&rig);
    let before = app
        .call(NodeGet, &[&thread])
        .token(&rig.token)
        .send()
        .await
        .ok();
    let revision = serde_json::to_value(before).unwrap()["revision"]
        .as_i64()
        .unwrap();
    let (mut client, mut page) = watching_tree(&rig).await;
    app.call(NodeUpdate, &[&thread])
        .token(&rig.token)
        .raw_body(&json!({"name":"The new name","expected_revision":revision}))
        .send()
        .await
        .ok();
    page.follow(&mut client, |page| {
        page.nodes
            .get(&thread)
            .is_some_and(|row| row["name"] == "The new name")
    })
    .await;
    assert_eq!(page.nodes[&thread]["parent_id"], rig.topic.to_string());
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_thread_enters_the_existing_tree_from_its_create_route() {
    let rig = Rig::new().await;
    let (mut client, mut page) = watching_tree(&rig).await;
    let made = app(&rig)
        .call(ThreadCreate, &[&rig.topic])
        .token(&rig.token)
        .body(&CreateThread {
            name: Some("The new thread".into()),
            binding: Some(CreateThreadBinding {
                agent_id: rig.agent,
                provider_id: rig.provider,
                arrangement: None,
            }),
            model: Some(MODEL.into()),
            environment: Some(EnvironmentChoice::Use(rig.environment)),
            directory: Some("web".into()),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok();
    page.follow(&mut client, |page| {
        page.nodes.get(&made.base.id).is_some_and(|row| {
            row["name"] == "The new thread" && row["parent_id"] == rig.topic.to_string()
        })
    })
    .await;
    assert_eq!(page.nodes[&made.base.id]["kind"], "thread");
    rig.server.stop().await;
}
