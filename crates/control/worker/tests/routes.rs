//! What waits for an unread delay or a frequency limit goes as one message when its time has come; quiet hours hold every route; a route starts from the present; and which channel carries which notification.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use async_trait::async_trait;
use chrono::{Duration, TimeZone, Utc};
use orch_db::queries::notifications::outbox as db_outbox;
use orch_db::queries::notifications::routes::{self, RouteChange};
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery};
use orch_notifier_contract::descriptor::{Needs, NotifierDescriptor, Reaches};
use orch_notifier_contract::notifier::Notifier;
use orch_types::Json;
use orch_worker_core::channels::Notifiers;
use orch_worker_core::routes::{Change, RouteError, serve, set};
use serde_json::json;
use std::sync::Arc;
use support::fakes::Scripted;
use support::world::{Of, World};
use tokio::sync::Notify;

/// Holds a notifier's answer until the test has changed the in-flight attempt.
struct GatedNotifier {
    inner: Arc<Scripted>,
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl Notifier for GatedNotifier {
    fn describe(&self) -> NotifierDescriptor {
        self.inner.describe()
    }

    async fn check(&self, config: &Config) -> Result<Json, DeliverError> {
        self.inner.check(config).await
    }

    async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError> {
        self.entered.notify_one();
        self.release.notified().await;
        self.inner.deliver(delivery).await
    }
}

fn only(notifier: &Arc<Scripted>) -> Notifiers {
    let mut notifiers = Notifiers::new();
    notifiers.register(notifier.clone()).unwrap();
    notifiers
}

fn delayed(minutes: i32) -> RouteChange<'static> {
    RouteChange {
        enabled: Some(true),
        unread_delay_minutes: Some(Some(minutes)),
        ..RouteChange::default()
    }
}

