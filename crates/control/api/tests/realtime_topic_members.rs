//! A topic's socket carries the HTTP projection's threads, jobs and announcements for each reader.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use orch_api_contract::access::{PageChange, PageChangeWhole, PermissionPageUpdate};
use orch_api_contract::topics::{TopicSyncGet, TopicSyncGetQuery};
use orch_api_core::{registry::Registry, testing::TestApp};
use orch_db::queries::{feeds, hierarchy, jobs, threads::tools};
use orch_types::{Id, new_id};
use realtime_support::{Client, Rig};
use serde_json::{Value, json};

async fn next_sync(client: &mut Client) -> Value {
    loop {
        let frame = client.frame().await;
        if frame["op"] == "sync" {
            return frame;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_topic_socket_and_http_show_the_same_members_and_leave_out_a_private_threads_job() {
    let rig = Rig::with_time_held().await;
    let public = rig.thread("Public work").await;
    let secret = rig.thread("Private work").await;
    let (bea, bea_token) = rig.person("bea").await;
    rig.give(bea, "viewer").await;
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::over(
        rig.server.app.clone(),
        orch_testkit::clock::manual(),
        registry,
    );
    app.call(PermissionPageUpdate, &[&"nodes", &secret])
        .token(&rig.token)
        .body(&PageChange {
            whole: Some(PageChangeWhole::Private),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    rig.server.engine.stop();
    let before = feeds::topic_seqs(&mut rig.pool.acquire().await.unwrap(), &[rig.topic])
        .await
        .unwrap()[0]
        .1;
    let mut ada = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut bea = Client::signed_in(rig.server.addr, &bea_token).await;
    for client in [&mut ada, &mut bea] {
        client
            .sub(1, &format!("topic:{}", rig.topic), Some(before))
            .await;
        client.expect("subscribed").await;
    }
    let mut tx = rig.pool.begin().await.unwrap();
    let mut made = Vec::<Id>::new();
    for thread in [public, secret] {
        let job = jobs::insert(
            &mut tx,
            &jobs::NewJob {
                id: new_id(),
                kind: jobs::forks::THREAD_FORK,
                node_id: Some(thread),
                environment_id: None,
                provider_id: None,
                plan: &json!({}),
                progress: &json!({}),
                created_by: Some(rig.ada),
            },
            rig.clock.now(),
        )
        .await
        .unwrap();
        made.push(job.id);
    }
    tools::insert_announcement(
        &mut tx,
        &tools::NewAnnouncement {
            id: new_id(),
            topic_id: rig.topic,
            from_thread_id: None,
            from_user_id: Some(rig.ada),
            category: "notice",
            title: "Ready",
            body: "The public release is ready",
            files: &json!([]),
            severity: "info",
            delivery: "next_turn",
            fallback: "next_turn",
            wake_idle: false,
            targets: "all",
            to_thread_id: None,
            group_id: None,
            at: rig.clock.now(),
        },
    )
    .await
    .unwrap();
    hierarchy::stamp(&mut tx, rig.topic).await.unwrap();
    tx.commit().await.unwrap();
    for (client, token, may_read_secret) in
        [(&mut ada, &rig.token, true), (&mut bea, &bea_token, false)]
    {
        let frame = next_sync(client).await;
        assert_eq!(
            frame["prev"], before,
            "the first projection keeps every committed member since subscribing"
        );
        let http = app
            .call(TopicSyncGet, &[&rig.topic])
            .token(token)
            .query(&TopicSyncGetQuery {
                since: Some(before),
            })
            .send()
            .await
            .ok();
        assert_eq!(frame["seq"], http.seq);
        assert_eq!(
            frame["data"],
            json!({"topic": http.topic, "threads": http.threads, "jobs": http.jobs, "announcements": http.announcements})
        );
        assert_eq!(frame["data"]["announcements"][0]["title"], "Ready");
        assert!(
            frame["data"]["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|job| job["id"] == made[0].to_string())
        );
        assert_eq!(
            frame["data"]["jobs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|job| job["id"] == made[1].to_string()),
            may_read_secret
        );
        assert_eq!(
            frame["data"]["threads"]
                .as_array()
                .unwrap()
                .iter()
                .any(|thread| thread["id"] == secret.to_string()),
            may_read_secret
        );
    }
    rig.server.stop().await;
}
