//! An environment's live feed tells its readers what its HTTP state tells them, from the number they hold, and nobody else.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use chrono::Duration as Span;
use orch_api_contract::environment::{EnvironmentSyncGet, EnvironmentSyncGetQuery};
use orch_api_core::{registry::Registry, testing::TestApp};
use orch_db::queries::identity::testing::insert_user;
use orch_db::queries::{feeds, jobs};
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

async fn env_seq(rig: &Rig) -> i64 {
    feeds::environment_seqs(&mut rig.pool.acquire().await.unwrap(), &[rig.environment])
        .await
        .unwrap()[0]
        .1
}

/// A job about the environment, as the engine writes one when somebody asks for a recreation.
async fn recreation_asked(rig: &Rig) -> Id {
    let mut tx = rig.pool.begin().await.unwrap();
    let job = jobs::insert(
        &mut tx,
        &jobs::NewJob {
            id: new_id(),
            kind: jobs::kind::ENVIRONMENT_RECREATE,
            node_id: None,
            environment_id: Some(rig.environment),
            provider_id: None,
            plan: &json!({}),
            progress: &json!({}),
            created_by: Some(rig.ada),
        },
        rig.clock.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    job.id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_job_about_an_environment_reaches_its_reader_as_its_http_state_says_and_a_late_reader_catches_up()
 {
    let rig = Rig::with_unstarted_engine_and_time_held().await;
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::over(
        rig.server.app.clone(),
        orch_testkit::clock::manual(),
        registry,
    );
    // Nothing takes the job up: what the feed says of it is what was asked.
    let before = env_seq(&rig).await;
    let feed = format!("env:{}", rig.environment);
    let mut ada = Client::signed_in(rig.server.addr, &rig.token).await;
    ada.sub(1, &feed, Some(before)).await;
    let subscribed = ada.expect("subscribed").await;
    assert_eq!(subscribed["mode"], "delta", "{subscribed}");

    let job = recreation_asked(&rig).await;

    let frame = next_sync(&mut ada).await;
    let http = app
        .call(EnvironmentSyncGet, &[&rig.environment])
        .token(&rig.token)
        .query(&EnvironmentSyncGetQuery {
            since: Some(before),
        })
        .send()
        .await
        .ok();
    assert!(http.seq > before, "the job moved the environment's feed");
    assert_eq!(frame["prev"], before);
    assert_eq!(frame["seq"], http.seq);
    assert_eq!(
        frame["data"],
        json!({ "environment": http.environment, "jobs": http.jobs }),
        "the socket and the request say the same"
    );
    assert_eq!(frame["data"]["environment"]["name"], "Workspace");
    assert!(
        frame["data"]["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|each| each["id"] == job.to_string() && each["kind"] == "environment_recreate"),
        "{frame}"
    );

    // Somebody whose page held the number from before the job, and who follows the feed only now,
    // is sent what they missed at once.
    let mut late = Client::signed_in(rig.server.addr, &rig.token).await;
    late.sub(1, &feed, Some(before)).await;
    late.expect("subscribed").await;
    let caught_up = next_sync(&mut late).await;
    assert_eq!(caught_up["prev"], before);
    assert_eq!(caught_up["seq"], frame["seq"]);
    assert_eq!(caught_up["data"], frame["data"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn somebody_who_may_not_read_an_environment_may_not_follow_its_feed() {
    let rig = Rig::new().await;
    let stranger = new_id();
    insert_user(&mut rig.pool.acquire().await.unwrap(), stranger, "cyd")
        .await
        .unwrap();
    let token = rig.token_for(stranger, Span::days(1)).await;
    let mut cyd = Client::signed_in(rig.server.addr, &token).await;
    cyd.sub(4, &format!("env:{}", rig.environment), None).await;
    let refused = cyd.expect("error").await;
    assert_eq!(refused["id"], 4);
    assert_eq!(refused["code"], "forbidden", "{refused}");
    cyd.sub(5, &format!("env:{}", new_id()), None).await;
    let missing = cyd.expect("error").await;
    assert_eq!(missing["code"], "not_found", "{missing}");
}
