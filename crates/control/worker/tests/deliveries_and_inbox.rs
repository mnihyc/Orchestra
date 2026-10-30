//! Deliveries that are over go a week after their last try, or a month after one that failed for good, and an Inbox keeps its notifications for the days of its setting, read or not.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::{DateTime, Duration, Utc};
use orch_config::ServerConfig;
use orch_types::{Id, new_id};
use orch_worker_core::channels::Notifiers;
use orch_worker_core::purge;
use support::world::World;

/// A delivery in `state`, written at `written` and last due at `due`.
async fn delivery(world: &World, state: &str, written: DateTime<Utc>, due: DateTime<Utc>) -> Id {
    let id = new_id();
    sqlx::query(
        "INSERT INTO outbox (id, channel, payload, state, next_attempt_at, created_at)
         VALUES ($1, 'email', '{}', $2, $3, $4)",
    )
    .bind(id)
    .bind(state)
    .bind(due)
    .bind(written)
    .execute(&world.test.pool)
    .await
    .unwrap();
    id
}

/// A notification in the Inbox of `user`, written at `written`, and read then where `read`.
async fn notification(world: &World, user: Id, written: DateTime<Utc>, read: bool) -> Id {
    let id = new_id();
    sqlx::query(
        "INSERT INTO notifications (id, user_id, kind, title, dedupe_key, inbox_seq, created_at, read_at)
         VALUES ($1, $2, 'mention', 'Ada mentioned you', $3, 1, $4, $5)",
    )
    .bind(id)
    .bind(user)
    .bind(format!("mention:{id}"))
    .bind(written)
    .bind(read.then_some(written))
    .execute(&world.test.pool)
    .await
    .unwrap();
    id
}

async fn kept(world: &World, table: &str, id: Id) -> bool {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT EXISTS(SELECT 1 FROM {table} WHERE id = $1)"
    )))
    .bind(id)
    .fetch_one(&world.test.pool)
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn deliveries_that_are_over_go_after_a_week_or_after_a_month_when_they_failed_and_those_that_wait_stay()
 {
    let world = World::new(Notifiers::new()).await;
    let now = world.now();
    let (week, month, hour) = (Duration::days(7), Duration::days(30), Duration::hours(1));
    let sent_long_ago = delivery(&world, "sent", now - week - hour, now - week - hour).await;
    let sent_lately = delivery(&world, "sent", now - week + hour, now - week + hour).await;
    // Written a month ago, but tried again until lately: a week is counted from its last try.
    let retried_lately = delivery(&world, "sent", now - month, now - week + hour).await;
    let given_up = delivery(&world, "cancelled", now - week - hour, now - week - hour).await;
    let failed_weeks_ago = delivery(&world, "failed", now - week - hour, now - week - hour).await;
    let failed_long_ago = delivery(&world, "failed", now - month - hour, now - month - hour).await;
    let waiting = delivery(&world, "pending", now - month - hour, now - month - hour).await;

    let removed = purge::expired(&world.db(), &world.clock, &ServerConfig::default())
        .await
        .unwrap()
        .expect("the lock is free");
    assert_eq!(removed.deliveries, 3);
    for gone in [sent_long_ago, given_up, failed_long_ago] {
        assert!(
            !kept(&world, "outbox", gone).await,
            "a delivery over long enough ago goes"
        );
    }
    assert!(
        kept(&world, "outbox", sent_lately).await,
        "one that went this week stays"
    );
    assert!(
        kept(&world, "outbox", retried_lately).await,
        "one tried this week stays"
    );
    assert!(
        kept(&world, "outbox", failed_weeks_ago).await,
        "one that failed stays for a month, for its person to see why and retry it"
    );
    assert!(
        kept(&world, "outbox", waiting).await,
        "one that waits is never removed"
    );

    world.clock.advance(Duration::days(30));
    let later = purge::expired(&world.db(), &world.clock, &ServerConfig::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        later.deliveries, 3,
        "the rest that are over go in their turn"
    );
    assert!(kept(&world, "outbox", waiting).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_inbox_keeps_its_notifications_for_the_days_of_its_setting_read_or_not() {
    let world = World::new(Notifiers::new()).await;
    let now = world.now();
    let ada = world.person("ada").await;
    let old_read = notification(&world, ada, now - Duration::days(91), true).await;
    let old_unread = notification(&world, ada, now - Duration::days(91), false).await;
    let recent = notification(&world, ada, now - Duration::days(89), false).await;

    let config = ServerConfig::default();
    let removed = purge::expired(&world.db(), &world.clock, &config)
        .await
        .unwrap()
        .expect("the lock is free");
    assert_eq!(removed.inbox, 2, "ninety days as shipped");
    assert!(!kept(&world, "notifications", old_read).await);
    assert!(
        !kept(&world, "notifications", old_unread).await,
        "unread or not"
    );
    assert!(kept(&world, "notifications", recent).await);

    // A system admin keeps Inboxes for thirty days.
    sqlx::query(
        "INSERT INTO settings (node_id, key, value) VALUES (NULL, 'retention.inbox_days', '30')",
    )
    .execute(&world.test.pool)
    .await
    .unwrap();
    let month_old = notification(&world, ada, now - Duration::days(31), false).await;
    let fresh = notification(&world, ada, now - Duration::days(29), false).await;
    let removed = purge::expired(&world.db(), &world.clock, &config)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(removed.inbox, 2);
    assert!(!kept(&world, "notifications", recent).await);
    assert!(!kept(&world, "notifications", month_old).await);
    assert!(kept(&world, "notifications", fresh).await);
}
