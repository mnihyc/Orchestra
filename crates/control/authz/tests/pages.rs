//! Reading a page and changing it: rows, entries, owners, locks, outsiders, the override, and the lines they leave.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::Origin;
use orch_access_contract::permissions::{Permission as P, Profile};
use orch_access_contract::roles::{PlaceRef, Role};
use orch_authz::Caller;
use orch_authz::pages::{ChangeError, Target};
use serde_json::json;
use support::{World, group_holder, user_holder};

#[tokio::test]
async fn a_new_things_page_is_its_owners_and_follows_the_defaults_throughout() {
    let w = World::new().await;
    let page = w.read(w.maker, Target::Live(w.thread)).await;
    assert_eq!(page.state, orch_access_contract::pages::PageState::Follows);
    assert_eq!(page.first_owner, Some(w.maker));
    assert_eq!(page.owners.len(), 1);
    assert_eq!(page.revision, 1);
    assert!(page.writable && page.owners_writable);
    assert!(
        page.rows
            .iter()
            .all(|row| row.inherit && row.entries.is_empty())
    );
    let view = page
        .rows
        .iter()
        .find(|row| row.permission == P::ThreadView)
        .unwrap();
    let inherited = view.inherited.as_ref().unwrap();
    assert_eq!(inherited.len(), 1, "the shipped Viewers");
    assert_eq!(inherited[0].from, Origin::Defaults(PlaceRef::System));
    assert_eq!(page.admins_of.last(), Some(&PlaceRef::System));
    assert!(
        !page.rows.iter().any(|row| row.permission == P::AccessOwn),
        "the Owners row is not a row"
    );
}

