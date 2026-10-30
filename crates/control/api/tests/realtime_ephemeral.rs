//! Unreplayed lists reach every API process, seed new readers and obey the environment's access page.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod realtime_support;

use orch_realtime_contract::feeds::Feed;
use realtime_support::{Client, Rig, Server, quick};
use serde_json::json;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ten_whole_lists_seed_readers_on_two_processes_with_the_last_complete_list() {
    let rig = Rig::new().await;
    let second = Server::over(
        &rig.pool,
        &rig.clock,
        quick(&orch_config::ServerConfig::default()),
    )
    .await;
    let feed = Feed::EnvBrowsers(rig.environment);
    let mut one = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut two = Client::signed_in(second.addr, &rig.token).await;
    let profile = |n| orch_api_contract::browsers::BrowserProfile {
        name: format!("profile-{n}"),
        size_bytes: None,
        used_at: None,
        browser: None,
    };
    for n in 0..10 {
        rig.server
            .realtime
            .publish_list(
                feed.clone(),
                json!({"browsers": [], "profiles": [profile(n)]}),
            )
            .await
            .unwrap();
    }
    // Publishing is complete before either reader asks for its snapshot. The coordinator's
    // unit case separately proves coalescing within one controlled collection interval.
    for client in [&mut one, &mut two] {
        client.sub(1, &feed.to_string(), Some(999)).await;
        let frame = client.expect("subscribed").await;
        assert_eq!(frame["mode"], "ephemeral");
        assert!(frame["seq"].is_null());
        let event = client.expect("event").await;
        assert_eq!(
            event["data"],
            json!({"browsers": [], "profiles": [profile(9)]})
        );
        assert!(client.is_quiet_for(Duration::from_millis(200)).await);
    }
    let mut newcomer = Client::signed_in(second.addr, &rig.token).await;
    newcomer.sub(2, &feed.to_string(), None).await;
    newcomer.expect("subscribed").await;
    assert_eq!(
        newcomer.expect("event").await["data"],
        json!({"browsers": [], "profiles": [profile(9)]})
    );
    drop((one, two, newcomer));
    second.stop().await;
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_list_larger_than_one_database_notification_arrives_whole() {
    let rig = Rig::new().await;
    // What a feed carries is its typed list: a browser list of four hundred profiles is larger than
    // one notification of the database.
    let feed = Feed::EnvBrowsers(rig.environment);
    let profiles: Vec<_> = (0..400)
        .map(|n| orch_api_contract::browsers::BrowserProfile {
            name: format!("profile-{n:04}"),
            size_bytes: None,
            used_at: None,
            browser: None,
        })
        .collect();
    let data = json!({"browsers": [], "profiles": profiles});
    rig.server
        .realtime
        .publish_list(feed.clone(), data.clone())
        .await
        .unwrap();
    let mut client = Client::signed_in(rig.server.addr, &rig.token).await;
    client.sub(1, &feed.to_string(), None).await;
    client.expect("subscribed").await;
    assert_eq!(client.expect("event").await["data"], data);
    drop(client);
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stranger_cannot_follow_a_browser_or_display_list() {
    let rig = Rig::new().await;
    let (_, token) = rig.person("stranger").await;
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    for feed in [
        Feed::EnvBrowsers(rig.environment),
        Feed::EnvDisplays(rig.environment),
    ] {
        client.sub(1, &feed.to_string(), None).await;
        assert_eq!(client.expect("error").await["code"], "forbidden");
    }
    drop(client);
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subscription_closes_when_the_person_loses_what_let_them_follow() {
    let rig = Rig::new().await;
    let (reader, token) = rig.person("reader").await;
    let role = rig.give(reader, "developer").await;
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    client
        .sub(1, &Feed::EnvBrowsers(rig.environment).to_string(), None)
        .await;
    client.expect("subscribed").await;
    rig.take(role).await;
    assert_eq!(client.expect("closed").await["reason"], "revoked");
    drop(client);
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_watch_coordinator_holds_a_renewed_lease_and_no_transaction_stays_open_after_its_election()
 {
    let rig = Rig::new().await;
    // A process coordinates the transient watches once it has the machines' link, which the first
    // socket that signs in gives it.
    let _socket = realtime_support::Client::signed_in(rig.server.addr, &rig.token).await;
    realtime_support::coordinator_ready(&rig).await;
    // The coordinator renews its lease every second. A lock kept in a transaction instead would
    // show here as a transaction open since the election.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        realtime_support::transactions_open_longer_than(&rig, 2.0).await,
        0
    );
    let mut conn = rig.pool.acquire().await.unwrap();
    let holder = orch_db::queries::job_leases::holder(
        &mut conn,
        orch_api_core::realtime::ephemeral::COORDINATOR,
        orch_types::Clock::now(rig.clock.as_ref()),
    )
    .await
    .unwrap();
    assert!(holder.is_some(), "the coordinator still holds its lease");
}
