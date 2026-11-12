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

#[tokio::test]
async fn time_alone_does_not_restore_an_outside_entry_rejected_by_a_tighter_bound() {
    let w = World::new().await;
    let guest = w.user("guest").await;
    let local = w.member("local").await;
    name(&w, guest, Some(w.later(Duration::days(90))))
        .await
        .unwrap();
    name(&w, local, None).await.unwrap();
    assert!(w.can(guest, w.thread, P::ThreadView).await);

    let original: Vec<(Id, String, Option<chrono::DateTime<chrono::Utc>>, i64)> = sqlx::query_as(
        "SELECT id, permission, expires_at, revision FROM permission_entries
             WHERE object_kind='node' AND object_id=$1 AND holder_kind='user'
               AND holder_id=$2 AND removed_at IS NULL ORDER BY id",
    )
    .bind(w.thread)
    .bind(guest)
    .fetch_all(&w.db.pool)
    .await
    .unwrap();
    assert!(!original.is_empty());
    bound(&w, w.root, 30).await;
    assert!(!w.can(guest, w.thread, P::ThreadView).await);
    let legal = w.user("legal").await;
    name(&w, legal, Some(w.later(Duration::days(20))))
        .await
        .unwrap();
    assert!(w.can(legal, w.thread, P::ThreadView).await);

    let later = w.later(Duration::days(60));
    assert!(w.can_at(local, w.thread, P::ThreadView, later).await);
    assert!(!w.can_at(legal, w.thread, P::ThreadView, later).await);
    let unchanged: Vec<(Id, String, Option<chrono::DateTime<chrono::Utc>>, i64)> = sqlx::query_as(
        "SELECT id, permission, expires_at, revision FROM permission_entries
             WHERE object_kind='node' AND object_id=$1 AND holder_kind='user'
               AND holder_id=$2 AND removed_at IS NULL ORDER BY id",
    )
    .bind(w.thread)
    .bind(guest)
    .fetch_all(&w.db.pool)
    .await
    .unwrap();
    assert_eq!(
        unchanged, original,
        "the entry was neither repaired nor replaced"
    );
    assert!(
        !w.can_at(guest, w.thread, P::ThreadView, later).await,
        "time alone must not restore the overlong outside entry"
    );

    let repaired_end = w.later(Duration::days(65));
    let rows: Vec<Value> = original
        .iter()
        .map(|(id, permission, _, _)| {
            json!({ "permission": permission,
                "update": [{ "entry_id": id, "expires_at": repaired_end }] })
        })
        .collect();
    w.change_at(
        Caller::session(w.maker),
        Target::Live(w.thread),
        json!({ "rows": rows }),
        outsiders_named(),
        later,
    )
    .await
    .unwrap();
    assert!(w.can_at(guest, w.thread, P::ThreadView, later).await);
    let repaired: Vec<(Id, String, Option<chrono::DateTime<chrono::Utc>>, i64)> = sqlx::query_as(
        "SELECT id, permission, expires_at, revision FROM permission_entries
             WHERE object_kind='node' AND object_id=$1 AND holder_kind='user'
               AND holder_id=$2 AND removed_at IS NULL ORDER BY id",
    )
    .bind(w.thread)
    .bind(guest)
    .fetch_all(&w.db.pool)
    .await
    .unwrap();
    assert_eq!(repaired.len(), original.len());
    for (before, after) in original.iter().zip(&repaired) {
        assert_eq!((after.0, &after.1), (before.0, &before.1));
        assert_eq!(after.2, Some(repaired_end.get()));
        assert!(
            after.3 > before.3,
            "the explicit repair changes its revision"
        );
    }
}