#[tokio::test]
async fn naming_somebody_with_a_choice_names_them_in_every_row_it_covers_and_leaves_the_rows_following()
 {
    let w = World::new().await;
    let chen = w.member("chen").await;
    assert!(!w.can(chen, w.thread, P::ThreadInteract).await);
    let before = w.epoch().await;
    let outcome = w
        .change(
            w.maker,
            Target::Live(w.thread),
            json!({ "name": [{ "holders": [user_holder(chen)], "choice": "full_interact" }] }),
        )
        .await
        .unwrap();
    assert!(outcome.changed && !outcome.r#override);
    assert_eq!(outcome.revision, 2);
    assert_eq!(outcome.epoch, before + 1, "the epoch is moved once");
    assert!(w.can(chen, w.thread, P::ThreadInteract).await);
    assert!(
        w.can(chen, w.thread, P::ThreadView).await,
        "a level holds those below it"
    );
    assert!(!w.can(chen, w.thread, P::ThreadManage).await);
    let page = w.read(w.maker, Target::Live(w.thread)).await;
    assert_eq!(page.revision, 2);
    assert_eq!(page.state, orch_access_contract::pages::PageState::Custom);
    for permission in [P::ChatInteract, P::DocEdit, P::ThreadInteract] {
        let row = page
            .rows
            .iter()
            .find(|row| row.permission == permission)
            .unwrap();
        assert!(row.inherit, "{permission} still follows");
        assert_eq!(row.entries.len(), 1, "{permission}");
    }
    assert!(
        page.rows.iter().all(|row| !row.permission.area().is_door()),
        "a thread's page holds none of the doors, so none is named"
    );
    let audit = w.audit().await;
    let added: Vec<_> = audit
        .iter()
        .filter(|row| row.action == "permission.added")
        .collect();
    assert_eq!(
        added.len(),
        6,
        "the three rows the choice covers and the three Export rows that travel with it"
    );
    assert!(
        added
            .iter()
            .all(|row| row.history && !row.r#override && row.node_id == Some(w.thread))
    );
    for permission in [P::ThreadExport, P::ChatExport, P::DocExport] {
        let row = page
            .rows
            .iter()
            .find(|row| row.permission == permission)
            .unwrap();
        assert_eq!(row.entries.len(), 1, "{permission} comes with the choice");
    }
    assert!(added[0].summary.contains("chen"), "{}", added[0].summary);
}

#[tokio::test]
async fn a_change_that_does_not_fit_the_page_changes_nothing_and_says_every_fault() {
    let w = World::new().await;
    let chen = w.member("chen").await;
    let before = w.epoch().await;
    let refused = w
        .change(
            w.maker,
            Target::Live(w.thread),
            json!({
                "name": [{ "holders": [user_holder(chen)], "choice": "full_view" }],
                "rows": [{ "permission": "provider.use" }, { "permission": "topic.view" }]
            }),
        )
        .await;
    let Err(ChangeError::Invalid(faults)) = refused else {
        panic!("{refused:?}")
    };
    assert_eq!(faults.len(), 2, "{faults:?}");
    assert_eq!(w.epoch().await, before);
    assert!(w.audit().await.is_empty());
    assert!(!w.can(chen, w.thread, P::ThreadView).await);
}

#[tokio::test]
async fn a_page_changed_since_it_was_read_is_refused_with_its_revision() {
    let w = World::new().await;
    let chen = w.member("chen").await;
    let change = |expected| {
        json!({
            "name": [{ "holders": [user_holder(chen)], "choice": "full_view" }],
            "expected_revision": expected
        })
    };
    w.change(w.maker, Target::Live(w.thread), change(1))
        .await
        .unwrap();
    let stale = w.change(w.maker, Target::Live(w.thread), change(1)).await;
    assert!(
        matches!(stale, Err(ChangeError::Stale { current: 2 })),
        "{stale:?}"
    );
}

#[tokio::test]
async fn naming_somebody_who_is_named_already_changes_only_what_the_request_says_anew() {
    let w = World::new().await;
    let chen = w.member("chen").await;
    let change = |body: serde_json::Value| w.change(w.maker, Target::Live(w.thread), body);
    change(json!({ "rows": [{ "permission": "thread.view", "add": [{ "holder": user_holder(chen), "note": "for a week", "expires_at": "2026-10-15T00:00:00.000Z" }] }] }))
        .await
        .unwrap();
    let again = change(json!({ "rows": [{ "permission": "thread.view", "add": [{ "holder": user_holder(chen) }] }] })).await.unwrap();
    assert!(!again.changed, "naming them again says nothing new");
    let page = w.read(w.maker, Target::Live(w.thread)).await;
    let row = page
        .rows
        .iter()
        .find(|row| row.permission == P::ThreadView)
        .unwrap();
    assert_eq!(row.entries.len(), 1);
    assert_eq!(row.entries[0].note, "for a week");
    assert!(row.entries[0].expires_at.is_some(), "the end stays");
    let moved = change(json!({ "rows": [{ "permission": "thread.view", "add": [{ "holder": user_holder(chen), "expires_at": "2026-10-20T00:00:00.000Z" }] }] })).await.unwrap();
    assert!(moved.changed);
    let audit = w.audit().await;
    assert_eq!(
        audit
            .iter()
            .filter(|row| row.action == "permission.updated")
            .count(),
        1
    );
}

#[tokio::test]
async fn an_entry_is_changed_and_removed_by_its_identifier_in_its_own_row_only() {
    let w = World::new().await;
    let chen = w.member("chen").await;
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({ "rows": [
        { "permission": "thread.view", "add": [{ "holder": user_holder(chen) }] },
        { "permission": "thread.interact", "add": [{ "holder": user_holder(chen) }] }
    ] }),
    )
    .await
    .unwrap();
    let page = w.read(w.maker, Target::Live(w.thread)).await;
    let view_entry = page
        .rows
        .iter()
        .find(|r| r.permission == P::ThreadView)
        .unwrap()
        .entries[0]
        .id;
    let interact_entry = page
        .rows
        .iter()
        .find(|r| r.permission == P::ThreadInteract)
        .unwrap()
        .entries[0]
        .id;
    // The entry of another row is no entry of this one.
    let wrong = w
        .change(
            w.maker,
            Target::Live(w.thread),
            json!({ "rows": [{ "permission": "thread.view", "remove": [interact_entry] }] }),
        )
        .await;
    let Err(ChangeError::Invalid(faults)) = wrong else {
        panic!("{wrong:?}")
    };
    assert_eq!(faults[0].path, "rows[0].remove[0]");
    // The Owners row is changed with owners only.
    let owner = w.read(w.maker, Target::Live(w.thread)).await.owners[0].id;
    assert!(matches!(
        w.change(
            w.maker,
            Target::Live(w.thread),
            json!({ "rows": [{ "permission": "thread.view", "remove": [owner] }] })
        )
        .await,
        Err(ChangeError::Invalid(_))
    ));
    w.change(w.maker, Target::Live(w.thread), json!({ "rows": [{ "permission": "thread.view", "update": [{ "entry_id": view_entry, "note": "noted" }], }] })).await.unwrap();
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({ "rows": [{ "permission": "thread.interact", "remove": [interact_entry] }] }),
    )
    .await
    .unwrap();
    assert!(!w.can(chen, w.thread, P::ThreadInteract).await);
    assert!(w.can(chen, w.thread, P::ThreadView).await);
    let actions: Vec<String> = w.audit().await.into_iter().map(|row| row.action).collect();
    assert!(
        actions.contains(&"permission.updated".to_owned())
            && actions.contains(&"permission.removed".to_owned())
    );
}

