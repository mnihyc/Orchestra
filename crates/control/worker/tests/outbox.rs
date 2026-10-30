//! What the outbox does with a row: retries it on the schedule and gives up, delivers it once however many workers there are, takes up what a dead worker held, and ends the route or the destination whose target is gone.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_db::queries::identity::queue_mail;
use orch_db::queries::notifications::channels::{self, NewDestination};
use orch_db::queries::notifications::outbox as db_outbox;
use orch_db::queries::notifications::routes::RouteChange;
use orch_notifier_contract::delivery::DeliverError;
use orch_notifier_contract::descriptor::Reaches;
use orch_notifier_contract::templates::BodyKind;
use orch_types::{Id, new_id};
use orch_worker_core::channels::Notifiers;
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::Arc;
use support::fakes::Scripted;
use support::world::{Of, World};

fn only(notifier: &Arc<Scripted>) -> Notifiers {
    let mut notifiers = Notifiers::new();
    notifiers.register(notifier.clone()).unwrap();
    notifiers
}

/// Puts a message that signing in would write into the outbox, for `to`.
async fn mail(world: &World, channel: Id, to: &str, code: &str) -> Id {
    let mut conn = world.test.pool.acquire().await.unwrap();
    queue_mail(
        &mut conn,
        "email",
        channel,
        &json!({
            "purpose": "sign_in_code", "to": to, "user_id": null, "language": "en",
            "data": { "code": code, "expires_at": "2026-10-08T14:10:00.000Z" },
            "subject": format!("Your code: {code}"), "body": format!("Your code is {code}.")
        }),
        world.now(),
    )
    .await
    .unwrap()
}

