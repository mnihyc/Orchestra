//! How long an entry for somebody from outside may run is bounded where a place says so: one without an end, or ending later than the bound from now, is refused with the latest end and the place that sets it; one within counts; and one that a stricter bound puts beyond it lets nobody in until it is given a nearer end, while the tree's own people are untouched.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_access_contract::permissions::Permission as P;
use orch_authz::pages::{ChangeError, Target};
use orch_authz::{Caller, Policy};
use orch_types::{Id, Time};
use serde_json::{Value, json};
use support::{World, user_holder};

/// Sets `sharing.external_max_days` to `days` at `place`.
async fn bound(w: &World, place: Id, days: i64) {
    sqlx::query(
        "INSERT INTO settings (node_id, key, value) VALUES ($1, 'sharing.external_max_days', $2)
         ON CONFLICT (node_id, key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(place)
    .bind(json!(days))
    .execute(&w.db.pool)
    .await
    .unwrap();
}

/// The owners of the thread may name somebody from outside.
fn outsiders_named() -> Policy {
    Policy {
        owners_name_outsiders: true,
        ..Policy::default()
    }
}

/// Names `who` on the thread for full view, until `end`.
fn naming(who: Id, end: Option<Time>) -> Value {
    json!({ "name": [{ "holders": [user_holder(who)], "choice": "full_view", "expires_at": end }] })
}

async fn name(w: &World, who: Id, end: Option<Time>) -> Result<(), ChangeError> {
    w.change_as(
        Caller::session(w.maker),
        Target::Live(w.thread),
        naming(who, end),
        outsiders_named(),
    )
    .await
    .map(|_| ())
}

#[tokio::test]
async fn an_outside_entry_runs_no_longer_than_the_bound_of_its_place_from_now() {
    let w = World::new().await;
    let guest = w.user("guest").await;
    bound(&w, w.project, 30).await;
    match name(&w, guest, None).await {
        Err(ChangeError::ExternalLifetime { latest, place }) => {
            assert_eq!(latest, w.later(Duration::days(30)));
            assert_eq!(place, Some(w.project), "the project sets the bound");
        }
        other => panic!("an entry without an end was not refused: {other:?}"),
    }
    assert!(matches!(
        name(&w, guest, Some(w.later(Duration::days(31)))).await,
        Err(ChangeError::ExternalLifetime { .. })
    ));
    assert!(!w.can(guest, w.thread, P::ThreadView).await);
    name(&w, guest, Some(w.later(Duration::days(30))))
        .await
        .unwrap();
    assert!(w.can(guest, w.thread, P::ThreadView).await);
    // The tree's own people are no outsiders: their entries need no end.
    let colleague = w.member("colleague").await;
    name(&w, colleague, None).await.unwrap();
    assert!(w.can(colleague, w.thread, P::ThreadView).await);
}

#[tokio::test]
async fn the_fewest_days_on_the_way_bound_it() {
    let w = World::new().await;
    let guest = w.user("guest").await;
    bound(&w, w.root, 60).await;
    bound(&w, w.topic, 14).await;
    match name(&w, guest, Some(w.later(Duration::days(20)))).await {
        Err(ChangeError::ExternalLifetime { latest, place }) => {
            assert_eq!(latest, w.later(Duration::days(14)));
            assert_eq!(place, Some(w.topic));
        }
        other => panic!("the topic's bound did not hold: {other:?}"),
    }
}

#[tokio::test]
async fn an_entry_a_stricter_bound_puts_beyond_it_lets_nobody_in_until_it_gets_a_nearer_end() {
    let w = World::new().await;
    let guest = w.user("guest").await;
    name(&w, guest, Some(w.later(Duration::days(20))))
        .await
        .unwrap();
    assert!(w.can(guest, w.thread, P::ThreadView).await);
    // The org now lets outside entries run a week: this one runs beyond that, and opens nothing.
    bound(&w, w.root, 7).await;
    assert!(!w.can(guest, w.thread, P::ThreadView).await);
    assert!(
        w.can(w.maker, w.thread, P::ThreadView).await,
        "the tree's own people are untouched"
    );
    // Given a nearer end, each of their entries counts again.
    let entries: Vec<(Id, String)> = sqlx::query_as(
        "SELECT id, permission FROM permission_entries
          WHERE holder_kind = 'user' AND holder_id = $1 AND removed_at IS NULL",
    )
    .bind(guest)
    .fetch_all(&w.db.pool)
    .await
    .unwrap();
    assert!(!entries.is_empty());
    let end = w.later(Duration::days(5));
    let rows: Vec<Value> = entries
        .iter()
        .map(|(id, permission)| {
            json!({ "permission": permission, "update": [{ "entry_id": id, "expires_at": end }] })
        })
        .collect();
    w.change_as(
        Caller::session(w.maker),
        Target::Live(w.thread),
        json!({ "rows": rows }),
        outsiders_named(),
    )
    .await
    .unwrap();
    assert!(w.can(guest, w.thread, P::ThreadView).await);
    // Renewing beyond the bound is refused like naming beyond it.
    let (id, permission) = &entries[0];
    let beyond = w
        .change_as(
            Caller::session(w.maker),
            Target::Live(w.thread),
            json!({ "rows": [{ "permission": permission,
                "update": [{ "entry_id": id, "expires_at": w.later(Duration::days(8)) }] }] }),
            outsiders_named(),
        )
        .await;
    assert!(
        matches!(beyond, Err(ChangeError::ExternalLifetime { .. })),
        "{beyond:?}"
    );
}
