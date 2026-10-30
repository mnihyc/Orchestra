//! A person is linked to a bot by a code that is good for ten minutes, the bot is asked about it only while a code waits, and a person who stopped the bot is found at the next delivery.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_db::queries::notifications::people;
use orch_db::queries::notifications::routes;
use orch_notifier_contract::delivery::{DeliverError, LinkPoll, LinkStarted};
use orch_notifier_contract::descriptor::{Needs, Reaches};
use orch_worker_core::bot_links::{self, LINK_LIFE, follow, hash_code, start};
use orch_worker_core::channels::Notifiers;
use serde_json::json;
use std::sync::Arc;
use support::fakes::Scripted;
use support::world::{Of, World};

fn only(notifier: &Arc<Scripted>) -> Notifiers {
    let mut notifiers = Notifiers::new();
    notifiers.register(notifier.clone()).unwrap();
    notifiers
}

fn followed(code: &str, chat: i64, label: &str) -> LinkStarted {
    LinkStarted {
        code: code.to_owned(),
        address: json!({ "chat_id": chat }),
        label: label.to_owned(),
    }
}

/// The code that is in the address a person was given.
fn code_in(url: &str) -> String {
    url.rsplit_once("code=").unwrap().1.to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_is_linked_by_a_code_that_comes_back_from_the_bot_and_the_code_is_spent() {
    let bot = Scripted::new("bot", Reaches::Link);
    let world = World::new(only(&bot)).await;
    let channel = world.channel(Of::System, "bot", json!({}), &[]).await;
    let (ada, bob) = (world.person("ada").await, world.person("bob").await);
    let mut conn = world.test.pool.acquire().await.unwrap();

    let started = start(
        &mut conn,
        &world.notifiers,
        Some(&world.master),
        ada,
        &channel,
        world.now(),
    )
    .await
    .unwrap();
    assert_eq!(started.expires_at, world.now() + LINK_LIFE);
    assert_eq!(LINK_LIFE, Duration::minutes(10));
    let code = code_in(&started.url);
    let route = routes::route_for(&mut conn, ada, channel.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !route.enabled,
        "a route is off until the person switches it on"
    );
    assert_eq!(
        route.link_hash.as_deref(),
        Some(hash_code(&code).as_slice()),
        "only the hash is kept"
    );
    assert_eq!(route.link_expires_at, Some(started.expires_at));

    let again = start(
        &mut conn,
        &world.notifiers,
        Some(&world.master),
        ada,
        &channel,
        world.now(),
    )
    .await
    .unwrap();
    assert_ne!(
        code_in(&again.url),
        code,
        "a second link replaces the first"
    );
    let code = code_in(&again.url);
    let other = start(
        &mut conn,
        &world.notifiers,
        Some(&world.master),
        bob,
        &channel,
        world.now(),
    )
    .await
    .unwrap();
    let others_code = code_in(&other.url);
    drop(conn);

    bot.followers([Ok(LinkPoll {
        started: vec![
            followed("nobody-asked-for-this", 1, "Stranger"),
            followed(&code, 42, "Dana"),
        ],
        cursor: Some("7".into()),
    })]);
    let done = follow(&world.db(), &world.clock, &world.notifiers, &world.master)
        .await
        .unwrap();
    assert_eq!((done.asked, done.linked), (1, 1));
    assert_eq!(
        bot.polled(),
        [None],
        "the first reading starts from nothing"
    );

    let route = world.route_now(route.id).await;
    assert_eq!(
        route.address,
        json!({ "chat_id": 42, "label": "Dana" }),
        "the chat, and who started it"
    );
    assert!(
        route.linked_at.is_some() && route.link_hash.is_none(),
        "and the code is spent"
    );
    assert!(!route.enabled, "linking does not switch the route on");
    assert_eq!(
        world.channel_now(channel.id).await.link_cursor.as_deref(),
        Some("7")
    );
    let mut conn = world.test.pool.acquire().await.unwrap();
    let person = people::recipient(&mut conn, ada).await.unwrap().unwrap();
    use orch_notifier_contract::notifier::Notifier;
    assert_eq!(
        orch_worker_core::routes::needs(&mut conn, &bot.describe(), &person, Some(&route))
            .await
            .unwrap(),
        None
    );

    // Bob's code still waits, so the bot is asked again, from where the last reading ended.
    bot.followers([Ok(LinkPoll {
        started: vec![
            followed(&code, 42, "Dana"),
            followed(&others_code, 43, "Bo"),
        ],
        cursor: Some("9".into()),
    })]);
    let done = follow(&world.db(), &world.clock, &world.notifiers, &world.master)
        .await
        .unwrap();
    assert_eq!(
        done.linked, 1,
        "a code that came back is good once: Ada's is spent"
    );
    assert_eq!(bot.polled(), [None, Some("7".to_owned())]);
    assert_eq!(world.route_now(route.id).await.address["chat_id"], 42);
    let bobs = routes::route_for(&mut conn, bob, channel.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bobs.address, json!({ "chat_id": 43, "label": "Bo" }));

    // Nothing waits any longer: nobody is asked.
    let done = follow(&world.db(), &world.clock, &world.notifiers, &world.master)
        .await
        .unwrap();
    assert_eq!(done.asked, 0);
    assert_eq!(bot.polled().len(), 2, "no code waiting, no request");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_code_that_comes_back_after_its_time_matches_nothing_and_asks_nobody() {
    let bot = Scripted::new("bot", Reaches::Link);
    let world = World::new(only(&bot)).await;
    let channel = world.channel(Of::System, "bot", json!({}), &[]).await;
    let ada = world.person("ada").await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    let started = start(
        &mut conn,
        &world.notifiers,
        Some(&world.master),
        ada,
        &channel,
        world.now(),
    )
    .await
    .unwrap();
    let code = code_in(&started.url);
    drop(conn);

    world.clock.advance(Duration::minutes(9));
    bot.followers([Ok(LinkPoll {
        started: vec![],
        cursor: Some("1".into()),
    })]);
    assert_eq!(
        follow(&world.db(), &world.clock, &world.notifiers, &world.master)
            .await
            .unwrap()
            .asked,
        1,
        "still good"
    );

    world.clock.advance(Duration::minutes(2));
    bot.followers([Ok(LinkPoll {
        started: vec![followed(&code, 42, "Dana")],
        cursor: Some("2".into()),
    })]);
    let done = follow(&world.db(), &world.clock, &world.notifiers, &world.master)
        .await
        .unwrap();
    assert_eq!(
        (done.asked, done.linked),
        (0, 0),
        "past its ten minutes nobody is asked"
    );
    let route = routes::route_for(
        &mut world.test.pool.acquire().await.unwrap(),
        ada,
        channel.id,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(route.link_hash.is_none(), "and the code is dropped");
    assert_eq!(route.address, json!({}));
    assert_eq!(bot.polled().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reading_that_fails_leaves_the_cursor_where_it_was_and_shows_why_on_the_channel() {
    let bot = Scripted::new("bot", Reaches::Link);
    let world = World::new(only(&bot)).await;
    let channel = world.channel(Of::System, "bot", json!({}), &[]).await;
    let ada = world.person("ada").await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    start(
        &mut conn,
        &world.notifiers,
        Some(&world.master),
        ada,
        &channel,
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    bot.followers([Err(DeliverError::temporary(
        "unreachable",
        "the bot's server is down",
    ))]);
    let done = follow(&world.db(), &world.clock, &world.notifiers, &world.master)
        .await
        .unwrap();
    assert_eq!((done.asked, done.linked), (1, 0));
    let channel = world.channel_now(channel.id).await;
    assert!(
        channel
            .last_error
            .unwrap()
            .contains("the bot's server is down")
    );
    assert_eq!(
        channel.link_cursor, None,
        "a reading that failed keeps nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_who_stopped_the_bot_is_found_at_the_next_delivery_and_their_route_is_off_and_unlinked()
 {
    let bot = Scripted::new("bot", Reaches::Link);
    let world = World::new(only(&bot)).await;
    let channel = world.channel(Of::System, "bot", json!({}), &[]).await;
    let ada = world.person("ada").await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    let started = start(
        &mut conn,
        &world.notifiers,
        Some(&world.master),
        ada,
        &channel,
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    bot.followers([Ok(LinkPoll {
        started: vec![followed(&code_in(&started.url), 42, "Dana")],
        cursor: Some("3".into()),
    })]);
    follow(&world.db(), &world.clock, &world.notifiers, &world.master)
        .await
        .unwrap();
    let route = world.route_on(ada, &channel).await;
    assert!(route.enabled);

    world
        .queue_for_route(
            &route,
            "bot",
            &[world.snapshot("approval.requested", "Approve?")],
        )
        .await;
    assert_eq!(world.outbox().run().await.unwrap().sent, 1);
    assert_eq!(
        bot.delivered()[0].to,
        json!({ "chat_id": 42, "label": "Dana" })
    );

    bot.answer([Err(DeliverError::gone(
        "blocked",
        "the person stopped the bot",
    ))]);
    world
        .queue_for_route(
            &route,
            "bot",
            &[world.snapshot("approval.requested", "Approve again?")],
        )
        .await;
    assert_eq!(world.outbox().run().await.unwrap().gone, 1);
    let route = world.route_now(route.id).await;
    assert!(!route.enabled && route.address == json!({}) && route.linked_at.is_none());
    let mut conn = world.test.pool.acquire().await.unwrap();
    let person = people::recipient(&mut conn, ada).await.unwrap().unwrap();
    use orch_notifier_contract::notifier::Notifier;
    assert_eq!(
        orch_worker_core::routes::needs(&mut conn, &bot.describe(), &person, Some(&route))
            .await
            .unwrap(),
        Some(Needs::Link),
        "their card says it has to be connected again"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_channel_whose_notifier_links_nobody_gives_no_link() {
    let plain = Scripted::new("plain", Reaches::Channel);
    let world = World::new(only(&plain)).await;
    let channel = world.channel(Of::System, "plain", json!({}), &[]).await;
    let ada = world.person("ada").await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    let error = start(
        &mut conn,
        &world.notifiers,
        Some(&world.master),
        ada,
        &channel,
        world.now(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, bot_links::LinkError::NotLinkable(_)),
        "{error}"
    );
    assert!(
        routes::route_for(&mut conn, ada, channel.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn while_the_notifier_is_asked_no_transaction_is_open_and_every_action_goes_on() {
    let bot = Scripted::new("bot", Reaches::Link);
    let world = World::new(only(&bot)).await;
    let channel = world.channel(Of::System, "bot", json!({}), &[]).await;
    let ada = world.person("ada").await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    let _started = start(
        &mut conn,
        &world.notifiers,
        Some(&world.master),
        ada,
        &channel,
        world.now(),
    )
    .await
    .unwrap();
    drop(conn);
    // The reading takes two seconds, as a long poll does while it waits for somebody to press Start.
    bot.takes(std::time::Duration::from_secs(2));
    let db = world.db();
    let (done, ()) = tokio::join!(
        follow(&db, &world.clock, &world.notifiers, &world.master),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            assert_eq!(bot.polled().len(), 1, "the notifier is being asked");
            let mut tx = db.begin().await.unwrap();
            tokio::time::timeout(
                std::time::Duration::from_millis(500),
                orch_db::queries::transaction_guard::lock_action(&mut tx),
            )
            .await
            .expect("an action takes the gate at once: the reading does not hold it")
            .unwrap();
            tx.rollback().await.unwrap();
            let open: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database()
                  AND pid <> pg_backend_pid() AND state LIKE 'idle in transaction%'",
            )
            .fetch_one(&world.test.pool)
            .await
            .unwrap();
            assert_eq!(open, 0, "and no transaction waits on the notifier");
        }
    );
    assert_eq!(done.unwrap().asked, 1);
}
