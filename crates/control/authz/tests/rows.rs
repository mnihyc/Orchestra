//! What a row comes to: following, being cut loose, entries that end, defaults at every level, the two rows of a permission, and locks.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission as P;
use orch_access_contract::roles::Role;
use orch_authz::Caller;
use orch_authz::pages::ChangeError;
use orch_db::queries::access::{PageKey, testing::add_entry};
use orch_db::queries::hierarchy::testing::{
    put_thread_in_environment, take_thread_out_of_environment,
};
use serde_json::json;
use support::{World, role_holder, user_holder};

fn role(role: Role) -> Holder {
    Holder::Role { role, at: None }
}

#[tokio::test]
async fn a_row_that_follows_is_held_by_the_defaults_and_adding_somebody_leaves_it_following() {
    let w = World::new().await;
    let (viewer, extra) = (w.user("viewer").await, w.user("extra").await);
    w.give(w.project, viewer, Role::Viewer).await;
    assert!(w.can(viewer, w.thread, P::ThreadView).await);
    assert!(!w.can(extra, w.thread, P::ThreadView).await);
    w.name(w.thread, P::ThreadView, Holder::User { id: extra })
        .await;
    assert!(w.can(extra, w.thread, P::ThreadView).await, "the entry");
    assert!(
        w.can(viewer, w.thread, P::ThreadView).await,
        "and the row still follows"
    );
}

#[tokio::test]
async fn a_row_cut_loose_and_empty_is_its_owners_alone_and_one_that_names_holders_is_theirs() {
    let w = World::new().await;
    let (viewer, dev, person, member_of_group) = (
        w.user("viewer").await,
        w.user("dev").await,
        w.user("person").await,
        w.user("grouped").await,
    );
    w.give(w.project, viewer, Role::Viewer).await;
    w.give(w.project, dev, Role::Developer).await;
    w.row(w.thread, P::ThreadView, false).await;
    // Cut loose and empty: the owners only. (A Developer still views what they manage.)
    assert!(!w.can(viewer, w.thread, P::ThreadView).await);
    assert!(w.can(w.maker, w.thread, P::ThreadView).await);
    assert!(
        w.can(dev, w.thread, P::ThreadView).await,
        "through managing"
    );
    // Naming a role, a group, a person.
    let group = w.group(w.root, "Readers", &[member_of_group]).await;
    w.name(w.thread, P::ThreadView, role(Role::Viewer)).await;
    assert!(
        w.can(viewer, w.thread, P::ThreadView).await,
        "the role named"
    );
    w.name(w.thread, P::ThreadView, Holder::Group { id: group })
        .await;
    assert!(
        w.can(member_of_group, w.thread, P::ThreadView).await,
        "the group named"
    );
    assert!(!w.can(person, w.thread, P::ThreadView).await);
    w.name(w.thread, P::ThreadView, Holder::User { id: person })
        .await;
    assert!(
        w.can(person, w.thread, P::ThreadView).await,
        "the person named"
    );
}

#[tokio::test]
async fn an_entry_that_ended_or_was_removed_counts_for_nothing_and_a_page_reset_follows_again() {
    let w = World::new().await;
    let (ending, gone, kept) = (
        w.member("ending").await,
        w.member("gone").await,
        w.member("kept").await,
    );
    let mut conn = w.conn().await;
    let page = PageKey::new("node", w.thread, "live");
    add_entry(
        &mut conn,
        &page,
        "any",
        "thread.view",
        ("user", Some(ending), None),
        Some(w.now.get() + Duration::days(7)),
        w.now.get(),
    )
    .await
    .unwrap();
    add_entry(
        &mut conn,
        &page,
        "any",
        "thread.view",
        ("user", Some(kept), None),
        None,
        w.now.get(),
    )
    .await
    .unwrap();
    let removed = add_entry(
        &mut conn,
        &page,
        "any",
        "thread.view",
        ("user", Some(gone), None),
        None,
        w.now.get(),
    )
    .await
    .unwrap();
    orch_db::queries::access::remove_entry(&mut conn, removed, w.boss, w.now.get())
        .await
        .unwrap();
    drop(conn);
    assert!(w.can(ending, w.thread, P::ThreadView).await, "for a week");
    assert!(!w.can(gone, w.thread, P::ThreadView).await, "removed");
    assert!(w.can(kept, w.thread, P::ThreadView).await);
    let later = w.later(Duration::days(8));
    assert!(
        !w.can_at(ending, w.thread, P::ThreadView, later).await,
        "ended"
    );
    assert!(w.can_at(kept, w.thread, P::ThreadView, later).await);
    w.change(
        w.maker,
        orch_authz::pages::Target::Live(w.thread),
        json!({ "whole": "reset" }),
    )
    .await
    .unwrap();
    assert!(
        !w.can(kept, w.thread, P::ThreadView).await,
        "a page reset follows the defaults again"
    );
}

