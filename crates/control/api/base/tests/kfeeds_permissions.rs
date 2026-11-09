//! Cached feed authority ends on time, and committed changes reach sockets before forbidden data.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod realtime_support;
mod rig;
mod support;
mod terminals_support;

use chrono::{DateTime, Duration as Span, Utc};
use orch_api_core::realtime::{Tuning, raw_traffic};
use orch_db::queries::access::{self, PageKey, testing};
use orch_link::streams::{Received, StreamReceiver};
use orch_link_contract::events::{Event, RawFrame};
use orch_link_contract::session::Role;
use orch_link_contract::streams::{Ev, Stream};
use orch_types::Id;
use realtime_support::{Client, Heard, Rig, Server, quick};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// Polling cannot account for a notification observed within this test's ten-second bound.
const NO_POLL: Duration = Duration::from_secs(3_600);
/// Leaves room for loaded test machines while remaining far below the disabled polling period.
const NOTICE_WAIT: Duration = Duration::from_secs(10);

fn notifications_only(tuning: &mut Tuning) {
    tuning.epoch_every = NO_POLL;
    tuning.sweep_every = NO_POLL;
}

async fn second(rig: &Rig) -> Server {
    let mut tuning = quick(&orch_config::ServerConfig::default());
    notifications_only(&mut tuning);
    let clock = Arc::clone(&rig.server.app.clock);
    Server::over(&rig.pool, &clock, tuning).await
}

async fn grant(
    rig: &Rig,
    reader: Id,
    kind: &str,
    object: Id,
    permission: &str,
    until: Option<DateTime<Utc>>,
) -> Id {
    let mut conn = rig.pool.acquire().await.unwrap();
    testing::add_entry(
        &mut conn,
        &PageKey::new(kind, object, "live"),
        "any",
        permission,
        ("user", Some(reader), None),
        until,
        rig.server.app.clock.now(),
    )
    .await
    .unwrap()
}

