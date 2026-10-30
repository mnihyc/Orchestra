//! A person's groups and places of page-only viewing, kept between requests, are exactly those worked out afresh
//! after every kind of change that can alter them, committed or not, and as roles and entries end.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_authz::{Caller, Principal};
use orch_db::queries::access::{self, PageKey, SYSTEM_ID, testing::*};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_types::{Id, Time, new_id};
use support::World;

/// What `access_holders` says now, without anything kept: the groups and the places of page-only viewing.
async fn afresh(
    conn: &mut orch_db::Conn,
    person: Id,
    at: Time,
    system: bool,
) -> (Vec<Id>, Vec<Id>) {
    let sets = access::effective_holder_sets(conn, person, at.get(), system)
        .await
        .unwrap();
    let pick = |kind: &str| {
        sets.iter()
            .filter(|(k, _)| k == kind)
            .map(|(_, id)| *id)
            .collect::<Vec<_>>()
    };
    (pick("group"), pick("visible"))
}

/// Every person, in an ordinary session and as an admin, twice (the second load may use what the first kept): the
/// principal's groups and viewing places are those worked out afresh on the same connection.
async fn same_as_afresh(
    conn: &mut orch_db::Conn,
    people: &[Id],
    places: &[Id],
    at: Time,
    step: &str,
) {
    for &person in people {
        for admin in [false, true] {
            for round in 0..2 {
                let principal =
                    Principal::load(conn, Caller::session(person).with_admin_mode(admin), at)
                        .await
                        .unwrap();
                let (groups, viewers) = afresh(conn, person, at, principal.is_system_admin()).await;
                assert_eq!(
                    principal.groups(),
                    groups.as_slice(),
                    "{step}: groups of {person} (admin {admin}, load {round})"
                );
                assert!(
                    viewers.iter().all(|place| places.contains(place)),
                    "{step}: a viewing place outside the world"
                );
                for place in places {
                    assert_eq!(
                        principal.is_viewer_of(*place),
                        viewers.contains(place),
                        "{step}: viewing of {place} by {person} (admin {admin}, load {round})"
                    );
                }
            }
        }
    }
}