#[tokio::test]
async fn a_default_changed_at_a_place_reaches_the_pages_inside_it_that_follow_and_not_those_that_do_not()
 {
    let w = World::new().await;
    let d = w.deep().await;
    let other = w.node_owned_by("thread", d.topic, "Private", w.maker).await;
    let (viewer, dev) = (w.user("viewer").await, w.user("dev").await);
    w.give(d.project, viewer, Role::Viewer).await;
    w.give(d.project, dev, Role::Developer).await;
    // One page was cut loose before the default changed; the other follows.
    w.row(other, P::ThreadView, false).await;
    assert!(w.can(viewer, d.thread, P::ThreadView).await);
    assert!(!w.can(viewer, other, P::ThreadView).await);
    // The project says that its threads are viewed by Developers and better.
    w.change_defaults(
        w.boss,
        d.project,
        json!({ "rows": [{ "permission": "thread.view", "profile": "managed", "inherit": false, "add": [{ "holder": role_holder(Role::Developer) }] }] }),
    )
    .await
    .unwrap();
    assert!(
        !w.can(viewer, d.thread, P::ThreadView).await,
        "the page followed the project's default"
    );
    assert!(w.can(dev, d.thread, P::ThreadView).await);
    assert!(
        !w.can(viewer, other, P::ThreadView).await,
        "the cut loose page was as it was"
    );
    // A thread elsewhere, under another project, is untouched.
    let elsewhere = w.node("thread", w.topic, "Else").await;
    w.give(w.project, viewer, Role::Viewer).await;
    assert!(w.can(viewer, elsewhere, P::ThreadView).await);
}

#[tokio::test]
async fn a_default_is_changed_at_the_system_a_root_org_an_org_inside_it_a_project_and_a_topic() {
    let w = World::new().await;
    let d = w.deep().await;
    let admin = w.user("sysadmin").await;
    let mut conn = w.conn().await;
    orch_db::queries::access::testing::make_system_admin(&mut conn, admin)
        .await
        .unwrap();
    drop(conn);
    let viewer = w.user("viewer").await;
    w.give(w.root, viewer, Role::Viewer).await;
    assert!(
        w.can(viewer, d.thread, P::ChatInteract).await,
        "as shipped Viewers take part in the chat"
    );
    let none = json!({ "rows": [{ "permission": "chat.interact", "inherit": false }] });
    // Each level in turn: only cutting the row loose there and naming nobody closes the chat below.
    for (place, what) in [
        (Some(d.topic), "topic"),
        (Some(d.project), "project"),
        (Some(d.d3), "org inside"),
        (Some(d.d1), "org"),
        (Some(w.root), "root org"),
    ] {
        let place = place.unwrap();
        w.change_defaults(w.boss, place, none.clone())
            .await
            .unwrap();
        assert!(
            !w.can(viewer, d.thread, P::ChatInteract).await,
            "cut loose at the {what}"
        );
        w.change_defaults(
            w.boss,
            place,
            json!({ "rows": [{ "permission": "chat.interact", "inherit": true }] }),
        )
        .await
        .unwrap();
        assert!(
            w.can(viewer, d.thread, P::ChatInteract).await,
            "following again at the {what}"
        );
    }
    // The system's own page is changed by a system admin and no one else.
    let at_the_system = |who| {
        w.change_as_admin(
            who,
            orch_authz::pages::Target::Defaults(orch_access_contract::roles::PlaceRef::System),
            json!({ "rows": [{ "permission": "chat.interact", "remove": [] }] }),
        )
    };
    assert!(matches!(
        at_the_system(w.boss).await,
        Err(ChangeError::SystemAdminOnly)
    ));
    at_the_system(admin).await.unwrap();
}

