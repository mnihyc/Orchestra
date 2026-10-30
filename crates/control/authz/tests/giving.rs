//! Who may share, and how much: whoever holds may name others further in what they hold, nobody gives more than they hold, a place may keep sharing to its owners and to whoever manages the page, and a page says beforehand what its reader may give.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::Refusal;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{Choice, Permission as P};
use orch_access_contract::roles::{PlaceRef, Role};
use orch_authz::giving::{self, AgentLevel};
use orch_authz::pages::{self, ChangeError, PageView, Target};
use orch_authz::{Caller, Policy};
use orch_types::Id;
use serde_json::json;
use support::{World, role_holder, user_holder};

fn policy(holders_share: bool) -> Policy {
    Policy {
        holders_share,
        ..Policy::default()
    }
}

async fn page_as(w: &World, user: Id, target: Target, policy: Policy) -> PageView {
    let principal = w.principal(Caller::session(user)).await;
    let mut conn = w.conn().await;
    pages::read(&mut conn, &principal, target, policy)
        .await
        .unwrap()
}

/// Somebody of the root org with a role in the topic, and nothing else: they own nothing and manage no page.
async fn holder_by_role_alone(w: &World, name: &str, role: Role) -> Id {
    let user = w.member(name).await;
    w.give(w.topic, user, role).await;
    user
}

fn refusal(result: Result<pages::Outcome, ChangeError>) -> Box<Refusal> {
    match result {
        Err(ChangeError::Denied(refusal)) => refusal,
        other => panic!("a refusal was expected, and this came: {other:?}"),
    }
}

#[tokio::test]
async fn a_page_says_beforehand_exactly_what_a_change_by_its_reader_is_not_refused_for() {
    let w = World::new().await;
    let thread = Target::Live(w.thread);
    let viewer = holder_by_role_alone(&w, "viewer", Role::Viewer).await;
    let interactor = holder_by_role_alone(&w, "interactor", Role::Interactor).await;
    let developer = holder_by_role_alone(&w, "developer", Role::Developer).await;
    let readers = [
        ("a Viewer", viewer),
        ("an Interactor", interactor),
        ("a Developer", developer),
        ("the owner of the thread", w.maker),
        ("an admin of its place", w.boss),
    ];
    let mut guests = 0_u32;
    for holders_share in [true, false] {
        for (who, user) in readers {
            let page = page_as(&w, user, thread, policy(holders_share)).await;
            assert_eq!(page.gives.holders_share, holders_share, "{who}");
            for row in &page.rows {
                guests += 1;
                let guest = w.member(&format!("row{guests}")).await;
                let made = w
                    .change_as(
                        Caller::session(user),
                        thread,
                        json!({ "rows": [{ "permission": row.permission.as_str(),
                                           "add": [{ "holder": user_holder(guest) }] }] }),
                        policy(holders_share),
                    )
                    .await;
                assert_eq!(
                    made.is_ok(),
                    row.can_give,
                    "{who}, holders share {holders_share}, the row {}: the page said {}",
                    row.permission,
                    row.can_give
                );
            }
            for choice in [Choice::FullView, Choice::FullInteract, Choice::FullManage] {
                guests += 1;
                let guest = w.member(&format!("choice{guests}")).await;
                let made = w
                    .change_as(
                        Caller::session(user),
                        thread,
                        json!({ "name": [{ "holders": [user_holder(guest)],
                                           "choice": choice.as_str() }] }),
                        policy(holders_share),
                    )
                    .await;
                assert_eq!(
                    made.is_ok(),
                    page.gives.choices.contains(&choice),
                    "{who}, holders share {holders_share}, {choice:?}: the page said {:?}",
                    page.gives.choices
                );
            }
        }
    }
}