#[tokio::test]
async fn a_foreign_places_page_only_viewers_obey_the_external_lifetime_bound() {
    use orch_access_contract::pages::Holder;
    use orch_access_contract::roles::Role;
    use orch_authz::engine::{Frame, Graph};
    use orch_db::queries::access::testing::{insert_member, name_owner};
    let w = World::new().await;
    let guest = w.user("guest").await;
    let foreign_owner = w.user("foreignowner").await;
    let foreign_root = orch_types::new_id();
    let mut conn = w.conn().await;
    orch_db::queries::hierarchy::testing::insert_node(
        &mut conn,
        foreign_root,
        "org",
        None,
        "Other root",
        foreign_owner,
    )
    .await
    .unwrap();
    name_owner(
        &mut conn,
        foreign_root,
        ("user", foreign_owner),
        w.now.get(),
    )
    .await
    .unwrap();
    insert_member(&mut conn, foreign_root, foreign_owner, foreign_root)
        .await
        .unwrap();
    drop(conn);
    let foreign_project = w
        .node_owned_by("project", foreign_root, "Other work", foreign_owner)
        .await;
    let foreign_place = w
        .node_owned_by("topic", foreign_project, "Page viewers", foreign_owner)
        .await;
    let local_target = w.node("thread", w.topic, "Local place viewers").await;
    w.change_as(
        Caller::session(foreign_owner),
        Target::Live(foreign_place),
        json!({"rows":[{"permission":"topic.view","add":[
            {"holder":user_holder(guest)}, {"holder":user_holder(w.maker)}]}]}),
        outsiders_named(),
    )
    .await
    .unwrap();
    w.change_as(
        Caller::session(w.boss),
        Target::Live(w.topic),
        json!({"rows":[{"permission":"topic.view","add":[
            {"holder":user_holder(guest),"expires_at":w.later(Duration::days(20))},
            {"holder":user_holder(w.maker)}]}]}),
        outsiders_named(),
    )
    .await
    .unwrap();
    for person in [guest, w.maker] {
        assert!(w.can(person, foreign_place, P::TopicView).await);
        assert!(
            !w.can(person, foreign_place, P::TopicInteract).await,
            "source visibility uses View, never an Interact grant"
        );
    }
    assert!(w.can(guest, w.topic, P::TopicView).await);
    assert!(!w.can(guest, w.topic, P::TopicInteract).await);
    assert!(w.can(w.maker, w.topic, P::TopicView).await);
    assert!(!w.can(w.maker, w.topic, P::TopicInteract).await);
    let foreign_holder = Holder::Role {
        role: Role::Viewer,
        at: Some(foreign_place),
    };
    let local_holder = Holder::Role {
        role: Role::Viewer,
        at: Some(w.topic),
    };
    for (target, holder) in [(w.thread, foreign_holder), (local_target, local_holder)] {
        w.change_as(
            Caller::session(w.maker),
            Target::Live(target),
            json!({"rows":[{"permission":"thread.view","add":[{"holder":holder}]}]}),
            outsiders_named(),
        )
        .await
        .unwrap();
        assert!(w.can(guest, target, P::ThreadView).await);
        assert!(!w.can(guest, target, P::ThreadInteract).await);
    }
    let mut conn = w.conn().await;
    let principal = orch_authz::Principal::load(&mut conn, Caller::session(guest), w.now)
        .await
        .unwrap();
    assert!(!principal.is_in_tree(w.root) && !principal.is_in_tree(foreign_root));
    let ids = [foreign_place, w.topic];
    let graph = Graph::load(&mut conn, &ids).await.unwrap();
    let sites: Vec<_> = ids
        .into_iter()
        .map(|id| graph.site_of(id).unwrap())
        .collect();
    let frame = Frame::load(
        &mut conn,
        &principal,
        Policy::default(),
        graph,
        &sites,
        &[P::TopicView, P::TopicInteract],
    )
    .await
    .unwrap();
    assert!(
        frame.role_at(foreign_place).is_none() && frame.role_at(w.topic).is_none(),
        "these viewers have page access, not a named or inherited role"
    );
    assert!(principal.is_held_by(&foreign_holder, |at| {
        at.and_then(|place| frame.role_at(place))
            .map(|held| held.role)
    }));
    drop(frame);
    drop(conn);
    bound(&w, w.root, 30).await;
    assert!(w.can(guest, foreign_place, P::TopicView).await);
    assert!(!w.can(guest, foreign_place, P::TopicInteract).await);
    assert!(w.can(guest, w.topic, P::TopicView).await);
    assert!(
        w.can(guest, local_target, P::ThreadView).await,
        "a local place's viewers are not an outside holder source"
    );
    assert!(!w.can(guest, w.thread, P::ThreadInteract).await);
    assert!(
        !w.can(guest, w.thread, P::ThreadView).await,
        "the foreign place's page-only viewers must obey the outside entry lifetime"
    );
}