#[tokio::test]
async fn cutting_a_row_loose_leaves_it_to_its_owners_and_whom_it_names() {
    let w = World::new().await;
    let dev = w.user("dev").await;
    let viewer = w.user("viewer").await;
    let chen = w.member("chen").await;
    w.give(w.project, dev, Role::Developer).await;
    w.give(w.project, viewer, Role::Viewer).await;
    assert!(
        w.can(viewer, w.thread, P::ThreadView).await,
        "followed from the defaults"
    );
    let outcome = w
        .change(w.maker, Target::Live(w.thread), json!({ "rows": [{ "permission": "thread.view", "inherit": false, "add": [{ "holder": user_holder(chen) }] }] }))
        .await
        .unwrap();
    assert!(outcome.changed);
    assert!(
        !w.can(viewer, w.thread, P::ThreadView).await,
        "no longer follows"
    );
    assert!(
        w.can(dev, w.thread, P::ThreadView).await,
        "a Developer manages, and a level holds those below it: another row gives it"
    );
    assert!(w.can(chen, w.thread, P::ThreadView).await);
    assert!(w.can(w.maker, w.thread, P::ThreadView).await);
    let page = w.read(w.maker, Target::Live(w.thread)).await;
    assert!(
        !page
            .rows
            .iter()
            .find(|r| r.permission == P::ThreadView)
            .unwrap()
            .inherit
    );
    assert!(
        w.audit()
            .await
            .iter()
            .any(|row| row.action == "permission.row_changed" && row.summary.contains("cut loose"))
    );
}

#[tokio::test]
async fn private_cuts_every_row_loose_and_reset_puts_the_page_back_but_for_its_owners() {
    let w = World::new().await;
    let viewer = w.user("viewer").await;
    let chen = w.member("chen").await;
    w.give(w.root, viewer, Role::Viewer).await;
    assert!(w.can(viewer, w.thread, P::ThreadView).await);
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({ "whole": "private" }),
    )
    .await
    .unwrap();
    let page = w.read(w.maker, Target::Live(w.thread)).await;
    assert!(page.rows.iter().all(|row| !row.inherit));
    assert_eq!(
        page.state,
        orch_access_contract::pages::PageState::Restricted
    );
    assert!(!w.can(viewer, w.thread, P::ThreadView).await);
    assert!(
        w.can(w.maker, w.thread, P::ThreadManage).await,
        "its owners hold it"
    );
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({ "name": [{ "holders": [user_holder(chen)], "choice": "full_view" }] }),
    )
    .await
    .unwrap();
    assert!(w.can(chen, w.thread, P::ThreadView).await);
    w.change(w.maker, Target::Live(w.thread), json!({ "whole": "reset" }))
        .await
        .unwrap();
    let page = w.read(w.maker, Target::Live(w.thread)).await;
    assert!(
        page.rows
            .iter()
            .all(|row| row.inherit && row.entries.is_empty())
    );
    assert_eq!(page.owners.len(), 1, "the Owners row stays");
    assert_eq!(page.state, orch_access_contract::pages::PageState::Follows);
    assert!(w.can(viewer, w.thread, P::ThreadView).await);
    assert!(!w.can(chen, w.thread, P::ThreadView).await);
}