#[tokio::test]
async fn a_holder_gives_what_their_role_holds_and_is_refused_above_it_with_everything_they_lack() {
    let w = World::new().await;
    let thread = Target::Live(w.thread);
    let interactor = holder_by_role_alone(&w, "interactor", Role::Interactor).await;
    let developer = holder_by_role_alone(&w, "developer", Role::Developer).await;
    let guest = w.member("guest").await;
    let page = page_as(&w, interactor, thread, policy(true)).await;
    assert_eq!(
        page.gives.choices,
        [Choice::FullView, Choice::FullInteract],
        "an Interactor gives up to what an Interactor has"
    );
    let before = page.revision;
    let made = refusal(
        w.change_as(
            Caller::session(interactor),
            thread,
            json!({ "name": [{ "holders": [user_holder(guest)], "choice": "full_manage" }] }),
            policy(true),
        )
        .await,
    );
    let mut lacking: Vec<P> = made
        .missing
        .iter()
        .flat_map(|entry| entry.permissions.iter().copied())
        .collect();
    lacking.sort();
    let mut expected = vec![P::ChatManage, P::DocManage, P::ThreadManage];
    expected.sort();
    assert_eq!(
        lacking, expected,
        "everything the giver lacks, not the first"
    );
    assert_eq!(
        page_as(&w, w.maker, thread, policy(true)).await.revision,
        before,
        "refused whole: nothing was changed"
    );
    assert!(
        !w.can(guest, w.thread, P::ThreadInteract).await,
        "not even the part of it the giver holds"
    );
    // A Developer gives the whole of it.
    let gave = w
        .change_as(
            Caller::session(developer),
            thread,
            json!({ "name": [{ "holders": [user_holder(guest)], "choice": "full_manage" }] }),
            policy(true),
        )
        .await
        .unwrap();
    assert!(gave.changed);
    assert!(w.can(guest, w.thread, P::ThreadManage).await);
}

#[tokio::test]
async fn where_the_place_keeps_sharing_to_its_owners_a_holder_is_refused_and_told_who_can() {
    let w = World::new().await;
    let thread = Target::Live(w.thread);
    let developer = holder_by_role_alone(&w, "developer", Role::Developer).await;
    let guest = w.member("guest").await;
    let page = page_as(&w, developer, thread, policy(false)).await;
    assert!(!page.gives.holders_share);
    assert!(
        page.gives.choices.is_empty()
            && page.gives.roles.is_empty()
            && page.gives.agents.is_empty()
    );
    assert!(page.rows.iter().all(|row| !row.can_give));
    let made = refusal(
        w.change_as(
            Caller::session(developer),
            thread,
            json!({ "name": [{ "holders": [user_holder(guest)], "choice": "full_view" }] }),
            policy(false),
        )
        .await,
    );
    assert_eq!(made.required, P::AccessManage);
    let ask: Vec<Id> = made
        .missing
        .iter()
        .flat_map(|entry| entry.ask.iter().copied())
        .collect();
    assert!(
        ask.contains(&w.maker) && ask.contains(&w.boss),
        "the owner and the admin of its place can: {ask:?}"
    );
    // The owner and whoever manages the page share all the same.
    for (name, sharer) in [("by_the_owner", w.maker), ("by_the_admin", w.boss)] {
        let other = w.member(name).await;
        assert!(
            w.change_as(
                Caller::session(sharer).with_admin_mode(sharer == w.boss),
                thread,
                json!({ "name": [{ "holders": [user_holder(other)], "choice": "full_view" }] }),
                policy(false),
            )
            .await
            .unwrap()
            .changed
        );
    }
    // So does somebody who holds access.manage through an entry, up to what they hold.
    let manager = holder_by_role_alone(&w, "manager", Role::Viewer).await;
    w.name(w.thread, P::AccessManage, Holder::User { id: manager })
        .await;
    let other = w.member("another").await;
    assert!(
        w.change_as(
            Caller::session(manager),
            thread,
            json!({ "name": [{ "holders": [user_holder(other)], "choice": "full_view" }] }),
            policy(false),
        )
        .await
        .unwrap()
        .changed
    );
}