#[tokio::test]
async fn the_defaults_of_the_system_follow_nothing_and_are_changed_row_by_row() {
    let w = World::new().await;
    let admin = w.user("sysadmin").await;
    let mut conn = w.conn().await;
    orch_db::queries::access::testing::make_system_admin(&mut conn, admin)
        .await
        .unwrap();
    drop(conn);
    let viewer = w.user("viewer").await;
    w.give(w.project, viewer, Role::Viewer).await;
    assert!(w.can(viewer, w.thread, P::ChatInteract).await);
    let system = orch_authz::pages::Target::Defaults(orch_access_contract::roles::PlaceRef::System);
    let page = w
        .read_as(Caller::session(admin).with_admin_mode(true), system)
        .await;
    let row = page
        .rows
        .iter()
        .find(|row| row.permission == P::ChatInteract)
        .unwrap();
    assert_eq!(row.entries.len(), 1);
    assert!(!row.inherit && row.inherited.is_none());
    // Taking the Viewers out of the system's row changes every page that follows it.
    let entry = row.entries[0].id;
    w.change_as_admin(
        admin,
        system,
        json!({ "rows": [{ "permission": "chat.interact", "remove": [entry] }] }),
    )
    .await
    .unwrap();
    assert!(!w.can(viewer, w.thread, P::ChatInteract).await);
    // It cannot be cut loose from nothing.
    let refused = w
        .change_as_admin(admin, system, json!({ "whole": "private" }))
        .await;
    assert!(
        matches!(refused, Err(ChangeError::Invalid(_))),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_permission_with_two_rows_follows_the_one_that_fits_where_the_thread_runs() {
    let w = World::new().await;
    let interactor = w.user("interactor").await;
    w.give(w.project, interactor, Role::Interactor).await;
    assert!(
        w.can(interactor, w.thread, P::ThreadInteract).await,
        "no environment yet: managed"
    );
    let mut conn = w.conn().await;
    put_thread_in_environment(&mut conn, w.thread, "connected", w.maker, w.topic)
        .await
        .unwrap();
    drop(conn);
    assert!(
        !w.can(interactor, w.thread, P::ThreadInteract).await,
        "on a person's machine nobody but its owners, as shipped"
    );
    assert!(
        w.can(interactor, w.thread, P::ThreadView).await,
        "everybody watches"
    );
    // The thread's own entries stay when it is switched to a machine and back.
    let named = w.user("named").await;
    w.name(w.thread, P::ThreadInteract, Holder::User { id: named })
        .await;
    assert!(w.can(named, w.thread, P::ThreadInteract).await);
    let mut conn = w.conn().await;
    take_thread_out_of_environment(&mut conn, w.thread)
        .await
        .unwrap();
    put_thread_in_environment(&mut conn, w.thread, "managed", w.maker, w.topic)
        .await
        .unwrap();
    drop(conn);
    assert!(
        w.can(interactor, w.thread, P::ThreadInteract).await,
        "back where the default names Interactors"
    );
    assert!(
        w.can(named, w.thread, P::ThreadInteract).await,
        "its own entry stayed"
    );
    // A place that opens its machines says so on the connected row.
    let mut conn = w.conn().await;
    take_thread_out_of_environment(&mut conn, w.thread)
        .await
        .unwrap();
    put_thread_in_environment(&mut conn, w.thread, "connected", w.maker, w.topic)
        .await
        .unwrap();
    drop(conn);
    w.change_defaults(
        w.boss,
        w.project,
        json!({ "rows": [{ "permission": "thread.interact", "profile": "connected", "add": [{ "holder": role_holder(Role::Interactor) }] }] }),
    )
    .await
    .unwrap();
    assert!(w.can(interactor, w.thread, P::ThreadInteract).await);
}

#[tokio::test]
async fn the_topmost_lock_decides_and_pages_below_follow_it_whatever_they_stored_until_it_is_lifted()
 {
    let w = World::new().await;
    let d = w.deep().await;
    let (admin, interactor, eve) = (
        w.user("admin").await,
        w.user("interactor").await,
        w.user("eve").await,
    );
    w.give(d.d1, admin, Role::Admin).await;
    w.give(d.d1, interactor, Role::Interactor).await;
    let second = w.node("thread", d.topic, "Cut loose").await;
    // One page added to the row before the lock, the other cut it loose.
    w.name(d.thread, P::ThreadManage, Holder::User { id: eve })
        .await;
    w.row(second, P::ThreadManage, false).await;
    w.name(second, P::ThreadManage, Holder::User { id: eve })
        .await;
    assert!(w.can(eve, d.thread, P::ThreadManage).await);
    // The org and the project under it both lock the row; the upper one decides.
    let lock = |role: Role| json!({ "rows": [{ "permission": "thread.manage", "profile": "managed", "locked": true, "add": [{ "holder": role_holder(role) }] }] });
    // (The project locks first: once the org has, the project's row is changed no more.)
    w.change_defaults(w.boss, d.project, lock(Role::Interactor))
        .await
        .unwrap();
    w.change_defaults(w.boss, d.d1, lock(Role::Admin))
        .await
        .unwrap();
    for thread in [d.thread, second] {
        assert!(
            w.can(admin, thread, P::ThreadManage).await,
            "the upper lock names Admins"
        );
        assert!(
            !w.can(interactor, thread, P::ThreadManage).await,
            "not what the project's lock names"
        );
        assert!(
            !w.can(eve, thread, P::ThreadManage).await,
            "what they stored counts for nothing"
        );
    }
    // An entry tried under the lock is refused.
    let refused = w
        .change(w.maker, orch_authz::pages::Target::Live(d.thread), json!({ "rows": [{ "permission": "thread.manage", "add": [{ "holder": user_holder(interactor) }] }] }))
        .await;
    assert!(
        matches!(refused, Err(ChangeError::Locked { by: orch_access_contract::roles::PlaceRef::Node(place), .. }) if place == d.d1),
        "{refused:?}"
    );
    let page = w
        .read(w.maker, orch_authz::pages::Target::Live(d.thread))
        .await;
    let row = page
        .rows
        .iter()
        .find(|row| row.permission == P::ThreadManage)
        .unwrap();
    assert_eq!(
        row.locked.as_ref().map(|lock| lock.by),
        Some(orch_access_contract::roles::PlaceRef::Node(d.d1))
    );
    assert!(!row.writable);
    // The project cannot lift its own lock while the org's holds: a row under a lock is changed where the lock is.
    let below = w.change_defaults(w.boss, d.project, json!({ "rows": [{ "permission": "thread.manage", "profile": "managed", "locked": false }] })).await;
    assert!(
        matches!(below, Err(ChangeError::Locked { .. })),
        "{below:?}"
    );
    // Lifting the upper one lets the lower decide; lifting that puts every page back as it was.
    w.change_defaults(w.boss, d.d1, json!({ "rows": [{ "permission": "thread.manage", "profile": "managed", "locked": false }] })).await.unwrap();
    assert!(
        w.can(interactor, d.thread, P::ThreadManage).await,
        "now the project's lock decides: Interactors"
    );
    w.change_defaults(w.boss, d.project, json!({ "rows": [{ "permission": "thread.manage", "profile": "managed", "locked": false }] })).await.unwrap();
    assert!(
        w.can(eve, d.thread, P::ThreadManage).await,
        "as it was: the entry added"
    );
    assert!(
        w.can(eve, second, P::ThreadManage).await,
        "as it was: cut loose and naming eve"
    );
    assert!(
        w.can(interactor, d.thread, P::ThreadManage).await,
        "the entries the locks were set with stay on the defaults pages, and are followed"
    );
}

#[tokio::test]
async fn a_lock_set_on_a_defaults_page_is_changed_there_and_nowhere_below() {
    let w = World::new().await;
    let d = w.deep().await;
    w.change_defaults(
        w.boss,
        d.d1,
        json!({ "rows": [{ "permission": "chat.manage", "locked": true }] }),
    )
    .await
    .unwrap();
    let below = w
        .change_defaults(
            w.boss,
            d.project,
            json!({ "rows": [{ "permission": "chat.manage", "inherit": false }] }),
        )
        .await;
    assert!(
        matches!(below, Err(ChangeError::Locked { .. })),
        "{below:?}"
    );
    let page = w
        .read(
            w.boss,
            orch_authz::pages::Target::Defaults(orch_access_contract::roles::PlaceRef::Node(d.d1)),
        )
        .await;
    let row = page
        .rows
        .iter()
        .find(|row| row.permission == P::ChatManage)
        .unwrap();
    assert!(
        row.lock_set_here && row.writable,
        "where the lock is, the row is changed"
    );
    let rows_below = w
        .read(
            w.boss,
            orch_authz::pages::Target::Defaults(orch_access_contract::roles::PlaceRef::Node(
                d.project,
            )),
        )
        .await;
    let row = rows_below
        .rows
        .iter()
        .find(|row| row.permission == P::ChatManage)
        .unwrap();
    assert!(!row.writable && !row.lock_set_here);
    assert_eq!(
        row.locked.as_ref().map(|lock| (lock.by, lock.here)),
        Some((orch_access_contract::roles::PlaceRef::Node(d.d1), false))
    );
}

#[tokio::test]
async fn private_and_reset_leave_alone_the_rows_a_lock_above_decides_and_say_which() {
    let w = World::new().await;
    w.change_defaults(
        w.boss,
        w.root,
        json!({ "rows": [{ "permission": "thread.view", "profile": "managed", "locked": true }] }),
    )
    .await
    .unwrap();
    let outcome = w
        .change(
            w.maker,
            orch_authz::pages::Target::Live(w.thread),
            json!({ "whole": "private" }),
        )
        .await
        .unwrap();
    assert!(outcome.changed);
    assert!(
        outcome
            .skipped
            .iter()
            .any(|(permission, ..)| *permission == P::ThreadView)
    );
    let page = w
        .read(w.maker, orch_authz::pages::Target::Live(w.thread))
        .await;
    let view = page
        .rows
        .iter()
        .find(|row| row.permission == P::ThreadView)
        .unwrap();
    let chat = page
        .rows
        .iter()
        .find(|row| row.permission == P::ChatView)
        .unwrap();
    assert!(view.inherit && !chat.inherit);
}

#[tokio::test]
async fn a_change_of_a_default_reaches_the_pages_that_follow_it_and_a_page_says_how_many() {
    let w = World::new().await;
    let d = w.deep().await;
    let quiet = w.node("topic", d.project, "Quiet").await;
    let (a, b, c) = (
        w.node("thread", d.topic, "A").await,
        w.node("thread", d.topic, "B").await,
        w.node("thread", quiet, "C").await,
    );
    let rows = [(
        P::ThreadView,
        orch_access_contract::permissions::Profile::Managed,
    )];
    let count = |place| {
        let w = &w;
        async move {
            let mut conn = w.conn().await;
            orch_authz::defaults::followers(&mut conn, place, &rows)
                .await
                .unwrap()
        }
    };
    // d.thread, a, b under the topic and c under the quiet one: four threads follow the project's row.
    assert_eq!(count(d.project).await, 4);
    // One page cut its row loose, and a topic cut its own default loose: those below it do not follow.
    w.row(a, P::ThreadView, false).await;
    w.default_row(quiet, P::ThreadView, "managed", false, false)
        .await;
    assert_eq!(
        count(d.project).await,
        2,
        "d.thread and b follow; a and c do not"
    );
    assert_eq!(count(d.topic).await, 2);
    // Locked at the project, the row is followed by every thread inside whatever it stored.
    w.default_row(d.project, P::ThreadView, "managed", true, true)
        .await;
    assert_eq!(count(d.project).await, 4);
    // A lock above decides the row: the project's own row reaches nothing.
    w.default_row(d.d1, P::ThreadView, "managed", true, true)
        .await;
    assert_eq!(count(d.project).await, 0);
    let _ = (b, c);
    // The page says it too, for the rows it has: its followers are pages that follow at least one.
    let page = w
        .read(
            w.boss,
            orch_authz::pages::Target::Defaults(orch_access_contract::roles::PlaceRef::Node(
                d.topic,
            )),
        )
        .await;
    assert!(page.followers.unwrap() >= 2);
    let live = w
        .read(w.boss, orch_authz::pages::Target::Live(d.thread))
        .await;
    assert_eq!(live.followers, None);
}