async fn changes(conn: &mut orch_db::Conn) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM access_changes")
        .fetch_one(conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn kept_holder_sets_follow_every_change_that_can_alter_them() {
    let w = World::new().await;
    let at = w.now;
    let alice = w.member("alice").await;
    let outsider = w.user("outsider").await;
    let department = w.node("org", w.root, "Research").await;
    let inner = w.node("project", department, "Inner").await;
    let people = [w.boss, w.maker, alice, outsider];
    let places = [w.root, w.project, w.topic, w.thread, department, inner];
    let mut conn = w.conn().await;
    same_as_afresh(&mut conn, &people, &places, at, "as made").await;

    let group = insert_group(&mut conn, w.root, "Reviewers", w.boss)
        .await
        .unwrap();
    let outer = insert_group(&mut conn, w.root, "Everybody", w.boss)
        .await
        .unwrap();
    add_to_group(&mut conn, group, alice).await.unwrap();
    same_as_afresh(&mut conn, &people, &places, at, "a member of a group").await;
    sqlx::query("INSERT INTO group_members (group_id, member_group_id) VALUES ($1, $2)")
        .bind(outer)
        .bind(group)
        .execute(&mut *conn)
        .await
        .unwrap();
    same_as_afresh(&mut conn, &people, &places, at, "a group in a group").await;
    let by_role = insert_group(&mut conn, w.root, "Viewers of Research", w.boss)
        .await
        .unwrap();
    sqlx::query("INSERT INTO group_members (group_id, member_node_id, member_role) VALUES ($1, $2, 'viewer')")
        .bind(by_role).bind(department).execute(&mut *conn).await.unwrap();
    let role = give_role(
        &mut conn, department, "group", group, "viewer", w.boss, None,
    )
    .await
    .unwrap();
    same_as_afresh(&mut conn, &people, &places, at, "a role given to a group").await;

    let entry = add_entry(
        &mut conn,
        &PageKey::new("node", inner, "live"),
        "any",
        "project.view",
        ("user", Some(outsider), None),
        None,
        at.get(),
    )
    .await
    .unwrap();
    same_as_afresh(
        &mut conn,
        &people,
        &places,
        at,
        "an outsider named on a page",
    )
    .await;
    let everyone = add_entry(
        &mut conn,
        &PageKey::new("node", department, "defaults"),
        "any",
        "project.view",
        ("everyone", None, None),
        None,
        at.get(),
    )
    .await
    .unwrap();
    same_as_afresh(
        &mut conn,
        &people,
        &places,
        at,
        "everyone named on a defaults page",
    )
    .await;
    set_row(
        &mut conn,
        &PageKey::new("node", inner, "live"),
        "any",
        "project.view",
        false,
        false,
    )
    .await
    .unwrap();
    same_as_afresh(
        &mut conn,
        &people,
        &places,
        at,
        "a row that stops following",
    )
    .await;
    set_row(
        &mut conn,
        &PageKey::new("node", department, "defaults"),
        "any",
        "project.view",
        true,
        true,
    )
    .await
    .unwrap();
    same_as_afresh(&mut conn, &people, &places, at, "a locked default").await;
    add_entry(
        &mut conn,
        &PageKey::new("system", SYSTEM_ID, "defaults"),
        "any",
        "chat.view",
        ("members", Some(w.root), None),
        None,
        at.get(),
    )
    .await
    .unwrap();
    same_as_afresh(
        &mut conn,
        &people,
        &places,
        at,
        "the members of an org named on the system's defaults",
    )
    .await;
    add_entry(
        &mut conn,
        &PageKey::new("node", w.root, "defaults"),
        "any",
        "org.view",
        ("role", None, Some("interactor")),
        None,
        at.get(),
    )
    .await
    .unwrap();
    same_as_afresh(
        &mut conn,
        &people,
        &places,
        at,
        "a role named on an org's defaults",
    )
    .await;
    access::remove_entry(&mut conn, entry, w.boss, at.get())
        .await
        .unwrap();
    access::remove_entry(&mut conn, everyone, w.boss, at.get())
        .await
        .unwrap();
    same_as_afresh(&mut conn, &people, &places, at, "entries removed").await;
    take_role_away(&mut conn, role, w.boss, at.get())
        .await
        .unwrap();
    same_as_afresh(&mut conn, &people, &places, at, "a role taken away").await;
    name_owner(&mut conn, inner, ("group", outer), at.get())
        .await
        .unwrap();
    same_as_afresh(&mut conn, &people, &places, at, "a group among the owners").await;

    suspend_in(&mut conn, department, alice, w.boss)
        .await
        .unwrap();
    same_as_afresh(&mut conn, &people, &places, at, "a suspension").await;
    lift_suspension(&mut conn, department, alice).await.unwrap();
    same_as_afresh(&mut conn, &people, &places, at, "a suspension lifted").await;
    access::set_domain(&mut conn, w.root, alice, department, w.boss, at.get())
        .await
        .unwrap();
    same_as_afresh(&mut conn, &people, &places, at, "a domain moved").await;
    remove_from_group(&mut conn, group, alice).await.unwrap();
    same_as_afresh(&mut conn, &people, &places, at, "out of a group").await;
    make_system_admin(&mut conn, alice).await.unwrap();
    same_as_afresh(&mut conn, &people, &places, at, "a system admin").await;
    sqlx::query("UPDATE users SET status = 'suspended' WHERE id = $1")
        .bind(alice)
        .execute(&mut *conn)
        .await
        .unwrap();
    same_as_afresh(&mut conn, &people, &places, at, "suspended altogether").await;
    sqlx::query("UPDATE users SET status = 'active' WHERE id = $1")
        .bind(alice)
        .execute(&mut *conn)
        .await
        .unwrap();
    same_as_afresh(&mut conn, &people, &places, at, "active again").await;
    let topic = new_id();
    insert_node(&mut conn, topic, "topic", Some(inner), "New", w.boss)
        .await
        .unwrap();
    let places_and_topic = [&places[..], &[topic]].concat();
    same_as_afresh(&mut conn, &people, &places_and_topic, at, "a new topic").await;
    sqlx::query("UPDATE nodes SET deleted_at = $2 WHERE id = $1")
        .bind(department)
        .bind(at.get())
        .execute(&mut *conn)
        .await
        .unwrap();
    same_as_afresh(
        &mut conn,
        &people,
        &places_and_topic,
        at,
        "a department deleted",
    )
    .await;
    sqlx::query("UPDATE nodes SET deleted_at = NULL WHERE id = $1")
        .bind(department)
        .execute(&mut *conn)
        .await
        .unwrap();
    same_as_afresh(
        &mut conn,
        &people,
        &places_and_topic,
        at,
        "a department restored",
    )
    .await;
    remove_member(&mut conn, w.root, alice).await.unwrap();
    same_as_afresh(
        &mut conn,
        &people,
        &places_and_topic,
        at,
        "a membership ended",
    )
    .await;
}

#[tokio::test]
async fn a_kept_set_is_not_used_past_the_end_of_a_role_or_an_entry() {
    let w = World::new().await;
    let at = w.now;
    let outsider = w.user("outsider").await;
    let department = w.node("org", w.root, "Research").await;
    let places = [w.root, w.project, w.topic, w.thread, department];
    let mut conn = w.conn().await;
    give_role(
        &mut conn,
        department,
        "user",
        outsider,
        "viewer",
        w.boss,
        Some(at.get() + Duration::hours(1)),
    )
    .await
    .unwrap();
    add_entry(
        &mut conn,
        &PageKey::new("node", w.project, "live"),
        "any",
        "project.view",
        ("user", Some(outsider), None),
        Some(at.get() + Duration::hours(3)),
        at.get(),
    )
    .await
    .unwrap();
    // On both sides of each end, forward and back again.
    for seconds in [
        0, 3_599, 3_600, 7_200, 10_799, 10_800, 14_400, 3_599, 0, 10_800, 1,
    ] {
        let moment = Time::new(at.get() + Duration::seconds(seconds));
        same_as_afresh(
            &mut conn,
            &[outsider, w.boss],
            &places,
            moment,
            &format!("{seconds} s on"),
        )
        .await;
    }
}

#[tokio::test]
async fn a_change_not_committed_is_seen_by_its_own_transaction_alone_and_never_kept() {
    let w = World::new().await;
    let at = w.now;
    let alice = w.member("alice").await;
    let places = [w.root, w.project, w.topic, w.thread];
    let mut other = w.conn().await;
    let group = insert_group(&mut other, w.root, "Reviewers", w.boss)
        .await
        .unwrap();
    same_as_afresh(&mut other, &[alice], &places, at, "before").await;

    let mut tx = w.db.pool.begin().await.unwrap();
    // Kept by a transaction that had not written yet; then it writes.
    same_as_afresh(
        &mut tx,
        &[alice],
        &places,
        at,
        "in the transaction, before its change",
    )
    .await;
    add_to_group(&mut tx, group, alice).await.unwrap();
    add_entry(
        &mut tx,
        &PageKey::new("node", w.project, "live"),
        "any",
        "project.view",
        ("user", Some(alice), None),
        None,
        at.get(),
    )
    .await
    .unwrap();
    same_as_afresh(
        &mut tx,
        &[alice],
        &places,
        at,
        "in the transaction, after its change",
    )
    .await;
    let inside = Principal::load(&mut tx, Caller::session(alice), at)
        .await
        .unwrap();
    assert_eq!(inside.groups(), [group]);
    same_as_afresh(
        &mut other,
        &[alice],
        &places,
        at,
        "elsewhere, while it is open",
    )
    .await;
    let outside = Principal::load(&mut other, Caller::session(alice), at)
        .await
        .unwrap();
    assert!(
        outside.groups().is_empty(),
        "nothing uncommitted is seen elsewhere"
    );
    tx.rollback().await.unwrap();
    same_as_afresh(&mut other, &[alice], &places, at, "after the rollback").await;
    assert!(
        Principal::load(&mut other, Caller::session(alice), at)
            .await
            .unwrap()
            .groups()
            .is_empty()
    );

    let mut tx = w.db.pool.begin().await.unwrap();
    same_as_afresh(
        &mut tx,
        &[alice],
        &places,
        at,
        "a second transaction, before its change",
    )
    .await;
    add_to_group(&mut tx, group, alice).await.unwrap();
    tx.commit().await.unwrap();
    same_as_afresh(&mut other, &[alice], &places, at, "after the commit").await;
    assert_eq!(
        Principal::load(&mut other, Caller::session(alice), at)
            .await
            .unwrap()
            .groups(),
        [group]
    );
}

#[tokio::test]
async fn what_is_never_read_for_holders_is_never_recorded_as_a_change() {
    let w = World::new().await;
    let mut conn = w.conn().await;
    let before = changes(&mut conn).await;
    let thread = w.node("thread", w.topic, "Another").await;
    sqlx::query("UPDATE nodes SET name = 'Renamed', activity_at = $2 WHERE id = ANY($1)")
        .bind([w.project, thread])
        .bind(w.now.get())
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET display_name = 'Renamed' WHERE id = $1")
        .bind(w.maker)
        .execute(&mut *conn)
        .await
        .unwrap();
    add_entry(
        &mut conn,
        &PageKey::new("node", thread, "live"),
        "any",
        "thread.view",
        ("user", Some(w.boss), None),
        None,
        w.now.get(),
    )
    .await
    .unwrap();
    add_entry(
        &mut conn,
        &PageKey::new("document", new_id(), "live"),
        "any",
        "access.own",
        ("user", Some(w.boss), None),
        None,
        w.now.get(),
    )
    .await
    .unwrap();
    assert_eq!(
        changes(&mut conn).await,
        before,
        "a thread, its page, a document's page, names and activity"
    );
    let topic = w.node("topic", w.project, "Another").await;
    assert!(changes(&mut conn).await > before, "a new topic is a change");
    let after_topic = changes(&mut conn).await;
    sqlx::query("UPDATE nodes SET deleted_at = $2 WHERE id = $1")
        .bind(topic)
        .bind(w.now.get())
        .execute(&mut *conn)
        .await
        .unwrap();
    assert!(
        changes(&mut conn).await > after_topic,
        "a deletion is a change"
    );
}