#[tokio::test]
async fn a_sharer_takes_back_what_they_gave_and_not_what_others_gave_and_manages_nothing_else() {
    let w = World::new().await;
    let thread = Target::Live(w.thread);
    let developer = holder_by_role_alone(&w, "developer", Role::Developer).await;
    let (first, second) = (w.member("first").await, w.member("second").await);
    let one = |guest: Id| json!({ "rows": [{ "permission": "thread.view", "add": [{ "holder": user_holder(guest) }] }] });
    w.change_as(Caller::session(developer), thread, one(first), policy(true))
        .await
        .unwrap();
    w.change_as_admin(w.boss, thread, one(second))
        .await
        .unwrap();
    let page = page_as(&w, developer, thread, policy(true)).await;
    let row = page
        .rows
        .iter()
        .find(|row| row.permission == P::ThreadView)
        .unwrap();
    let named = |guest: Id| {
        row.entries
            .iter()
            .find(|entry| entry.holder == Holder::User { id: guest })
            .unwrap()
    };
    assert_eq!(
        named(first).created_by,
        Some(developer),
        "every entry says who gave it"
    );
    assert_eq!(named(second).created_by, Some(w.boss));
    let take = |entry: Id| json!({ "rows": [{ "permission": "thread.view", "remove": [entry] }] });
    // What somebody else gave stays, and nothing is changed.
    assert!(matches!(
        w.change_as(
            Caller::session(developer),
            thread,
            take(named(second).id),
            policy(true)
        )
        .await,
        Err(ChangeError::Denied(_))
    ));
    // The sharer's own can be taken back, while they could give it again.
    assert!(
        w.change_as(
            Caller::session(developer),
            thread,
            take(named(first).id),
            policy(true)
        )
        .await
        .unwrap()
        .changed
    );
    // Everything else that takes away takes access.manage.
    for change in [
        json!({ "rows": [{ "permission": "thread.view", "inherit": false }] }),
        json!({ "whole": "private" }),
        json!({ "whole": "reset" }),
        json!({ "owners": { "add": [user_holder(first)] } }),
        json!({ "rows": [{ "permission": "thread.view",
                           "update": [{ "entry_id": named(second).id, "note": "mine now" }] }] }),
    ] {
        assert!(
            matches!(
                w.change_as(
                    Caller::session(developer),
                    thread,
                    change.clone(),
                    policy(true)
                )
                .await,
                Err(ChangeError::Denied(_))
            ),
            "{change}"
        );
    }
    // Naming again somebody an owner named, to say more of them, is changing what the owner made.
    assert!(matches!(
        w.change_as(
            Caller::session(developer),
            thread,
            json!({ "rows": [{ "permission": "thread.view",
                               "add": [{ "holder": user_holder(second), "note": "mine now" }] }] }),
            policy(true),
        )
        .await,
        Err(ChangeError::Denied(_))
    ));
    // A defaults page is changed by whoever manages the place.
    let topic = Target::Defaults(PlaceRef::Node(w.topic));
    assert!(matches!(
        w.change_as(
            Caller::session(developer),
            topic,
            json!({ "rows": [{ "permission": "thread.view", "profile": "managed",
                               "add": [{ "holder": role_holder(Role::Viewer) }] }] }),
            policy(true),
        )
        .await,
        Err(ChangeError::Denied(_))
    ));
    // And once the place keeps sharing to its owners, the sharer cannot take back what they gave either.
    let third = w.member("third").await;
    w.change_as(Caller::session(developer), thread, one(third), policy(true))
        .await
        .unwrap();
    let page = page_as(&w, developer, thread, policy(true)).await;
    let third_entry = page
        .rows
        .iter()
        .find(|row| row.permission == P::ThreadView)
        .unwrap()
        .entries
        .iter()
        .find(|entry| entry.holder == Holder::User { id: third })
        .unwrap()
        .id;
    assert!(matches!(
        w.change_as(
            Caller::session(developer),
            thread,
            take(third_entry),
            policy(false)
        )
        .await,
        Err(ChangeError::Denied(_))
    ));
}