#[tokio::test]
async fn the_owners_are_changed_by_an_owner_or_an_admin_and_the_last_person_stays() {
    let w = World::new().await;
    let (second, third) = (w.member("second").await, w.member("third").await);
    let admin = w.user("admin").await;
    w.give(w.root, admin, Role::Admin).await;
    let dev = w.member("dev").await;
    w.give(w.project, dev, Role::Developer).await;
    w.name(
        w.thread,
        P::AccessManage,
        orch_access_contract::pages::Holder::User { id: dev },
    )
    .await;

    // A second owner, named by an owner, holds everything on the page but naming outsiders.
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({ "owners": { "add": [user_holder(second)] } }),
    )
    .await
    .unwrap();
    assert!(w.can(second, w.thread, P::ThreadManage).await);
    assert!(w.can(second, w.thread, P::AccessManage).await);
    assert!(!w.can(second, w.thread, P::AccessExternal).await);
    // A third, by an admin explicitly selecting that session mode.
    w.change_as_admin(
        admin,
        Target::Live(w.thread),
        json!({ "owners": { "add": [user_holder(third)] } }),
    )
    .await
    .unwrap();
    // Somebody who changes the page through an entry cannot change the owners.
    let refused = w
        .change(
            dev,
            Target::Live(w.thread),
            json!({ "owners": { "add": [user_holder(dev)] } }),
        )
        .await;
    assert!(
        matches!(refused, Err(ChangeError::Denied(_))),
        "{refused:?}"
    );
    // The first owner changes, and the address follows: the column too.
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({ "owners": { "first": second } }),
    )
    .await
    .unwrap();
    let page = w.read(w.maker, Target::Live(w.thread)).await;
    assert_eq!(page.first_owner, Some(second));
    assert!(
        w.audit()
            .await
            .iter()
            .any(|row| row.action == "node.transferred")
    );
    // Taking everybody out in turn: the last person is not taken out.
    let owners = page.owners;
    let id_of = |user| {
        owners
            .iter()
            .find(|entry| entry.holder == orch_access_contract::pages::Holder::User { id: user })
            .unwrap()
            .id
    };
    w.change(
        second,
        Target::Live(w.thread),
        json!({ "owners": { "remove": [id_of(w.maker), id_of(third)] } }),
    )
    .await
    .unwrap();
    let last = w
        .change(
            second,
            Target::Live(w.thread),
            json!({ "owners": { "remove": [id_of(second)] } }),
        )
        .await;
    assert!(matches!(last, Err(ChangeError::LastOwner)), "{last:?}");
    assert!(
        !w.can(w.maker, w.thread, P::ThreadManage).await
            || w.can(w.maker, w.thread, P::ThreadManage).await
    );
}