fn unreachable() -> DeliverError {
    DeliverError::temporary("unreachable", "connection refused")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delivery_error_cannot_store_a_channel_credential_in_the_outbox_or_channel() {
    const SECRET: &str = "channel-credential-never-stored-in-an-error";
    let notifier = Scripted::new("email", Reaches::Email);
    notifier.answer([Err(DeliverError::temporary(
        "authentication",
        &format!("The server rejected {SECRET}"),
    ))]);
    let world = World::new(only(&notifier)).await;
    let channel = world
        .channel(Of::System, "email", json!({}), &[("token", SECRET)])
        .await;
    let id = mail(&world, channel.id, "ada@example.org", "ABCD-1234").await;
    assert_eq!(world.outbox().run().await.unwrap().retried, 1);
    for error in [
        world.row(id).await.last_error,
        world.channel_now(channel.id).await.last_error,
    ] {
        let error = error.unwrap();
        assert!(error.contains("The server rejected"));
        assert!(!error.contains(SECRET));
        assert!(error.contains(orch_plugin_contract::secret::REDACTED));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delivery_that_fails_is_retried_on_the_schedule_and_given_up_at_the_end() {
    let notifier = Scripted::new("email", Reaches::Email);
    notifier.answer(std::iter::repeat_with(|| Err(unreachable())).take(6));
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "email", json!({}), &[]).await;
    let id = mail(&world, channel.id, "ada@example.org", "ABCD-1234").await;

    for (n, pause) in [60_i64, 300, 900, 3_600, 21_600].into_iter().enumerate() {
        let tried_at = world.now();
        let done = world.outbox().run().await.unwrap();
        assert_eq!((done.retried, done.total()), (1, 1), "try {}", n + 1);
        let row = world.row(id).await;
        assert_eq!(
            (row.state.as_str(), row.attempts),
            ("pending", i32::try_from(n + 1).unwrap())
        );
        assert_eq!(
            row.next_attempt_at,
            tried_at + Duration::seconds(pause),
            "the pause after try {}",
            n + 1
        );
        assert!(
            row.last_error
                .as_deref()
                .unwrap()
                .contains("connection refused")
        );
        assert!(
            row.payload["data"]["code"] == "ABCD-1234",
            "the code stays while the message may still be sent"
        );

        world.clock.advance(Duration::seconds(pause - 1));
        assert_eq!(
            world.outbox().run().await.unwrap().total(),
            0,
            "not due {}s early",
            1
        );
        world.clock.advance(Duration::seconds(1));
    }
    let done = world.outbox().run().await.unwrap();
    assert_eq!(
        (done.failed, done.retried),
        (1, 0),
        "after the sixth try it has failed"
    );
    let row = world.row(id).await;
    assert_eq!((row.state.as_str(), row.attempts), ("failed", 6));
    assert!(
        !row.payload.to_string().contains("ABCD-1234"),
        "a message that will never go holds no code"
    );
    assert_eq!(notifier.deliveries(), 6);
    assert_eq!(world.outbox().run().await.unwrap().total(), 0);
    let channel = world.channel_now(channel.id).await;
    assert!(
        channel.last_error.unwrap().contains("connection refused"),
        "the reason is kept on the channel"
    );
    assert!(channel.last_ok_at.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delivery_that_succeeds_after_a_failure_clears_the_error_of_the_channel() {
    let notifier = Scripted::new("email", Reaches::Email);
    notifier.answer([Err(unreachable()), Ok(())]);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "email", json!({}), &[]).await;
    let id = mail(&world, channel.id, "ada@example.org", "ABCD-1234").await;
    world.outbox().run().await.unwrap();
    assert!(world.channel_now(channel.id).await.last_error.is_some());
    world.clock.advance(Duration::seconds(60));
    assert_eq!(world.outbox().run().await.unwrap().sent, 1);
    let channel = world.channel_now(channel.id).await;
    assert!(channel.last_error.is_none() && channel.last_ok_at == Some(world.now()));
    let row = world.row(id).await;
    assert_eq!(
        (row.state.as_str(), row.attempts, row.last_error),
        ("sent", 2, None)
    );
    let delivered = notifier.delivered();
    assert_eq!(
        (delivered[0].attempt, delivered[1].attempt),
        (1, 2),
        "the try is told to the notifier"
    );
    assert_eq!(
        delivered[0].id, delivered[1].id,
        "and the delivery is the same across the tries"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delayed_chat_notice_is_not_delivered_after_the_recipient_loses_access() {
    use orch_db::queries::chats::{self, NewChat, NewMember};

    let notifier = Scripted::new("chat", Reaches::Channel);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "chat", json!({}), &[]).await;
    let reader = world.person("reader").await;
    let org = world.org("Office").await;
    let chat = new_id();
    {
        let mut conn = world.test.pool.acquire().await.unwrap();
        chats::insert_chat(
            &mut conn,
            &NewChat {
                id: chat,
                kind: "group",
                org_id: org,
                direct_key: None,
                name: Some("Release"),
                created_by: world.admin,
                at: world.now(),
            },
        )
        .await
        .unwrap()
        .unwrap();
        chats::add_member(
            &mut conn,
            &NewMember {
                expires_at: None,
                chat_id: chat,
                user_id: reader,
                added_by: Some(world.admin),
                guest: true,
                seq: 1,
                at: world.now(),
            },
        )
        .await
        .unwrap();
    }
    let route = world.route_on(reader, &channel).await;
    let mut notice = world.snapshot("chat.message", "Private chat");
    notice.id = None;
    notice.chat_id = Some(chat);
    notice.body = "Private words".to_owned();
    world
        .queue_for_route(&route, "chat", &[notice.clone()])
        .await;
    assert_eq!(world.outbox().run().await.unwrap().sent, 1);
    assert_eq!(notifier.deliveries(), 1);

    let waiting = world.queue_for_route(&route, "chat", &[notice]).await;
    {
        let mut conn = world.test.pool.acquire().await.unwrap();
        assert!(
            chats::remove_member(&mut conn, chat, reader, 2, world.now())
                .await
                .unwrap()
        );
    }
    world.outbox().run().await.unwrap();
    assert_eq!(
        notifier.deliveries(),
        1,
        "no stale excerpt reaches the former member"
    );
    let row = world.row(waiting).await;
    assert_eq!(row.state, "cancelled");
    let reason: Option<String> =
        sqlx::query_scalar("SELECT cancel_reason FROM outbox WHERE id = $1")
            .bind(waiting)
            .fetch_one(&world.test.pool)
            .await
            .unwrap();
    assert_eq!(reason.as_deref(), Some("lost_access"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_delayed_thread_notice_is_not_delivered_after_the_recipient_loses_access() {
    use orch_db::queries::access::{self, PageKey, testing as access_testing};
    use orch_db::queries::hierarchy::testing::insert_node;

    let notifier = Scripted::new("chat", Reaches::Channel);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "chat", json!({}), &[]).await;
    let reader = world.person("thread-reader").await;
    let org = world.org("Office").await;
    let project = world.node("project", org, "Project").await;
    let topic = world.node("topic", project, "Topic").await;
    let thread = new_id();
    let grant = {
        let mut conn = world.test.pool.acquire().await.unwrap();
        insert_node(
            &mut conn,
            thread,
            "thread",
            Some(topic),
            "Private thread",
            world.admin,
        )
        .await
        .unwrap();
        access_testing::insert_member(&mut conn, org, reader, org)
            .await
            .unwrap();
        access_testing::add_entry(
            &mut conn,
            &PageKey::new("node", thread, "live"),
            "any",
            "thread.view",
            ("user", Some(reader), None),
            None,
            world.now(),
        )
        .await
        .unwrap()
    };
    let route = world.route_on(reader, &channel).await;
    let mut notice = world.snapshot("turn.completed", "Private thread: turn completed");
    notice.id = None;
    notice.thread_id = Some(thread);
    notice.node_id = Some(thread);
    notice.body = "Private agent result".to_owned();
    world
        .queue_for_route(&route, "chat", &[notice.clone()])
        .await;
    assert_eq!(world.outbox().run().await.unwrap().sent, 1);
    assert_eq!(notifier.deliveries(), 1);
    let waiting = world.queue_for_route(&route, "chat", &[notice]).await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    assert!(
        access::remove_entry(&mut conn, grant, world.admin, world.now())
            .await
            .unwrap()
    );
    drop(conn);
    world.outbox().run().await.unwrap();
    assert_eq!(
        notifier.deliveries(),
        1,
        "no stale thread text reaches the former reader"
    );
    assert_eq!(world.row(waiting).await.state, "cancelled");
}

#[tokio::test(flavor = "multi_thread")]
async fn outward_machine_outage_notices_omit_unscoped_thread_names_but_keep_certificate_details() {
    let notifier = Scripted::new("chat", Reaches::Channel);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "chat", json!({}), &[]).await;
    let reader = world.person("machine-owner").await;
    let route = world.route_on(reader, &channel).await;
    let mut outage = world.snapshot("machine.offline", "Machine went offline");
    outage.id = None;
    outage.body = "Secret thread and 3 more were running".to_owned();
    world.queue_for_route(&route, "chat", &[outage]).await;
    assert_eq!(world.outbox().run().await.unwrap().sent, 1);
    assert!(!notifier.delivered()[0].body.contains("Secret thread"));
    let mut certificate = world.snapshot("machine.offline", "Certificate expires");
    certificate.id = None;
    certificate.body = "Certificate renewal information".to_owned();
    certificate.data = json!({"reason":"certificate_expiring"});
    world.queue_for_route(&route, "chat", &[certificate]).await;
    assert_eq!(world.outbox().run().await.unwrap().sent, 1);
    assert!(
        notifier.delivered()[1]
            .body
            .contains("Certificate renewal information")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_time_the_far_side_names_replaces_the_pause_of_the_schedule_and_what_cannot_succeed_fails_at_once()
 {
    let notifier = Scripted::new("email", Reaches::Email);
    notifier.answer([
        Err(DeliverError::after("rate_limited", "slow down", 7)),
        Err(DeliverError::refused("rejected", "the message was refused")),
    ]);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "email", json!({}), &[]).await;
    let slow = mail(&world, channel.id, "ada@example.org", "AAAA-1111").await;
    world.outbox().run().await.unwrap();
    assert_eq!(
        world.row(slow).await.next_attempt_at,
        world.now() + Duration::seconds(7)
    );
    world.clock.advance(Duration::seconds(7));
    let refused = world.outbox().run().await.unwrap();
    assert_eq!((refused.failed, refused.retried), (1, 0));
    assert_eq!(world.row(slow).await.state, "failed");
    assert_eq!(world.row(slow).await.attempts, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_workers_never_deliver_one_row_twice() {
    let notifier = Scripted::new("email", Reaches::Email);
    notifier.takes(std::time::Duration::from_millis(25));
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "email", json!({}), &[]).await;
    let mut ids = BTreeSet::new();
    for n in 0..40 {
        ids.insert(
            mail(
                &world,
                channel.id,
                &format!("person{n}@example.org"),
                &format!("CODE-{n:04}"),
            )
            .await,
        );
    }
    let (first, second) = (world.outbox(), world.outbox());
    let (a, b) = tokio::join!(first.run(), second.run());
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.sent + b.sent, 40);
    assert!(a.sent > 0 && b.sent > 0, "both took a share: {a:?} {b:?}");
    let delivered = notifier.delivered();
    assert_eq!(delivered.len(), 40, "every row was delivered");
    let once: BTreeSet<Id> = delivered.iter().map(|each| each.id).collect();
    assert_eq!(once, ids, "and none twice");
    assert!(notifier.most_at_once() > 1, "deliveries overlapped");
    assert!(
        world
            .rows()
            .await
            .iter()
            .all(|row| row.state == "sent" && row.attempts == 1)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claim_left_by_a_dead_worker_is_taken_up() {
    let notifier = Scripted::new("email", Reaches::Email);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "email", json!({}), &[]).await;
    let id = mail(&world, channel.id, "ada@example.org", "ABCD-1234").await;

    // A worker claims the row and then dies without letting go of it.
    let db = world.db();
    let mut dead = db_outbox::begin_claim(&db, std::time::Duration::from_secs(900))
        .await
        .unwrap();
    assert!(
        db_outbox::claim_due(&mut dead, world.now())
            .await
            .unwrap()
            .is_some()
    );
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *dead)
        .await
        .unwrap();

    assert_eq!(
        world.outbox().run().await.unwrap().total(),
        0,
        "while it holds the row nobody else delivers it"
    );
    assert_eq!(notifier.deliveries(), 0);

    let mut conn = world.test.pool.acquire().await.unwrap();
    assert!(db_outbox::end_session(&mut conn, pid).await.unwrap());
    let done = world.outbox().run().await.unwrap();
    assert_eq!(
        done.sent, 1,
        "once its session is gone the row is delivered"
    );
    assert_eq!(world.row(id).await.state, "sent");
    assert_eq!(notifier.deliveries(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_target_that_is_gone_switches_its_route_off_and_forgets_where_it_was_linked() {
    let notifier = Scripted::new("chat", Reaches::Link);
    notifier.answer([Err(DeliverError::gone(
        "chat_not_found",
        "the person stopped the bot",
    ))]);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "chat", json!({}), &[]).await;
    let (ada, bob) = (world.person("ada").await, world.person("bob").await);
    let linked = |chat: i64| json!({ "chat_id": chat, "label": "Ada" });
    let route = world.route_on(ada, &channel).await;
    let other = world.route_on(bob, &channel).await;
    sqlx::query("UPDATE notification_routes SET address = $2, linked_at = now() WHERE id = $1")
        .bind(route.id)
        .bind(linked(1))
        .execute(&world.test.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE notification_routes SET address = $2, linked_at = now() WHERE id = $1")
        .bind(other.id)
        .bind(linked(2))
        .execute(&world.test.pool)
        .await
        .unwrap();
    let first = world
        .queue_for_route(&route, "chat", &[world.snapshot("turn.completed", "One")])
        .await;
    let second = world
        .queue_for_route(&route, "chat", &[world.snapshot("turn.completed", "Two")])
        .await;
    let others = world
        .queue_for_route(&other, "chat", &[world.snapshot("turn.completed", "Three")])
        .await;

    // One at a time, in the order they were queued.
    let outbox = world.outbox();
    let mut done = orch_worker_core::outbox::Run::default();
    while let Some(one) = outbox.deliver_next().await.unwrap() {
        done = orch_worker_core::outbox::Run {
            sent: done.sent + one.sent,
            retried: done.retried + one.retried,
            failed: done.failed + one.failed,
            gone: done.gone + one.gone,
            held: done.held + one.held,
        };
    }
    assert_eq!(done.gone, 1);
    assert_eq!(
        done.failed, 0,
        "work abandoned after the route switches off is cancelled without a failed attempt"
    );
    assert_eq!(done.sent, 1, "the other person's route is not touched");
    let route = world.route_now(route.id).await;
    assert!(!route.enabled, "the route is off");
    assert_eq!(route.address, json!({}), "and unlinked");
    assert!(route.linked_at.is_none());
    assert_eq!(world.row(first).await.state, "failed");
    let abandoned = world.row(second).await;
    assert_eq!(abandoned.state, "cancelled");
    assert_eq!(
        abandoned.attempts, 0,
        "the switched-off route is never called again"
    );
    assert!(
        abandoned.last_error.is_none(),
        "cancellation is no notifier failure"
    );
    let reason: Option<String> =
        sqlx::query_scalar("SELECT cancel_reason FROM outbox WHERE id = $1")
            .bind(second)
            .fetch_one(&world.test.pool)
            .await
            .unwrap();
    assert_eq!(reason.as_deref(), Some("route_off"));
    assert_eq!(world.row(others).await.state, "sent");
    assert!(world.route_now(other.id).await.enabled);
    let delivered = notifier.delivered();
    assert_eq!(
        delivered[0].to,
        linked(1),
        "the bot is told the chat of the route"
    );
    assert!(delivered.iter().any(|each| each.to == linked(2)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_destination_whose_place_is_gone_is_switched_off() {
    let notifier = Scripted::new("hook", Reaches::Channel);
    notifier.answer([Err(DeliverError::gone("not_found", "the room was deleted"))]);
    let world = World::new(only(&notifier)).await;
    let org = world.org("Acme").await;
    let destination = {
        let mut conn = world.test.pool.acquire().await.unwrap();
        channels::insert_destination(
            &mut conn,
            &NewDestination {
                id: new_id(),
                node_id: org,
                channel: "hook",
                name: "Ops room",
                config: &json!({}),
                templates: &json!({}),
                kinds: &[],
                level: "important",
                enabled: true,
                created_by: world.admin,
                at: world.now(),
            },
        )
        .await
        .unwrap()
    };
    let mut conn = world.test.pool.acquire().await.unwrap();
    let snapshot = serde_json::from_value(json!({
        "kind": "turn.failed", "at": world.now(), "title": "Build failed", "body": "", "node_id": org, "data": {}
    }))
    .unwrap();
    let row = new_id();
    db_outbox::insert(
        &mut conn,
        &db_outbox::NewRow {
            id: row,
            channel: "hook",
            route_id: None,
            destination_id: Some(destination.id),
            channel_id: None,
            payload: &db_outbox::Payload::of(&[snapshot]).to_json(),
            at: world.now(),
        },
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(world.outbox().run().await.unwrap().gone, 1);
    let mut conn = world.test.pool.acquire().await.unwrap();
    let destination = channels::destination(&mut conn, destination.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!destination.enabled);
    assert_eq!(world.row(row).await.state, "failed");
    let delivered = notifier.delivered();
    assert!(
        delivered[0].recipient.is_none(),
        "a place is told for nobody in particular"
    );
    assert_eq!(
        delivered[0].notifications[0]
            .node
            .as_ref()
            .unwrap()
            .name
            .as_deref(),
        Some("Acme")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_route_or_a_channel_that_was_switched_off_since_the_row_was_written_sends_nothing() {
    let notifier = Scripted::new("chat", Reaches::Channel);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "chat", json!({}), &[]).await;
    let ada = world.person("ada").await;
    let route = world.route_on(ada, &channel).await;
    let off_route = world
        .queue_for_route(&route, "chat", &[world.snapshot("turn.completed", "One")])
        .await;
    world
        .route(
            ada,
            &channel,
            RouteChange {
                enabled: Some(false),
                ..RouteChange::default()
            },
        )
        .await;
    assert_eq!(
        world.outbox().run().await.unwrap().total(),
        0,
        "a route switched off is no failure of the delivery: it is given up"
    );
    assert_eq!(world.row(off_route).await.state, "cancelled");

    world.route_on(ada, &channel).await;
    let off_channel = world
        .queue_for_route(&route, "chat", &[world.snapshot("turn.completed", "Two")])
        .await;
    sqlx::query("UPDATE notification_channels SET enabled = false WHERE id = $1")
        .bind(channel.id)
        .execute(&world.test.pool)
        .await
        .unwrap();
    assert_eq!(world.outbox().run().await.unwrap().failed, 1);
    assert!(
        world
            .row(off_channel)
            .await
            .last_error
            .unwrap()
            .contains("channel is switched off")
    );
    assert_eq!(notifier.deliveries(), 0, "nothing left the application");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_notifier_of_a_json_medium_is_given_the_notifications_and_a_body_only_when_the_channel_has_a_text()
 {
    let notifier = Scripted::with_body("hook", Reaches::Channel, BodyKind::Json);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "hook", json!({}), &[]).await;
    let ada = world.person("ada").await;
    let route = world.route_on(ada, &channel).await;
    world
        .queue_for_route(
            &route,
            "hook",
            &[world.snapshot("turn.completed", "Say \"hi\"")],
        )
        .await;
    world.outbox().run().await.unwrap();
    sqlx::query("UPDATE notification_channels SET templates = $2 WHERE id = $1")
        .bind(channel.id)
        .bind(
            json!({ "*": { "single": { "subject": null, "body": "{\"text\": \"{{title}}\"}" } } }),
        )
        .execute(&world.test.pool)
        .await
        .unwrap();
    world
        .queue_for_route(
            &route,
            "hook",
            &[world.snapshot("turn.completed", "Say \"hi\"")],
        )
        .await;
    world.outbox().run().await.unwrap();
    let delivered = notifier.delivered();
    assert_eq!(
        delivered[0].body, "",
        "without a text of its own the notifier builds the body itself"
    );
    assert_eq!(delivered[0].notifications.len(), 1);
    let own: serde_json::Value = serde_json::from_str(&delivered[1].body).unwrap();
    assert_eq!(
        own["text"], "Say \"hi\"",
        "with one, what it renders, with every value escaped for JSON"
    );
    assert_eq!(delivered[1].recipient.as_ref().unwrap().username, "ada");
}

#[tokio::test(flavor = "multi_thread")]
async fn while_the_far_side_answers_the_delivery_is_reserved_and_nothing_of_the_database_is_held() {
    let notifier = Scripted::new("email", Reaches::Email);
    notifier.takes(std::time::Duration::from_millis(600));
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "email", json!({}), &[]).await;
    let id = mail(&world, channel.id, "ada@example.org", "ABCD-1234").await;

    let outbox = world.outbox();
    let running = tokio::spawn(async move { outbox.run().await.unwrap() });
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    let mut probe = sqlx::Connection::begin(&mut *conn).await.unwrap();
    let state: String =
        sqlx::query_scalar("SELECT state FROM outbox WHERE id = $1 FOR UPDATE NOWAIT")
            .bind(id)
            .fetch_one(&mut *probe)
            .await
            .expect("no lock is held on the delivery while the notifier works");
    assert_eq!(state, "sending", "it is reserved for one attempt");
    probe.rollback().await.unwrap();
    drop(conn);
    assert_eq!(running.await.unwrap().sent, 1);
    let row = world.row(id).await;
    assert_eq!((row.state.as_str(), row.attempts), ("sent", 1));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_attempt_whose_worker_ended_goes_back_to_its_retries_after_its_deadline_and_counts() {
    let notifier = Scripted::new("email", Reaches::Email);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "email", json!({}), &[]).await;
    let id = mail(&world, channel.id, "ada@example.org", "ABCD-1234").await;
    // A worker reserved it, and ended before it said what came of it.
    let mut conn = world.test.pool.acquire().await.unwrap();
    let gone_worker = new_id();
    let deadline = world.now() + Duration::minutes(6);
    assert!(
        db_outbox::reserve(&mut conn, id, gone_worker, deadline, world.now())
            .await
            .unwrap()
            .is_some()
    );
    drop(conn);

    assert_eq!(
        world.outbox().run().await.unwrap().total(),
        0,
        "a reservation is left alone until its deadline"
    );
    assert_eq!(notifier.deliveries(), 0);
    world.clock.advance(Duration::minutes(6));
    let done = world.outbox().run().await.unwrap();
    assert_eq!(done.sent, 1, "after its deadline it is tried again");
    let row = world.row(id).await;
    assert_eq!(
        (row.state.as_str(), row.attempts),
        ("sent", 2),
        "the attempt nobody confirmed counts: it may have gone"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_late_answer_changes_nothing_once_its_attempt_was_given_up() {
    let notifier = Scripted::new("email", Reaches::Email);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "email", json!({}), &[]).await;
    let id = mail(&world, channel.id, "ada@example.org", "ABCD-1234").await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    let attempt = db_outbox::reserve(
        &mut conn,
        id,
        new_id(),
        world.now() + Duration::minutes(6),
        world.now(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        db_outbox::cancel(&mut conn, id, "lost_access")
            .await
            .unwrap()
    );
    assert!(
        !db_outbox::record(
            &mut conn,
            id,
            attempt,
            &db_outbox::Settle::Sent { payload: None }
        )
        .await
        .unwrap(),
        "the answer of an attempt that was given up is not recorded"
    );
    assert_eq!(world.row(id).await.state, "cancelled");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_route_whose_message_is_refused_for_good_tells_its_person_once_and_only_in_the_inbox() {
    let notifier = Scripted::new("chat", Reaches::Channel);
    notifier.answer([
        Err(DeliverError::refused("forbidden", "the hook said no")),
        Err(DeliverError::refused("forbidden", "the hook said no again")),
    ]);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "chat", json!({}), &[]).await;
    let ada = world.person("ada").await;
    let route = world.route_on(ada, &channel).await;
    for title in ["One", "Two"] {
        world
            .queue_for_route(&route, "chat", &[world.snapshot("turn.failed", title)])
            .await;
        assert_eq!(world.outbox().run().await.unwrap().failed, 1);
    }
    let kept = world.route_now(route.id).await;
    assert_eq!(kept.health, "failed");
    assert!(kept.failure_episode.is_some(), "the route is in a failure");
    let notices: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT kind, data FROM notifications WHERE user_id = $1 AND kind = 'notification.route_attention'",
    )
    .bind(ada)
    .fetch_all(&world.test.pool)
    .await
    .unwrap();
    assert_eq!(
        notices.len(),
        1,
        "one notice for the failure, however often it fails"
    );
    assert_eq!(notices[0].1["reason"], "exhausted");
    assert!(
        !notices[0].1.to_string().contains("said no"),
        "nothing of what the far side or a message said"
    );
    let routed: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox WHERE route_id = $1")
        .bind(route.id)
        .fetch_one(&world.test.pool)
        .await
        .unwrap();
    assert_eq!(routed, 2, "the notice itself goes through no route");
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_a_channel_keeps_its_routes_as_records_cancels_what_waited_and_tells_whoever_it_mattered_to()
 {
    let notifier = Scripted::new("chat", Reaches::Channel);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "chat", json!({}), &[]).await;
    let (ada, bob) = (world.person("ada").await, world.person("bob").await);
    let on = world.route_on(ada, &channel).await;
    let off = world.route(bob, &channel, RouteChange::default()).await;
    let waiting = world
        .queue_for_route(&on, "chat", &[world.snapshot("turn.completed", "Late")])
        .await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    assert!(
        channels::delete_channel(&mut conn, channel.id, world.now())
            .await
            .unwrap()
    );
    drop(conn);
    let kept = world.route_now(on.id).await;
    assert_eq!(kept.channel_id, None);
    assert_eq!(kept.health, "channel_removed");
    assert!(!kept.enabled);
    assert_eq!(kept.channel_snapshot["name"], channel.name.as_str());
    assert_eq!(world.row(waiting).await.state, "cancelled");
    let told: Vec<Id> = sqlx::query_scalar(
        "SELECT user_id FROM notifications WHERE kind = 'notification.route_attention' ORDER BY user_id",
    )
    .fetch_all(&world.test.pool)
    .await
    .unwrap();
    assert_eq!(
        told,
        [ada],
        "a route that was off and had nothing waiting tells nobody"
    );
    assert_eq!(world.route_now(off.id).await.health, "channel_removed");
}