#[tokio::test]
async fn explicit_administration_gives_native_interact_without_a_self_naming_override() {
    let w = World::new().await;
    let thread = Target::Live(w.thread);
    w.change(w.maker, thread, json!({ "whole": "private" }))
        .await
        .unwrap();
    let guest = w.member("guest").await;
    let give = json!({ "rows": [{ "permission": "thread.interact",
                                  "add": [{ "holder": user_holder(guest) }] }] });
    // The ordinary session does not administer the private page.
    let made = refusal(w.change(w.boss, thread, give.clone()).await);
    assert!(
        !made.r#override,
        "administration is not offered while the mode is off"
    );
    assert_eq!(made.required, P::AccessManage);
    let given = w.change_as_admin(w.boss, thread, give).await.unwrap();
    assert!(given.changed && !given.r#override);
    assert!(w.audit().await.iter().all(|row| !row.r#override));
    assert!(
        !w.can(w.boss, w.thread, P::ThreadInteract).await,
        "sharing native reach creates no self entry"
    );
    assert!(w.can(guest, w.thread, P::ThreadInteract).await);
}

#[tokio::test]
async fn only_an_admin_gives_a_role_in_a_place_and_nobody_gives_one_above_their_own() {
    let w = World::new().await;
    let people = [
        (
            "a Viewer",
            holder_by_role_alone(&w, "viewer", Role::Viewer).await,
            vec![Role::Viewer],
        ),
        (
            "an Interactor",
            holder_by_role_alone(&w, "interactor", Role::Interactor).await,
            vec![Role::Viewer, Role::Interactor],
        ),
        (
            "a Developer",
            holder_by_role_alone(&w, "developer", Role::Developer).await,
            vec![Role::Viewer, Role::Interactor, Role::Developer],
        ),
        ("an owner of the place", w.boss, Role::ALL.to_vec()),
    ];
    for (who, user, expected) in people {
        let principal = w.principal(Caller::session(user)).await;
        let mut conn = w.conn().await;
        let on = giving::role_standing(&mut conn, &principal, w.topic, policy(true))
            .await
            .unwrap();
        assert_eq!(on.roles(), expected, "{who}, where holders share");
        assert_eq!(on.is_admin(), expected.contains(&Role::Admin), "{who}");
        let off = giving::role_standing(&mut conn, &principal, w.topic, policy(false))
            .await
            .unwrap();
        assert_eq!(
            off.roles(),
            if user == w.boss {
                Role::ALL.to_vec()
            } else {
                Vec::new()
            },
            "{who}, where the place keeps sharing to its owners"
        );
        drop(conn);
        let page = page_as(&w, user, Target::Live(w.topic), policy(true)).await;
        assert_eq!(page.gives.roles, expected, "{who}: the page says the same");
    }
}

#[tokio::test]
async fn the_defaults_of_a_place_are_given_by_those_who_manage_it_up_to_what_a_thing_that_follows_gives_them()
 {
    let w = World::new().await;
    let interactor = w.member("interactor").await;
    w.give(w.topic, interactor, Role::Interactor).await;
    // Named in the row that lets them change the page of the topic, they manage its defaults too.
    w.name(w.topic, P::AccessManage, Holder::User { id: interactor })
        .await;
    let defaults = |permission: &str| {
        json!({ "rows": [{ "permission": permission, "profile": "managed",
                           "add": [{ "holder": role_holder(Role::Viewer) }] }] })
    };
    // They hold the right to take part in a thread that follows the row, not the right to manage one.
    assert!(
        w.change_defaults(interactor, w.topic, defaults("thread.interact"))
            .await
            .unwrap()
            .changed
    );
    let made = refusal(
        w.change_defaults(interactor, w.topic, defaults("thread.manage"))
            .await,
    );
    assert!(
        made.missing
            .iter()
            .any(|entry| entry.permissions.contains(&P::ThreadManage))
    );
    // What the page of the defaults says beforehand is the same.
    let page = page_as(
        &w,
        interactor,
        Target::Defaults(PlaceRef::Node(w.topic)),
        policy(true),
    )
    .await;
    let can = |permission: P| {
        page.rows
            .iter()
            .filter(|row| row.permission == permission)
            .any(|row| row.can_give)
    };
    assert!(can(P::ThreadInteract) && !can(P::ThreadManage));
    // The owner of the place, an Admin there, sets any row of what its pages start from.
    assert!(
        w.change_defaults(w.boss, w.topic, defaults("thread.manage"))
            .await
            .unwrap()
            .changed
    );
}

#[tokio::test]
async fn sharing_with_agents_is_offered_up_to_what_the_sharer_holds_in_every_area_the_thing_has() {
    let w = World::new().await;
    let viewer = holder_by_role_alone(&w, "viewer", Role::Viewer).await;
    let interactor = holder_by_role_alone(&w, "interactor", Role::Interactor).await;
    let developer = holder_by_role_alone(&w, "developer", Role::Developer).await;
    let (thread, topic) = (Target::Live(w.thread), Target::Live(w.topic));
    for (who, user, targets, expected) in [
        (
            "a Viewer",
            viewer,
            vec![thread, topic],
            vec![AgentLevel::View],
        ),
        (
            "an Interactor",
            interactor,
            vec![thread, topic],
            vec![AgentLevel::View, AgentLevel::Edit],
        ),
        (
            "a Developer, who manages documents as shipped",
            developer,
            vec![thread, topic],
            vec![AgentLevel::View, AgentLevel::Edit, AgentLevel::Manage],
        ),
        (
            "the owner of the thread",
            w.maker,
            vec![thread],
            vec![AgentLevel::View, AgentLevel::Edit, AgentLevel::Manage],
        ),
    ] {
        for target in targets {
            let page = page_as(&w, user, target, policy(true)).await;
            assert_eq!(page.gives.agents, expected, "{who}");
        }
        let off = page_as(&w, user, thread, policy(false)).await;
        assert_eq!(
            off.gives.agents.is_empty(),
            user != w.maker,
            "{who}: where the place keeps sharing to its owners, only they share with agents"
        );
    }
}