#[tokio::test]
async fn an_owner_who_is_no_member_of_the_root_org_is_refused() {
    let w = World::new().await;
    let stranger = w.user("stranger").await;
    let refused = w
        .change(
            w.maker,
            Target::Live(w.thread),
            json!({ "owners": { "add": [user_holder(stranger)] } }),
        )
        .await;
    assert!(
        matches!(refused, Err(ChangeError::Outside { .. })),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_group_may_own_a_thing_and_its_members_then_hold_everything_but_naming_outsiders() {
    let w = World::new().await;
    let member = w.member("member").await;
    w.give(w.root, w.maker, Role::Viewer).await;
    let group = {
        let mut conn = w.conn().await;
        let group =
            orch_db::queries::access::testing::insert_group(&mut conn, w.root, "Leads", w.maker)
                .await
                .unwrap();
        orch_db::queries::access::testing::add_to_group(&mut conn, group, member)
            .await
            .unwrap();
        group
    };
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({ "owners": { "add": [group_holder(group)] } }),
    )
    .await
    .unwrap();
    assert!(w.can(member, w.thread, P::ThreadManage).await);
    assert!(!w.can(member, w.thread, P::AccessExternal).await);
}

#[tokio::test]
async fn naming_a_holder_of_the_right_to_name_outsiders_is_for_an_admin_alone() {
    let w = World::new().await;
    let chen = w.member("chen").await;
    let admin = w.user("admin").await;
    w.give(w.root, admin, Role::Admin).await;
    let body = json!({ "rows": [{ "permission": "access.external", "add": [{ "holder": user_holder(chen) }] }] });
    let refused = w
        .change(w.maker, Target::Live(w.thread), body.clone())
        .await;
    assert!(
        matches!(refused, Err(ChangeError::Denied(_))),
        "owners do not hold that row, and so do not give it: {refused:?}"
    );
    w.change_as_admin(admin, Target::Live(w.thread), body)
        .await
        .unwrap();
    assert!(w.can(chen, w.thread, P::AccessExternal).await);
}

#[tokio::test]
async fn somebody_from_outside_the_root_org_is_named_by_whoever_holds_the_right_or_by_an_owner_where_the_policy_lets()
 {
    let w = World::new().await;
    let guest = w.user("guest").await;
    let dev = w.member("dev").await;
    let colleague = w.member("colleague").await;
    w.give(w.project, dev, Role::Developer).await;
    let by_access = w.member("keeper").await;
    // Somebody who changes the page names no more than they hold: the keeper reads the thread.
    w.give(w.project, by_access, Role::Viewer).await;
    w.name(
        w.thread,
        P::AccessManage,
        orch_access_contract::pages::Holder::User { id: by_access },
    )
    .await;
    w.name(
        w.thread,
        P::AccessManage,
        orch_access_contract::pages::Holder::User { id: dev },
    )
    .await;
    let body = |who| json!({ "name": [{ "holders": [user_holder(who)], "choice": "full_view" }] });
    // A colleague of the tree is named by anybody who may change the page.
    w.change(by_access, Target::Live(w.thread), body(colleague))
        .await
        .unwrap();
    // Somebody who may change the page and holds no right to name outsiders cannot.
    let refused = w
        .change(by_access, Target::Live(w.thread), body(guest))
        .await;
    let Err(ChangeError::Outside { ask, .. }) = refused else {
        panic!("{refused:?}")
    };
    assert!(
        ask.contains(&dev),
        "the Developers of the place may, as shipped: {ask:?}"
    );
    // The owner may not as shipped, and may where the policy lets owners.
    let policy = orch_authz::Policy {
        owners_name_outsiders: true,
        ..orch_authz::Policy::default()
    };
    assert!(matches!(
        w.change(w.maker, Target::Live(w.thread), body(guest)).await,
        Err(ChangeError::Outside { .. })
    ));
    w.change_as(
        Caller::session(w.maker),
        Target::Live(w.thread),
        body(guest),
        policy,
    )
    .await
    .unwrap();
    assert!(
        w.can(guest, w.thread, P::ThreadView).await,
        "from then on it is access like anybody's"
    );
    // A Developer holds the row and names an outsider at once.
    let other = w.user("other").await;
    w.change(dev, Target::Live(w.thread), body(other))
        .await
        .unwrap();
    assert!(w.can(other, w.thread, P::ThreadView).await);
}

#[tokio::test]
async fn explicit_administration_reads_and_manages_a_private_thread_without_self_naming() {
    let w = World::new().await;
    let admin = w.member("admin").await;
    w.give(w.root, admin, Role::Admin).await;
    // The thread was made private: its owners and whom it names alone.
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({ "whole": "private" }),
    )
    .await
    .unwrap();
    assert!(
        !w.can(admin, w.thread, P::ThreadView).await,
        "the content is refused"
    );
    assert!(!w.can(admin, w.thread, P::AccessManage).await);
    let caller = Caller::session(admin).with_admin_mode(true);
    assert!(w.can_as(caller.clone(), w.thread, P::ThreadView).await);
    assert!(w.can_as(caller.clone(), w.thread, P::AccessManage).await);
    let page = w.read_as(caller, Target::Live(w.thread)).await;
    assert!(page.writable);
    assert!(
        page.rows.iter().all(|row| row.entries.is_empty()),
        "native reach writes no entry"
    );
    // A native View/Manage act needs no self-naming override.
    let outcome = w
        .change_as_admin(admin, Target::Live(w.thread), json!({ "rows": [{ "permission": "thread.view", "add": [{ "holder": user_holder(admin) }] }] }))
        .await
        .unwrap();
    assert!(!outcome.r#override);
    assert!(
        w.can(admin, w.thread, P::ThreadView).await,
        "from then on the page shows them like anybody"
    );
    let audit = w.audit().await;
    let marked: Vec<_> = audit.iter().filter(|row| row.r#override).collect();
    assert!(marked.is_empty(), "native View is not an advanced override");
    assert!(
        audit
            .iter()
            .any(|row| row.action == "permission.added" && row.history)
    );
}

#[tokio::test]
async fn an_act_the_page_already_gives_an_admin_is_no_bypass() {
    let w = World::new().await;
    let admin = w.user("admin").await;
    w.give(w.root, admin, Role::Admin).await;
    assert!(
        w.can(admin, w.thread, P::ThreadView).await,
        "the row names Viewers, and an Admin is one"
    );
    let outcome = w
        .change_as_admin(admin, Target::Live(w.thread), json!({ "rows": [{ "permission": "thread.view", "add": [{ "holder": user_holder(admin) }] }] }))
        .await
        .unwrap();
    assert!(!outcome.r#override);
    assert!(w.audit().await.iter().all(|row| !row.r#override));
    // And nobody but an admin of the place has one: an owner naming themself is none.
    let owner_named = w
        .change(w.maker, Target::Live(w.thread), json!({ "rows": [{ "permission": "thread.manage", "add": [{ "holder": user_holder(w.maker) }] }] }))
        .await
        .unwrap();
    assert!(!owner_named.r#override);
}

#[tokio::test]
async fn somebody_who_is_no_admin_of_the_place_changes_nothing_and_is_not_offered_to_force_it() {
    let w = World::new().await;
    let dev = w.user("dev").await;
    w.give(w.project, dev, Role::Developer).await;
    let refused = w
        .change(dev, Target::Live(w.thread), json!({ "rows": [{ "permission": "thread.view", "add": [{ "holder": user_holder(dev) }] }] }))
        .await;
    let Err(ChangeError::Denied(refusal)) = refused else {
        panic!("{refused:?}")
    };
    assert_eq!(refusal.required, P::AccessManage);
    assert!(refusal.requestable && !refusal.r#override);
    assert!(
        refusal.missing[0].ask.contains(&w.maker),
        "the owner can give it"
    );
}

#[tokio::test]
async fn an_admin_of_one_topic_has_no_say_over_a_thread_of_another() {
    let w = World::new().await;
    let other_topic = w.node("topic", w.project, "Other").await;
    let other_thread = w.node("thread", other_topic, "Elsewhere").await;
    let admin = w.user("admin").await;
    w.give(w.topic, admin, Role::Admin).await;
    let caller = Caller::session(admin).with_admin_mode(true);
    assert!(!w.can(admin, w.thread, P::AccessManage).await);
    assert!(w.can_as(caller.clone(), w.thread, P::AccessManage).await);
    assert!(!w.can_as(caller, other_thread, P::AccessManage).await);
    let refused = w
        .change(
            admin,
            Target::Live(other_thread),
            json!({ "whole": "private" }),
        )
        .await;
    assert!(matches!(refused, Err(ChangeError::Denied(_))));
    let _ = Profile::Any;
}

#[tokio::test]
async fn naming_somebody_again_whose_entry_ended_names_them_afresh() {
    let w = World::new().await;
    let chen = w.member("chen").await;
    let named = |expires: &str| {
        let body = json!({ "rows": [{ "permission": "thread.view", "add": [{ "holder": user_holder(chen), "expires_at": expires, "note": "for a week" }] }] });
        w.change(w.maker, Target::Live(w.thread), body)
    };
    named("2026-10-08T14:00:01.000Z").await.unwrap();
    assert!(w.can(chen, w.thread, P::ThreadView).await, "for a week");
    // A week later it has ended and counts for nothing.
    let later = w.later(chrono::Duration::days(8));
    assert!(!w.can_at(chen, w.thread, P::ThreadView, later).await);
    // Named again without an end: from now on, with no end.
    let again = json!({ "rows": [{ "permission": "thread.view", "add": [{ "holder": user_holder(chen) }] }] });
    let outcome = w
        .change_at(
            Caller::session(w.maker),
            Target::Live(w.thread),
            again,
            orch_authz::Policy::default(),
            later,
        )
        .await
        .unwrap();
    assert!(outcome.changed);
    assert!(w.can_at(chen, w.thread, P::ThreadView, later).await);
    assert!(
        w.can_at(
            chen,
            w.thread,
            P::ThreadView,
            w.later(chrono::Duration::days(400))
        )
        .await,
        "no end"
    );
    let page = w.read(w.maker, Target::Live(w.thread)).await;
    let entry = &page
        .rows
        .iter()
        .find(|row| row.permission == P::ThreadView)
        .unwrap()
        .entries[0];
    assert_eq!(
        entry.note, "",
        "named afresh: the old note is not carried over"
    );
    let added = w
        .audit()
        .await
        .iter()
        .filter(|row| row.action == "permission.added")
        .count();
    assert_eq!(added, 2, "named twice");
}
