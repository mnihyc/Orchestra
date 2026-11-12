//! An admitted delivery may finish after its channel is removed or its route is switched off.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_db::queries::notifications::{channels, people, routes};
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery};
use orch_notifier_contract::descriptor::{NotifierDescriptor, Reaches};
use orch_notifier_contract::notifier::Notifier;
use orch_types::{Json, new_id};
use orch_worker_core::channels::Notifiers;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Notify;

use support::fakes::Scripted;
use support::world::{Of, World};

// Scheduling headroom, matching the other worker database lock regressions.
const HEADROOM: std::time::Duration = std::time::Duration::from_secs(10);

struct Gated {
    inner: Arc<Scripted>,
    started: Notify,
    resume: Notify,
}

#[async_trait::async_trait]
impl Notifier for Gated {
    fn describe(&self) -> NotifierDescriptor {
        self.inner.describe()
    }

    async fn check(&self, config: &Config) -> Result<Json, DeliverError> {
        self.inner.check(config).await
    }

    async fn deliver(&self, delivery: Delivery) -> Result<(), DeliverError> {
        self.started.notify_one();
        self.resume.notified().await;
        self.inner.deliver(delivery).await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_in_flight_acceptance_preserves_channel_removal_and_route_off() {
    for remove_channel in [true, false] {
        let notifier = Arc::new(Gated {
            inner: Scripted::new("chat", Reaches::Channel),
            started: Notify::new(),
            resume: Notify::new(),
        });
        let mut notifiers = Notifiers::new();
        notifiers.register(notifier.clone()).unwrap();
        let world = World::new(notifiers).await;
        let channel = world.channel(Of::System, "chat", json!({}), &[]).await;
        let ada = world.person("ada").await;
        let route = world.route_on(ada, &channel).await;
        let first = world
            .queue_for_route(&route, "chat", &[world.snapshot("turn.completed", "One")])
            .await;
        let outbox = world.outbox();
        let delivery = outbox.clone();
        let running = tokio::spawn(async move { delivery.deliver_next().await });
        tokio::time::timeout(HEADROOM, notifier.started.notified())
            .await
            .expect("the first attempt is admitted before its native answer");
        let second = world
            .queue_for_route(&route, "chat", &[world.snapshot("turn.completed", "Two")])
            .await;
        let mut change = world.test.pool.begin().await.unwrap();
        tokio::time::timeout(HEADROOM, async {
            if remove_channel {
                assert!(
                    channels::delete_channel(&mut change, channel.id, world.now())
                        .await
                        .unwrap()
                );
            } else {
                channels::lock_channel_shared(&mut change, channel.id)
                    .await
                    .unwrap()
                    .unwrap();
                people::lock_prefs_shared(&mut change, ada).await.unwrap();
                routes::put_route(
                    &mut change,
                    new_id(),
                    ada,
                    channel.id,
                    &routes::RouteChange {
                        enabled: Some(false),
                        ..routes::RouteChange::default()
                    },
                    world.now(),
                )
                .await
                .unwrap();
            }
            change.commit().await.unwrap();
        })
        .await
        .expect("native I/O holds no database lock against changing the channel or route");
        let before_answer = world.route_now(route.id).await;
        assert!(!before_answer.enabled);
        notifier.resume.notify_one();
        let done = tokio::time::timeout(HEADROOM, running)
            .await
            .expect("the already admitted native answer can finish after the change")
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(done.sent, 1);
        let first_row = world.row(first).await;
        assert_eq!((first_row.state.as_str(), first_row.attempts), ("sent", 1));
        let after = world.route_now(route.id).await;
        assert!(!after.enabled, "acceptance never switches a route back on");
        assert_eq!(after.last_ok_at, Some(world.now()));
        if remove_channel {
            assert_eq!(after.channel_id, None);
            assert_eq!(after.health, "channel_removed");
            assert_eq!(after.last_error, before_answer.last_error);
            assert_eq!(after.failure_episode, before_answer.failure_episode);
            assert_eq!(after.failed_at, before_answer.failed_at);
        } else {
            assert_eq!(after.channel_id, Some(channel.id));
        }
        let remaining = tokio::time::timeout(HEADROOM, outbox.run())
            .await
            .expect("no new notifier call starts after the channel or route was stopped")
            .unwrap();
        assert_eq!(remaining.total(), 0);
        let second_row = world.row(second).await;
        assert_eq!(
            (second_row.state.as_str(), second_row.attempts),
            ("cancelled", 0)
        );
        let delivered = notifier.inner.delivered();
        assert_eq!(
            delivered.len(),
            1,
            "nothing new dispatches after the change"
        );
        assert_eq!(delivered[0].id, first);
        assert_eq!(delivered[0].notifications.len(), 1);
        let notices: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM notifications WHERE user_id=$1 AND kind='notification.route_attention'",
        )
        .bind(ada)
        .fetch_one(&world.test.pool)
        .await
        .unwrap();
        assert_eq!(notices, i64::from(remove_channel));
    }
}