#[tokio::test]
async fn expiry_changes_use_request_time_but_full_options_payloads_do_not_renew() {
    type Stamp = (
        Id,
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
        i64,
    );
    let w = World::new().await;
    let guest = w.user("guest").await;
    let provider = w.provider(w.maker, w.root, "Shared models").await;
    w.model(provider, "small", &[]).await;
    w.model(provider, "large", &[]).await;
    let target = Target::Resource(support::things::object("provider", provider));
    w.change_as(
        Caller::session(w.maker),
        target,
        json!({"rows":[{"permission":"provider.use","add":[{
            "holder":user_holder(guest),"options":{"models":["small"]},
            "expires_at":w.later(Duration::days(90))}]}]}),
        outsiders_named(),
    )
    .await
    .unwrap();
    let first: Stamp = sqlx::query_as(
        "SELECT id,created_at,renewed_at,expires_at,revision FROM permission_entries
         WHERE object_kind='provider' AND object_id=$1 AND permission='provider.use'
           AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL",
    )
    .bind(provider)
    .bind(guest)
    .fetch_one(&w.db.pool)
    .await
    .unwrap();
    assert_eq!(first.1, w.now.get());
    assert_eq!(first.2, None);
    bound(&w, w.root, 30).await;
    let at = w.later(Duration::days(60));
    let end = w.later(Duration::days(85));
    w.change_at(
        Caller::session(w.maker),
        target,
        json!({"rows":[{"permission":"provider.use","update":[{
            "entry_id":first.0,"expires_at":end}]}]}),
        outsiders_named(),
        at,
    )
    .await
    .unwrap();
    let renewed: Stamp = sqlx::query_as(
        "SELECT id,created_at,renewed_at,expires_at,revision FROM permission_entries WHERE id=$1",
    )
    .bind(first.0)
    .fetch_one(&w.db.pool)
    .await
    .unwrap();
    assert_eq!(
        (renewed.0, renewed.1, renewed.2, renewed.3),
        (first.0, first.1, Some(at.get()), Some(end.get()))
    );
    assert!(renewed.4 > first.4);
    let mut conn = w.conn().await;
    let principal = orch_authz::Principal::load(&mut conn, Caller::session(guest), at)
        .await
        .unwrap();
    assert!(
        orch_authz::resources::check(
            &mut conn,
            &principal,
            support::things::object("provider", provider),
            P::ProviderUse,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    assert!(
        !orch_authz::resources::check(
            &mut conn,
            &principal,
            support::things::object("provider", provider),
            P::ProviderManage,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed()
    );
    drop(conn);

    // Retained raw rows can carry sub-millisecond precision. This half-millisecond stays in
    // the same Time value and must not turn an options-only edit into an expiry renewal.
    sqlx::query("UPDATE permission_entries SET expires_at=expires_at+interval '500 microseconds' WHERE id=$1")
        .bind(first.0).execute(&w.db.pool).await.unwrap();
    let legacy: Stamp = sqlx::query_as(
        "SELECT id,created_at,renewed_at,expires_at,revision FROM permission_entries WHERE id=$1",
    )
    .bind(first.0)
    .fetch_one(&w.db.pool)
    .await
    .unwrap();
    assert_ne!(legacy.3, Some(end.get()));
    assert_eq!(legacy.3.map(Time::new), Some(end));
    w.change_at(
        Caller::session(w.maker),
        target,
        json!({"rows":[{"permission":"provider.use","update":[{
            "entry_id":first.0,"options":{"models":["large"]}}]}]}),
        outsiders_named(),
        w.later(Duration::days(79)),
    )
    .await
    .unwrap();
    let options_only: Stamp = sqlx::query_as(
        "SELECT id,created_at,renewed_at,expires_at,revision FROM permission_entries WHERE id=$1",
    )
    .bind(first.0)
    .fetch_one(&w.db.pool)
    .await
    .unwrap();
    assert_eq!(
        (options_only.0, options_only.1, options_only.2),
        (renewed.0, renewed.1, renewed.2)
    );
    assert_eq!(options_only.3.map(Time::new), Some(end));

    // The full options form sends the retained expiry too; that is not a renewal act.
    w.change_at(
        Caller::session(w.maker),
        target,
        json!({"rows":[{"permission":"provider.use","update":[{
            "entry_id":first.0,"options":{"models":["small","large"]},
            "note":"Options were changed","expires_at":end}]}]}),
        outsiders_named(),
        w.later(Duration::days(80)),
    )
    .await
    .unwrap();
    w.change_at(
        Caller::session(w.maker),
        target,
        json!({"rows":[{"permission":"provider.use","update":[{
            "entry_id":first.0,"note":"Only the note changed"}]}]}),
        outsiders_named(),
        w.later(Duration::days(81)),
    )
    .await
    .unwrap();
    let edited: Stamp = sqlx::query_as(
        "SELECT id,created_at,renewed_at,expires_at,revision FROM permission_entries WHERE id=$1",
    )
    .bind(first.0)
    .fetch_one(&w.db.pool)
    .await
    .unwrap();
    assert_eq!(
        (edited.0, edited.1, edited.2, edited.3),
        (renewed.0, renewed.1, renewed.2, renewed.3)
    );
    let noop = w
        .change_at(
            Caller::session(w.maker),
            target,
            json!({"rows":[{"permission":"provider.use","update":[{
            "entry_id":first.0,"expires_at":end}]}]}),
            outsiders_named(),
            w.later(Duration::days(82)),
        )
        .await
        .unwrap();
    assert!(!noop.changed, "same expiry remains the existing no-op");
    let unchanged: Stamp = sqlx::query_as(
        "SELECT id,created_at,renewed_at,expires_at,revision FROM permission_entries WHERE id=$1",
    )
    .bind(first.0)
    .fetch_one(&w.db.pool)
    .await
    .unwrap();
    assert_eq!(unchanged, edited);
    bound(&w, w.root, 10).await;
    let mut conn = w.conn().await;
    let principal = orch_authz::Principal::load(
        &mut conn,
        Caller::session(guest),
        w.later(Duration::days(83)),
    )
    .await
    .unwrap();
    assert!(
        !orch_authz::resources::check(
            &mut conn,
            &principal,
            support::things::object("provider", provider),
            P::ProviderUse,
            Policy::default()
        )
        .await
        .unwrap()
        .is_allowed(),
        "editing options did not restart the grant's lifetime"
    );
}

#[tokio::test]
async fn a_local_defaults_grant_keeps_independent_destination_bounds_after_membership_ends() {
    use orch_access_contract::roles::PlaceRef;
    use orch_authz::engine::{Frame, Graph};
    let w = World::new().await;
    let guest = w.member("departing").await;
    let strict_topic = w.node("topic", w.project, "Strict topic").await;
    let strict = w.node("thread", strict_topic, "Strict target").await;
    let loose = w.node("thread", w.topic, "Loose target").await;
    let end = w.later(Duration::days(20));
    let rows: Vec<Value> = ["managed", "connected"]
        .into_iter()
        .map(|profile| {
            json!({"permission":"thread.view","profile":profile,"add":[{
            "holder":user_holder(guest),"expires_at":end}]})
        })
        .collect();
    w.change(
        w.boss,
        Target::Defaults(PlaceRef::Node(w.project)),
        json!({"rows":rows}),
    )
    .await
    .unwrap();
    assert!(w.can(guest, strict, P::ThreadView).await);
    assert!(w.can(guest, loose, P::ThreadView).await);
    bound(&w, w.root, 30).await;
    bound(&w, strict_topic, 7).await;
    let mut conn = w.conn().await;
    orch_db::queries::access::testing::remove_member(&mut conn, w.root, guest)
        .await
        .unwrap();
    let principal = orch_authz::Principal::load(&mut conn, Caller::session(guest), w.now)
        .await
        .unwrap();
    assert!(!principal.is_in_tree(w.root));
    let ids = [strict, loose];
    let graph = Graph::load(&mut conn, &ids).await.unwrap();
    let sites: Vec<_> = ids
        .into_iter()
        .map(|id| graph.site_of(id).unwrap())
        .collect();
    let frame = Frame::load(
        &mut conn,
        &principal,
        Policy::default(),
        graph,
        &sites,
        &[P::ThreadView, P::ThreadInteract],
    )
    .await
    .unwrap();
    assert!(frame.ways(&sites[0], P::ThreadView).is_empty());
    assert!(
        !frame.ways(&sites[1], P::ThreadView).is_empty(),
        "the strict destination must not remove the shared ancestor entry from the looser one"
    );
    assert!(
        sites
            .iter()
            .all(|site| frame.ways(site, P::ThreadInteract).is_empty())
    );
}