async fn take(rig: &Rig, entries: &[Id]) {
    let mut tx = rig.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    for entry in entries {
        assert!(
            access::remove_entry(&mut tx, *entry, rig.ada, rig.server.app.clock.now())
                .await
                .unwrap()
        );
    }
    orch_authz::epoch::bump(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
}

/// What the writers of pages, owners, providers and forks do: take entries and move the epoch with the
/// database's own call, and nothing else.
async fn take_as_every_writer_does(rig: &Rig, entries: &[Id]) {
    let mut tx = rig.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    for entry in entries {
        assert!(
            access::remove_entry(&mut tx, *entry, rig.ada, rig.server.app.clock.now())
                .await
                .unwrap()
        );
    }
    access::bump_epoch(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
}

async fn closed_before_content(client: &mut Client, ids: &[i64], forbidden: &str) {
    let started = tokio::time::Instant::now();
    let mut remaining: HashSet<_> = ids.iter().copied().collect();
    while !remaining.is_empty() {
        let wait = NOTICE_WAIT.saturating_sub(started.elapsed());
        let Some(Heard::Frame(frame)) = client.hear_within(wait).await else {
            panic!("subscriptions {remaining:?} did not close before the next content");
        };
        assert!(
            !frame.to_string().contains(forbidden),
            "forbidden data arrived: {frame}"
        );
        if frame["op"] == "closed" {
            assert_eq!(frame["reason"], "revoked", "{frame}");
            remaining.remove(&frame["id"].as_i64().unwrap());
        }
    }
}

async fn publish_raw(rig: &Rig, thread: Id, message: &str) {
    let segment = rig.segment_of(thread).await;
    let stream = Stream::Session {
        thread_id: thread,
        segment_id: segment,
    };
    let event = Event::RawFrame(RawFrame {
        n: orch_db::queries::raw_events::last_number(
            &mut rig.pool.acquire().await.unwrap(),
            segment,
        )
        .await
        .unwrap()
            + 1,
        direction: "in".to_owned(),
        payload: json!({"message": message}),
    });
    let mut receiver = StreamReceiver::new(BTreeMap::new(), vec![Role::Agent]);
    let Received::Apply(delivery) =
        receiver.on_ev(Ev::of(&stream, 1, rig.server.app.clock.time(), &event).unwrap())
    else {
        panic!("a raw delivery");
    };
    let mut tx = rig.pool.begin().await.unwrap();
    let boundary = raw_traffic::before(&mut tx, &delivery).await.unwrap();
    let row = orch_db::queries::threads::thread(&mut tx, thread)
        .await
        .unwrap()
        .unwrap();
    let mut keeper = orch_engine::raw_events::Keeper::new(&row, segment);
    let Event::RawFrame(frame) = event else {
        unreachable!("the diagnostic frame above")
    };
    keeper
        .keep(&mut tx, &frame, rig.server.app.clock.now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    raw_traffic::after(&rig.pool, boundary, &delivery)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_committed_permission_change_closes_live_raw_and_terminal_feeds_on_two_apis_without_polling()
 {
    let rig = Rig::tuned(notifications_only).await;
    let peer = second(&rig).await;
    let thread = rig.thread("Three live doors").await;
    rig.send(thread, "begin the diagnostic session").await;
    let segment = rig.segment_of(thread).await;
    let (reader, token) = rig.person("reader").await;
    let entries = [
        grant(&rig, reader, "node", thread, "thread.view", None).await,
        grant(&rig, reader, "node", thread, "thread.manage", None).await,
        grant(
            &rig,
            reader,
            "environment",
            rig.environment,
            "terminal.view",
            None,
        )
        .await,
    ];
    let mut one = Client::signed_in(rig.server.addr, &token).await;
    let mut two = Client::signed_in(peer.addr, &token).await;
    for client in [&mut one, &mut two] {
        client.sub(1, &format!("thread:{thread}"), None).await;
        client.expect("subscribed").await;
        client.expect("presence").await;
        client.sub_raw(2, thread, segment).await;
        client.expect("subscribed").await;
        client
            .sub(3, &format!("env:{}:procs", rig.environment), None)
            .await;
        client.expect("subscribed").await;
    }
    take(&rig, &entries).await;
    for client in [&mut one, &mut two] {
        closed_before_content(client, &[1, 2, 3], "unread after revocation").await;
    }
    drop((one, two));
    peer.stop().await;
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writer_that_moves_the_epoch_by_the_databases_own_call_closes_feeds_on_two_apis_without_polling()
 {
    let rig = Rig::tuned(notifications_only).await;
    let peer = second(&rig).await;
    let thread = rig.thread("Any writer").await;
    let (reader, token) = rig.person("reader").await;
    let entries = [grant(&rig, reader, "node", thread, "thread.view", None).await];
    let mut one = Client::signed_in(rig.server.addr, &token).await;
    let mut two = Client::signed_in(peer.addr, &token).await;
    for client in [&mut one, &mut two] {
        client.sub(1, &format!("thread:{thread}"), None).await;
        client.expect("subscribed").await;
        client.expect("presence").await;
    }
    take_as_every_writer_does(&rig, &entries).await;
    for client in [&mut one, &mut two] {
        closed_before_content(client, &[1], "unread after revocation").await;
    }
    drop((one, two));
    peer.stop().await;
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_next_raw_frame_after_its_sole_manage_grant_ends_is_withheld_on_both_apis() {
    let rig = Rig::cramped_with_time_held(notifications_only).await;
    let peer = second(&rig).await;
    let thread = rig.thread("Timed diagnostic grant").await;
    rig.send(thread, "begin the diagnostic session").await;
    let segment = rig.segment_of(thread).await;
    let (reader, token) = rig.person("reader").await;
    grant(
        &rig,
        reader,
        "node",
        thread,
        "thread.manage",
        Some(rig.server.app.clock.now() + Span::seconds(1)),
    )
    .await;
    let mut one = Client::signed_in(rig.server.addr, &token).await;
    let mut two = Client::signed_in(peer.addr, &token).await;
    for client in [&mut one, &mut two] {
        client.sub_raw(1, thread, segment).await;
        client.expect("subscribed").await;
    }
    // Cross the database's microsecond timestamp boundary even if it rounded upward.
    rig.let_time_pass(Span::seconds(2));
    publish_raw(&rig, thread, "unread after grant expiry").await;
    for client in [&mut one, &mut two] {
        closed_before_content(client, &[1], "unread after grant expiry").await;
    }
    drop((one, two));
    peer.stop().await;
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ended_role_cannot_authorize_the_next_live_message_without_an_epoch_change() {
    let rig = Rig::cramped_with_time_held(notifications_only).await;
    let peer = second(&rig).await;
    let thread = rig.thread("Timed role").await;
    let turn = rig.send(thread, "Before the role ended").await;
    let (reader, token) = rig.person("reader").await;
    {
        let mut conn = rig.pool.acquire().await.unwrap();
        testing::give_role(
            &mut conn,
            rig.topic,
            "user",
            reader,
            "viewer",
            rig.ada,
            Some(rig.server.app.clock.now() + Span::seconds(1)),
        )
        .await
        .unwrap();
    }
    let mut one = Client::signed_in(rig.server.addr, &token).await;
    let mut two = Client::signed_in(peer.addr, &token).await;
    for client in [&mut one, &mut two] {
        client.sub(1, &format!("thread:{thread}"), None).await;
        client.expect("subscribed").await;
        client.expect("presence").await;
    }
    rig.let_time_pass(Span::seconds(2));
    {
        let mut conn = rig.pool.acquire().await.unwrap();
        let principal = orch_authz::Principal::load(
            &mut conn,
            orch_authz::Caller::session(reader),
            rig.server.app.clock.time(),
        )
        .await
        .unwrap();
        let decision = orch_authz::nodes::check(
            &mut conn,
            &principal,
            thread,
            orch_access_contract::permissions::Permission::ThreadView,
            orch_authz::Act::Use,
            orch_authz::Policy::default(),
        )
        .await
        .unwrap();
        assert!(
            !decision.is_allowed(),
            "the sole role has ended for an ordinary read"
        );
    }
    rig.begin_message(thread, &turn, "role-ended").await;
    rig.finish_turn(thread, &turn, "role-ended", "unread after role expiry")
        .await;
    for client in [&mut one, &mut two] {
        closed_before_content(client, &[1], "unread after role expiry").await;
    }
    drop((one, two));
    peer.stop().await;
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_projection_requires_a_whole_reload_before_further_durable_content() {
    let rig = Rig::tuned(notifications_only).await;
    let thread = rig.thread("Still readable").await;
    let turn = rig.send(thread, "Before the projection changed").await;
    let (reader, token) = rig.person("reader").await;
    grant(&rig, reader, "node", thread, "thread.view", None).await;
    let manage = grant(&rig, reader, "node", thread, "thread.manage", None).await;
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    client.sub(1, &format!("thread:{thread}"), None).await;
    let start = client.expect("subscribed").await["seq"].as_i64().unwrap();
    client.expect("presence").await;
    take(&rig, &[manage]).await;
    assert_eq!(client.expect("resync").await["id"], 1);
    rig.begin_message(thread, &turn, "projection-changed").await;
    rig.finish_turn(
        thread,
        &turn,
        "projection-changed",
        "content after the projection changed",
    )
    .await;
    assert!(
        client.is_quiet_for(Duration::from_millis(300)).await,
        "a changed projection must be cleared before more data"
    );
    client
        .sub(1, &format!("thread:{thread}"), Some(start))
        .await;
    assert_eq!(client.expect("subscribed").await["mode"], "reload");
    // The application now reloads the authorized HTTP projection without a cursor.
    client.sub(1, &format!("thread:{thread}"), None).await;
    assert_eq!(client.expect("subscribed").await["mode"], "delta");
    client.expect("presence").await;
    drop(client);
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_committed_role_removal_closes_a_terminal_socket_and_a_peers_live_and_raw_feeds_without_polling()
 {
    let floor = terminals_support::Floor::tuned(notifications_only).await;
    let made = floor.shell_of(&floor.scene.dan).await;
    let mut screen = floor.screen(made.session.id, &made.ticket).await;
    screen.ready().await;
    let app = floor.app();
    let reader = floor.scene.dan.id();
    let mut conn = app.db.acquire().await.unwrap();
    let raw_grant = testing::add_entry(
        &mut conn,
        &PageKey::new("node", floor.thread, "live"),
        "any",
        "thread.manage",
        ("user", Some(reader), None),
        None,
        app.clock.now(),
    )
    .await
    .unwrap();
    drop(conn);
    let mut tuning = quick(&app.config);
    notifications_only(&mut tuning);
    let peer = Server::over(&app.db, &app.clock, tuning).await;
    let mut client = Client::signed_in(peer.addr, &floor.scene.dan.token).await;
    client
        .sub(1, &format!("thread:{}", floor.thread), None)
        .await;
    client.expect("subscribed").await;
    client.expect("presence").await;
    let segment: Id =
        sqlx::query_scalar("SELECT current_segment_id FROM threads WHERE node_id = $1")
            .bind(floor.thread)
            .fetch_one(app.db.pool())
            .await
            .unwrap();
    client.sub_raw(2, floor.thread, segment).await;
    client.expect("subscribed").await;
    let mut tx = app.db.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    let role: Id = sqlx::query_scalar(
        "SELECT id FROM node_roles WHERE node_id = $1 AND subject_id = $2 AND removed_at IS NULL",
    )
    .bind(floor.scene.tree.org.base.id)
    .bind(reader)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    testing::take_role_away(&mut tx, role, floor.scene.ada.id(), app.clock.now())
        .await
        .unwrap();
    access::remove_entry(&mut tx, raw_grant, floor.scene.ada.id(), app.clock.now())
        .await
        .unwrap();
    orch_authz::epoch::bump(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    closed_before_content(&mut client, &[1, 2], "unread after the role ended").await;
    let code = tokio::time::timeout(NOTICE_WAIT, screen.until_the_end())
        .await
        .expect("the terminal closes from the committed notification, without polling");
    assert_eq!(code, Some(1000));
    assert_eq!(screen.closed_reason().as_deref(), Some("revoked"));
    drop((client, screen));
    peer.stop().await;
    floor.stop().await;
}

async fn socket_closed_before_content(client: &mut Client, forbidden: &str) {
    let started = tokio::time::Instant::now();
    loop {
        let wait = NOTICE_WAIT.saturating_sub(started.elapsed());
        match client.hear_within(wait).await {
            Some(Heard::Frame(frame)) => {
                assert!(
                    !frame.to_string().contains(forbidden),
                    "forbidden identity data: {frame}"
                );
            }
            Some(Heard::Closed(code)) => {
                assert_eq!(code, Some(4401));
                break;
            }
            None => panic!("the ended identity did not close before further data"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_socket_token_cannot_receive_the_next_frame_even_when_another_token_is_valid() {
    let rig = Rig::cramped_with_time_held(|tuning| {
        notifications_only(tuning);
        tuning.tick = NO_POLL;
    })
    .await;
    let thread = rig.thread("Ended token").await;
    rig.send(thread, "begin the diagnostic session").await;
    let segment = rig.segment_of(thread).await;
    let token = rig.token_for(rig.ada, Span::seconds(1)).await;
    let mut client = Client::signed_in(rig.server.addr, &token).await;
    client.sub_raw(1, thread, segment).await;
    client.expect("subscribed").await;
    publish_raw(&rig, thread, "read before token expiry").await;
    // A raw event names its segment and the history it was admitted for beside the observation.
    assert_eq!(
        client.expect("event").await["data"]["event"]["payload"]["message"],
        "read before token expiry"
    );
    // Let the socket's initial interval tick run while its held clock still admits the token.
    assert!(client.is_quiet_for(Duration::from_millis(200)).await);
    rig.let_time_pass(Span::seconds(2));
    publish_raw(&rig, thread, "unread after token expiry").await;
    socket_closed_before_content(&mut client, "unread after token expiry").await;
    let mut fresh = Client::signed_in(rig.server.addr, &rig.token).await;
    fresh.sub_raw(1, thread, segment).await;
    fresh.expect("subscribed").await;
    drop((client, fresh));
    rig.server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_committed_session_end_closes_both_apis_before_the_next_raw_frame() {
    let rig = Rig::tuned(notifications_only).await;
    let peer = second(&rig).await;
    let thread = rig.thread("Ended session").await;
    rig.send(thread, "begin the diagnostic session").await;
    let segment = rig.segment_of(thread).await;
    let mut one = Client::signed_in(rig.server.addr, &rig.token).await;
    let mut two = Client::signed_in(peer.addr, &rig.token).await;
    for client in [&mut one, &mut two] {
        client.sub_raw(1, thread, segment).await;
        client.expect("subscribed").await;
    }
    let mut tx = rig.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE sessions SET revoked_at = $2 WHERE id = (
        SELECT session_id FROM access_tokens WHERE token_hash = $1)",
    )
    .bind(orch_api_core::middleware::authentication::token_hash(
        &rig.token,
    ))
    .bind(rig.server.app.clock.now())
    .execute(&mut *tx)
    .await
    .unwrap();
    orch_authz::epoch::bump(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    publish_raw(&rig, thread, "unread after session end").await;
    for client in [&mut one, &mut two] {
        socket_closed_before_content(client, "unread after session end").await;
    }
    drop((one, two));
    peer.stop().await;
    rig.server.stop().await;
}