fn limited(minutes: i32) -> RouteChange<'static> {
    RouteChange {
        enabled: Some(true),
        min_interval_minutes: Some(Some(minutes)),
        ..RouteChange::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn database_races_retry_without_sending_a_notification_twice() {
    let notifier = Scripted::new("chat", Reaches::Channel);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "chat", json!({}), &[]).await;
    let ada = world.person("ada").await;
    let route = world.route_on(ada, &channel).await;
    let id = world
        .queue_for_route(
            &route,
            "chat",
            &[world.snapshot("turn.completed", "Finished")],
        )
        .await;

    // Sequences survive rollback, so each injected failure occurs exactly once.
    for statement in [
        "CREATE SEQUENCE reserve_race",
        "CREATE SEQUENCE conclusion_race",
        "CREATE FUNCTION outbox_races() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           IF NEW.state = 'sending' THEN
             IF nextval('reserve_race') = 1 THEN
               RAISE EXCEPTION 'reservation race' USING ERRCODE = '40001';
             END IF;
           ELSIF NEW.state = 'sent' THEN
             IF nextval('conclusion_race') = 1 THEN
               RAISE EXCEPTION 'conclusion race' USING ERRCODE = '40001';
             END IF;
           END IF;
           RETURN NEW;
         END $$",
        "CREATE TRIGGER outbox_races BEFORE UPDATE ON outbox
         FOR EACH ROW EXECUTE FUNCTION outbox_races()",
    ] {
        sqlx::query(statement)
            .execute(&world.test.pool)
            .await
            .unwrap();
    }

    let done = world.outbox().deliver_next().await.unwrap().unwrap();
    assert_eq!(done.sent, 1);
    assert_eq!(notifier.deliveries(), 1, "the remote call is not retried");
    let row = world.row(id).await;
    assert_eq!((row.state.as_str(), row.attempts), ("sent", 1));
    for sequence in ["reserve_race", "conclusion_race"] {
        let attempts: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT last_value FROM {sequence}"
        )))
        .fetch_one(&world.test.pool)
        .await
        .unwrap();
        assert_eq!(attempts, 2, "both transactions were retried");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_attempts_late_answer_cannot_change_its_route_or_channel() {
    let scripted = Scripted::new("chat", Reaches::Channel);
    scripted.answer([Err(DeliverError::gone("gone", "the target is gone"))]);
    let gated = Arc::new(GatedNotifier {
        inner: scripted,
        entered: Notify::new(),
        release: Notify::new(),
    });
    let mut notifiers = Notifiers::new();
    notifiers.register(gated.clone()).unwrap();
    let world = World::new(notifiers).await;
    let channel = world.channel(Of::System, "chat", json!({}), &[]).await;
    let ada = world.person("ada").await;
    let route = world.route_on(ada, &channel).await;
    let id = world
        .queue_for_route(
            &route,
            "chat",
            &[world.snapshot("turn.completed", "Finished")],
        )
        .await;

    let outbox = world.outbox();
    let running = tokio::spawn(async move { outbox.deliver_next().await });
    tokio::time::timeout(std::time::Duration::from_secs(10), gated.entered.notified())
        .await
        .expect("the notifier receives the reserved delivery");
    let mut conn = world.test.pool.acquire().await.unwrap();
    assert!(
        db_outbox::cancel(&mut conn, id, "lost_access")
            .await
            .unwrap()
    );
    drop(conn);
    let route_before = world.route_now(route.id).await;
    let channel_before = world.channel_now(channel.id).await;
    gated.release.notify_one();
    let done = running.await.unwrap().unwrap().unwrap();
    assert_eq!(done.total(), 0);
    assert_eq!(world.row(id).await.state, "cancelled");
    assert_eq!(world.route_now(route.id).await, route_before);
    assert_eq!(world.channel_now(channel.id).await, channel_before);
    let attention: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND kind = 'notification.route_attention'",
    )
    .bind(ada)
    .fetch_one(&world.test.pool)
    .await
    .unwrap();
    assert_eq!(attention, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_route_with_an_unread_delay_sends_one_message_for_three_notifications() {
    let notifier = Scripted::new("chat", Reaches::Channel);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "chat", json!({}), &[]).await;
    let ada = world.person("ada").await;
    let route = world.route(ada, &channel, delayed(15)).await;
    let read = world
        .notification(
            ada,
            "turn.completed",
            "Read in the app",
            None,
            Duration::minutes(30),
        )
        .await;
    sqlx::query("UPDATE notifications SET read_at = $2 WHERE id = $1")
        .bind(read.id)
        .bind(world.now())
        .execute(&world.test.pool)
        .await
        .unwrap();
    let mut told = Vec::new();
    for title in ["Turn 1 finished", "Turn 2 finished", "Turn 3 finished"] {
        told.push(
            world
                .notification(ada, "turn.completed", title, None, Duration::minutes(20))
                .await,
        );
    }
    let too_new = world
        .notification(
            ada,
            "turn.completed",
            "Only just now",
            None,
            Duration::minutes(5),
        )
        .await;

    let served = serve(&world.db(), &world.clock).await.unwrap();
    assert_eq!((served.routes, served.notifications), (1, 3));
    let rows = world.rows().await;
    assert_eq!(rows.len(), 1, "one row for three notifications");
    assert_eq!(rows[0].route_id, Some(route.id));
    assert_eq!(
        rows[0].payload["notifications"].as_array().unwrap().len(),
        3
    );
    let route = world.route_now(route.id).await;
    assert_eq!(
        route.sent_seq, told[2].inbox_seq,
        "past the three that went"
    );
    assert!(
        route.sent_seq < too_new.inbox_seq,
        "the one that is too new still waits"
    );

    assert_eq!(
        serve(&world.db(), &world.clock)
            .await
            .unwrap()
            .notifications,
        0,
        "nothing is told twice"
    );
    let done = world.outbox().run().await.unwrap();
    assert_eq!(done.sent, 1);
    let delivery = &notifier.delivered()[0];
    assert_eq!(delivery.notifications.len(), 3);
    assert_eq!(delivery.subject.as_deref(), Some("[Acme Orch] 3 unread"));
    assert!(!delivery.body.contains("Read in the app") && !delivery.body.contains("Only just now"));
    assert!(delivery.body.contains("Turn 2 finished"));

    world.clock.advance(Duration::minutes(11));
    let served = serve(&world.db(), &world.clock).await.unwrap();
    assert_eq!(
        served.notifications, 1,
        "it is old enough now, and goes as a message of its own"
    );
    world.outbox().run().await.unwrap();
    let last = &notifier.delivered()[1];
    assert_eq!(last.notifications.len(), 1);
    assert_eq!(last.subject.as_deref(), Some("[Acme Orch] Only just now"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_frequency_limit_lets_the_first_through_and_holds_what_comes_until_the_time_has_passed() {
    let notifier = Scripted::new("chat", Reaches::Channel);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "chat", json!({}), &[]).await;
    let ada = world.person("ada").await;
    world.route(ada, &channel, limited(30)).await;

    world
        .notification(ada, "turn.completed", "First", None, Duration::seconds(5))
        .await;
    assert_eq!(
        serve(&world.db(), &world.clock)
            .await
            .unwrap()
            .notifications,
        1,
        "the first goes at once"
    );
    world.outbox().run().await.unwrap();

    world.clock.advance(Duration::minutes(5));
    world
        .notification(ada, "turn.completed", "Second", None, Duration::seconds(1))
        .await;
    world.clock.advance(Duration::minutes(5));
    world
        .notification(ada, "turn.completed", "Third", None, Duration::seconds(1))
        .await;
    world.clock.advance(Duration::minutes(10));
    assert_eq!(
        serve(&world.db(), &world.clock)
            .await
            .unwrap()
            .notifications,
        0,
        "20 minutes after the first: not yet"
    );

    world.clock.advance(Duration::minutes(10));
    assert_eq!(
        serve(&world.db(), &world.clock)
            .await
            .unwrap()
            .notifications,
        2,
        "30 minutes after: both, together"
    );
    world.outbox().run().await.unwrap();
    let delivered = notifier.delivered();
    assert_eq!(delivered.len(), 2, "two messages in all, not three");
    assert_eq!(delivered[1].notifications.len(), 2);
    assert!(delivered[1].body.contains("Second") && delivered[1].body.contains("Third"));
}

#[tokio::test(flavor = "multi_thread")]
async fn quiet_hours_hold_a_message_until_they_end_and_what_was_held_goes_as_one_message() {
    let notifier = Scripted::new("chat", Reaches::Channel);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "chat", json!({}), &[]).await;
    // Ten at night to seven in the morning, in Berlin, which is two hours ahead of UTC.
    let ada = world
        .person_with("ada", "en", "Europe/Berlin", Some("ada@example.org"))
        .await;
    world.quiet_hours(ada, 22 * 60, 7 * 60).await;
    let route = world.route_on(ada, &channel).await;
    world
        .clock
        .set(Utc.with_ymd_and_hms(2026, 10, 8, 21, 30, 0).unwrap()); // 23:30 in Berlin

    let first = world
        .queue_for_route(
            &route,
            "chat",
            &[world.snapshot("turn.completed", "Late one")],
        )
        .await;
    let second = world
        .queue_for_route(
            &route,
            "chat",
            &[world.snapshot("approval.requested", "Another")],
        )
        .await;
    let done = world.outbox().run().await.unwrap();
    assert_eq!((done.held, done.sent), (2, 0), "{done:?}");
    assert_eq!(
        notifier.deliveries(),
        0,
        "nothing leaves the application during quiet hours"
    );
    let morning = Utc.with_ymd_and_hms(2026, 10, 9, 5, 0, 0).unwrap(); // 07:00 in Berlin
    for id in [first, second] {
        let row = world.row(id).await;
        assert_eq!(
            (row.state.as_str(), row.attempts, row.next_attempt_at),
            ("pending", 0, morning)
        );
    }
    world.clock.set(morning - Duration::minutes(1));
    assert_eq!(
        world.outbox().run().await.unwrap().total(),
        0,
        "still quiet a minute before"
    );

    world.clock.set(morning);
    let done = world.outbox().run().await.unwrap();
    assert_eq!(
        done.sent, 1,
        "what was held goes out as one message: {done:?}"
    );
    let delivered = notifier.delivered();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].notifications.len(), 2);
    assert_eq!(
        delivered[0].subject.as_deref(),
        Some("[Acme Orch] 2 unread")
    );
    assert!(world.rows().await.iter().all(|row| row.state == "sent"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_route_that_waits_sends_nothing_during_quiet_hours_and_everything_when_they_end() {
    let notifier = Scripted::new("chat", Reaches::Channel);
    let world = World::new(only(&notifier)).await;
    let channel = world.channel(Of::System, "chat", json!({}), &[]).await;
    let ada = world.person("ada").await;
    world.quiet_hours(ada, 22 * 60, 7 * 60).await;
    world.route(ada, &channel, delayed(15)).await;
    world
        .clock
        .set(Utc.with_ymd_and_hms(2026, 10, 8, 23, 0, 0).unwrap());
    world
        .notification(
            ada,
            "turn.completed",
            "At night",
            None,
            Duration::minutes(40),
        )
        .await;
    assert_eq!(
        serve(&world.db(), &world.clock)
            .await
            .unwrap()
            .notifications,
        0
    );
    world
        .clock
        .set(Utc.with_ymd_and_hms(2026, 10, 9, 6, 59, 0).unwrap());
    assert_eq!(
        serve(&world.db(), &world.clock)
            .await
            .unwrap()
            .notifications,
        0
    );
    world
        .notification(
            ada,
            "turn.completed",
            "Before dawn",
            None,
            Duration::minutes(20),
        )
        .await;
    world
        .clock
        .set(Utc.with_ymd_and_hms(2026, 10, 9, 7, 0, 0).unwrap());
    assert_eq!(
        serve(&world.db(), &world.clock)
            .await
            .unwrap()
            .notifications,
        2,
        "both, when the window ends"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_route_that_is_switched_on_starts_from_the_present_and_needs_what_its_channel_reaches_by()
{
    let notifier = Scripted::new("chat", Reaches::Channel);
    let mailer = Scripted::new("email", Reaches::Email);
    let bot = Scripted::new("bot", Reaches::Link);
    let mut notifiers = only(&notifier);
    notifiers.register(mailer.clone()).unwrap();
    notifiers.register(bot.clone()).unwrap();
    let world = World::new(notifiers).await;
    let chat = world.channel(Of::System, "chat", json!({}), &[]).await;
    let mail = world.channel(Of::System, "email", json!({}), &[]).await;
    let bot_channel = world.channel(Of::System, "bot", json!({}), &[]).await;
    let ada = world.person("ada").await;
    let nobody = world.person_with("nobody", "en", "UTC", None).await;

    world
        .notification(
            ada,
            "turn.completed",
            "From before",
            None,
            Duration::hours(3),
        )
        .await;
    let mut conn = world.test.pool.acquire().await.unwrap();
    let person = orch_db::queries::notifications::people::recipient(&mut conn, ada)
        .await
        .unwrap()
        .unwrap();
    let stranger = orch_db::queries::notifications::people::recipient(&mut conn, nobody)
        .await
        .unwrap()
        .unwrap();
    let on = Change {
        enabled: Some(true),
        unread_delay_minutes: Some(Some(5)),
        ..Change::default()
    };
    let route = set(
        &mut conn,
        &notifier.describe(),
        &person,
        &chat,
        &on,
        world.now(),
    )
    .await
    .unwrap();
    assert!(route.enabled);
    drop(conn);
    assert_eq!(
        serve(&world.db(), &world.clock)
            .await
            .unwrap()
            .notifications,
        0,
        "what came before is not told as if new"
    );
    world
        .notification(ada, "turn.completed", "After", None, Duration::minutes(10))
        .await;
    assert_eq!(
        serve(&world.db(), &world.clock)
            .await
            .unwrap()
            .notifications,
        1
    );

    let mut conn = world.test.pool.acquire().await.unwrap();
    let on = Change {
        enabled: Some(true),
        ..Change::default()
    };
    let lacks = |result: Result<_, RouteError>| match result {
        Err(RouteError::NotReady(needs)) => needs,
        other => panic!("expected a route that is not ready, got {other:?}"),
    };
    assert_eq!(
        lacks(
            set(
                &mut conn,
                &mailer.describe(),
                &stranger,
                &mail,
                &on,
                world.now()
            )
            .await
        ),
        Needs::VerifiedEmail
    );
    assert_eq!(
        lacks(
            set(
                &mut conn,
                &bot.describe(),
                &person,
                &bot_channel,
                &on,
                world.now()
            )
            .await
        ),
        Needs::Link
    );
    assert!(
        set(
            &mut conn,
            &mailer.describe(),
            &person,
            &mail,
            &on,
            world.now()
        )
        .await
        .unwrap()
        .enabled
    );
    let off = Change {
        enabled: Some(false),
        ..Change::default()
    };
    assert!(
        !set(
            &mut conn,
            &mailer.describe(),
            &stranger,
            &mail,
            &off,
            world.now()
        )
        .await
        .unwrap()
        .enabled,
        "off needs nothing"
    );

    for (change, field) in [
        (
            Change {
                unread_delay_minutes: Some(Some(0)),
                ..Change::default()
            },
            "unread_delay_minutes",
        ),
        (
            Change {
                min_interval_minutes: Some(Some(1441)),
                ..Change::default()
            },
            "min_interval_minutes",
        ),
        (
            Change {
                kinds: Some(vec!["no.such.kind".into()]),
                ..Change::default()
            },
            "kinds",
        ),
    ] {
        match set(
            &mut conn,
            &notifier.describe(),
            &person,
            &chat,
            &change,
            world.now(),
        )
        .await
        {
            Err(RouteError::Invalid { field: named, .. }) => assert_eq!(named, field),
            other => panic!("{field}: {other:?}"),
        }
    }
    let ended = orch_worker_core::routes::end(&mut conn, ada, chat.id, world.now())
        .await
        .unwrap();
    assert!(ended);
    let route = routes::route_for(&mut conn, ada, chat.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!route.enabled && route.address == json!({}));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_channel_of_a_root_org_carries_what_happens_in_its_tree_and_the_systems_mail_server_carries_the_rest()
 {
    let mailer = Scripted::new("email", Reaches::Email);
    let world = World::new(only(&mailer)).await;
    let (acme, globex) = (world.org("Acme").await, world.org("Globex").await);
    let acme_project = world.node("project", acme, "Web").await;
    let globex_project = world.node("project", globex, "Api").await;
    let ada = world.person("ada").await;
    let systems = world.channel(Of::System, "email", json!({}), &[]).await;
    let acmes = world.channel(Of::Org(acme), "email", json!({}), &[]).await;
    let hers = world.channel(Of::User(ada), "email", json!({}), &[]).await;
    let (on_system, on_acme, on_hers) = (
        world.route_on(ada, &systems).await,
        world.route_on(ada, &acmes).await,
        world.route_on(ada, &hers).await,
    );
    let mut conn = world.test.pool.acquire().await.unwrap();
    let who = |rows: Vec<routes::ImmediateRoute>| {
        let mut ids: Vec<_> = rows.into_iter().map(|row| row.route_id).collect();
        ids.sort();
        ids
    };
    let sorted = |mut ids: Vec<orch_types::Id>| {
        ids.sort();
        ids
    };

    // Acme has a mail server of its own and the system lets mail be shared.
    let at_acme = routes::immediate_routes(&mut conn, ada, "turn.completed", Some(acme), true)
        .await
        .unwrap();
    assert_eq!(
        who(at_acme),
        sorted(vec![on_acme.id, on_hers.id]),
        "Acme's own, and the person's own; not the system's"
    );
    let at_globex = routes::immediate_routes(&mut conn, ada, "turn.completed", Some(globex), true)
        .await
        .unwrap();
    assert_eq!(
        who(at_globex),
        sorted(vec![on_system.id, on_hers.id]),
        "Globex has none: the system's carries it"
    );
    let at_globex_unshared =
        routes::immediate_routes(&mut conn, ada, "turn.completed", Some(globex), false)
            .await
            .unwrap();
    assert_eq!(
        who(at_globex_unshared),
        vec![on_hers.id],
        "mail that may not be shared is not sent through the system's"
    );
    let nowhere = routes::immediate_routes(&mut conn, ada, "sign_in.attention", None, false)
        .await
        .unwrap();
    assert_eq!(
        who(nowhere),
        sorted(vec![on_system.id, on_hers.id]),
        "what lies in no org goes by the system's and the person's own"
    );

    // A route that takes only some kinds.
    world
        .route(
            ada,
            &hers,
            RouteChange {
                kinds: Some(&["approval.requested".to_owned()]),
                ..RouteChange::default()
            },
        )
        .await;
    let kinds = routes::immediate_routes(&mut conn, ada, "turn.completed", Some(globex), true)
        .await
        .unwrap();
    assert_eq!(who(kinds), vec![on_system.id]);

    // The routes job applies the same rule to what waits.
    drop(conn);
    world.route(ada, &systems, delayed(5)).await;
    world.route(ada, &acmes, delayed(5)).await;
    world
        .route(
            ada,
            &hers,
            RouteChange {
                kinds: Some(&[]),
                enabled: Some(false),
                ..RouteChange::default()
            },
        )
        .await;
    let at_acme = world
        .notification(
            ada,
            "turn.completed",
            "In Acme",
            Some(acme_project),
            Duration::minutes(10),
        )
        .await;
    let at_globex = world
        .notification(
            ada,
            "turn.completed",
            "In Globex",
            Some(globex_project),
            Duration::minutes(10),
        )
        .await;
    let _ = (at_acme, at_globex);
    let mut conn = world.test.pool.acquire().await.unwrap();
    let on_acme = routes::waiting_inbox(
        &mut conn,
        acmes_route(&world, ada, &acmes).await,
        None,
        true,
    )
    .await
    .unwrap();
    let titles: Vec<_> = on_acme.iter().map(|row| row.title.as_str()).collect();
    assert_eq!(titles, ["In Acme"]);
    let on_system = routes::waiting_inbox(
        &mut conn,
        acmes_route(&world, ada, &systems).await,
        None,
        true,
    )
    .await
    .unwrap();
    let titles: Vec<_> = on_system.iter().map(|row| row.title.as_str()).collect();
    assert_eq!(titles, ["In Globex"], "Acme has a mail server of its own");
}

async fn acmes_route(
    world: &World,
    user: orch_types::Id,
    channel: &orch_db::queries::notifications::channels::ChannelRow,
) -> orch_types::Id {
    let mut conn = world.test.pool.acquire().await.unwrap();
    routes::route_for(&mut conn, user, channel.id)
        .await
        .unwrap()
        .unwrap()
        .id
}
