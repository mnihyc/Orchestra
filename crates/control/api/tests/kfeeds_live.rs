#![recursion_limit = "256"]
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
    let response = app
        .call(EnvironmentSyncGet, &[&rig.environment])
        .token(&rig.token)
        .query(&EnvironmentSyncGetQuery {
            since: Some(before),
        })
        .send()
        .await;
    let wire: Value = serde_json::from_slice(&response.raw).unwrap();
    let http = response.ok();
    assert!(wire.get("library").is_some_and(Value::is_object), "{wire}");
    let expected = app
        .call(orch_api_contract::library::ResourceLibraryRead, &[])
        .token(&rig.token)
        .raw_body(&json!({"targets":[{"object":{"kind":"environment","id":rig.environment},"version_id":null}]}))
        .send_bytes()
        .await;
    assert!(expected.status.is_success());
    let expected: Value = serde_json::from_slice(&expected.raw).unwrap();
    assert_eq!(wire["library"], expected[0]["facts"]);
    assert!(http.seq > before, "the job moved the environment's feed");
    assert_eq!(frame["prev"], before);
    assert_eq!(frame["seq"], http.seq);
    assert_eq!(
        frame["data"],
        json!({ "environment": http.environment, "jobs": http.jobs, "library": wire["library"] }),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_door_only_reader_gets_explicit_null_library_facts_on_http_and_the_environment_feed() {
    let rig = Rig::with_unstarted_engine_and_time_held().await;
    let (reader, token) = rig.person("terminal-reader").await;
    orch_db::queries::access::testing::add_entry(
        &mut rig.pool.acquire().await.unwrap(),
        &orch_db::queries::access::PageKey::new("environment", rig.environment, "live"),
        "any",
        "terminal.view",
        ("user", Some(reader), None),
        None,
        rig.clock.now(),
    )
    .await
    .unwrap();
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::over(
        rig.server.app.clone(),
        orch_testkit::clock::manual(),
        registry,
    );
    let refused = app.call(orch_api_contract::library::ResourceLibraryRead, &[])
        .token(&token)
        .raw_body(&json!({"targets":[{"object":{"kind":"environment","id":rig.environment},"version_id":null}]}))
        .send_bytes().await;
    assert_eq!(
        refused.problem().code,
        orch_api_contract::common::ErrorCode::NotFound
    );
    let before = env_seq(&rig).await;
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    client
        .sub(1, &format!("env:{}", rig.environment), Some(before))
        .await;
    assert_eq!(client.expect("subscribed").await["mode"], "delta");
    let job = recreation_asked(&rig).await;
    let frame = next_sync(&mut client).await;
    let response = app
        .call(EnvironmentSyncGet, &[&rig.environment])
        .token(&token)
        .query(&EnvironmentSyncGetQuery {
            since: Some(before),
        })
        .send_bytes()
        .await;
    assert!(response.status.is_success());
    let http: Value = serde_json::from_slice(&response.raw).unwrap();
    assert_eq!(http["jobs"][0]["id"], job.to_string());
    assert_eq!(http.get("library"), Some(&Value::Null));
    assert_eq!(frame["data"].get("library"), Some(&Value::Null));
    assert_eq!(frame["data"]["environment"], http["environment"]);
    assert_eq!(frame["data"]["jobs"], http["jobs"]);
}
