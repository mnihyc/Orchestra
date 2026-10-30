//! Generic access uses bounded giving and the holder's placement in the root tree.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission as P;
use orch_access_contract::roles::Role;
use orch_authz::pages::{ChangeError, Target};
use orch_authz::{Caller, Policy, TokenGrant, external};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::{access, identity, notifications::subscribers};
use orch_types::new_id;
use serde_json::json;
use support::World;

#[tokio::test]
async fn repeating_an_in_tree_group_and_role_across_rows_needs_no_outside_giving() {
    let w = World::new().await;
    let reader = w.member("reader").await;
    w.give(w.project, reader, Role::Viewer).await;
    let group = w.group(w.root, "Readers", &[reader]).await;
    // A member fixture has no domain role until given one; the source org must be visible.
    w.give(w.root, w.maker, Role::Viewer).await;
    w.change(w.maker, Target::Live(w.thread), json!({"whole": "private"}))
        .await
        .unwrap();
    assert!(!w.can(reader, w.thread, P::ThreadView).await);
    assert!(!w.can(w.maker, w.thread, P::AccessExternal).await);

    let holders = [
        Holder::Group { id: group },
        Holder::Role {
            role: Role::Viewer,
            at: Some(w.project),
        },
    ];
    w.change(
        w.maker,
        Target::Live(w.thread),
        json!({"rows": [
            {"permission": "thread.view", "add": [
                {"holder": holders[0]}, {"holder": holders[1]}
            ]},
            {"permission": "thread.interact", "add": [
                {"holder": holders[0]}, {"holder": holders[1]}
            ]}
        ]}),
    )
    .await
    .unwrap();
    assert!(w.can(reader, w.thread, P::ThreadView).await);
    assert!(w.can(reader, w.thread, P::ThreadInteract).await);
}

#[tokio::test]
async fn repeating_outside_or_missing_holders_still_requires_outside_giving() {
    let w = World::new().await;
    let outside_root = new_id();
    let mut conn = w.conn().await;
    insert_node(&mut conn, outside_root, "org", None, "Other", w.boss)
        .await
        .unwrap();
    drop(conn);
    let outside_group = w.group(outside_root, "Other readers", &[]).await;
    w.grant(
        "node",
        outside_root,
        P::OrgView,
        Holder::User { id: w.maker },
    )
    .await;
    for holder in [
        Holder::Group { id: outside_group },
        Holder::Role {
            role: Role::Viewer,
            at: Some(outside_root),
        },
        Holder::Group { id: new_id() },
        Holder::Node { id: new_id() },
    ] {
        let mut conn = w.conn().await;
        assert!(
            external::any_outside(&mut conn, w.root, &[holder, holder])
                .await
                .unwrap()
        );
    }
    let holder = Holder::Group { id: outside_group };
    let change = json!({"rows": [
        {"permission": "thread.view", "add": [{"holder": holder}]},
        {"permission": "thread.interact", "add": [{"holder": holder}]}
    ]});
    let refused = w
        .change(w.maker, Target::Live(w.thread), change.clone())
        .await;
    assert!(matches!(refused, Err(ChangeError::Outside { .. })));
    w.change_as(
        Caller::session(w.maker),
        Target::Live(w.thread),
        change,
        Policy {
            owners_name_outsiders: true,
            ..Policy::default()
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn outside_giving_does_not_let_a_giver_name_an_unseen_source_place() {
    let w = World::new().await;
    let other = new_id();
    let mut conn = w.conn().await;
    insert_node(&mut conn, other, "org", None, "Private other", w.boss)
        .await
        .unwrap();
    drop(conn);
    let group = w.group(other, "Private reviewers", &[]).await;
    let holder = Holder::Group { id: group };
    let change = json!({"rows":[{"permission":"thread.view","add":[{"holder":holder}]}]});
    let policy = Policy {
        owners_name_outsiders: true,
        ..Policy::default()
    };
    let refused = w
        .change_as(
            Caller::session(w.maker),
            Target::Live(w.thread),
            change.clone(),
            policy,
        )
        .await;
    assert!(
        matches!(refused, Err(ChangeError::Invalid(_))),
        "outside giving does not reveal a source"
    );
    w.grant("node", other, P::OrgView, Holder::User { id: w.maker })
        .await;
    w.change_as(
        Caller::session(w.maker),
        Target::Live(w.thread),
        change,
        policy,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn local_service_identities_need_no_outside_giving_and_gain_no_membership_or_notifications() {
    let w = World::new().await;
    let department = w.node("org", w.root, "Department").await;
    let mut conn = w.conn().await;
    for (org, username) in [(w.root, "root-bot"), (department, "dept-bot")] {
        let service = new_id();
        identity::create_service_account(
            &mut conn,
            service,
            org,
            username,
            username,
            w.boss,
            w.now.get(),
        )
        .await
        .unwrap();
        let role = w.give(w.topic, service, Role::Viewer).await;
        let flags = access::role_flags(&mut conn, &[role], w.root)
            .await
            .unwrap();
        assert!(!flags[0].outside);
        assert!(!flags[0].domain);
        let holder = Holder::User { id: service };
        assert!(
            !external::any_outside(&mut conn, w.root, &[holder, holder])
                .await
                .unwrap()
        );
        let caller = Caller::token(service, TokenGrant::everything());
        assert!(!w.can_as(caller.clone(), w.thread, P::ThreadInteract).await);
        w.change(
            w.maker,
            Target::Live(w.thread),
            json!({"rows": [{"permission": "thread.interact", "add": [{"holder": holder}]}]}),
        )
        .await
        .unwrap();
        assert!(w.can_as(caller.clone(), w.thread, P::ThreadInteract).await);
        assert!(w.principal(caller).await.member_of().is_empty());
        assert!(
            access::members_among(&mut conn, w.root, &[service])
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            !subscribers::follow(&mut conn, service, w.thread, w.now.get())
                .await
                .unwrap()
        );
        assert!(
            subscribers::people_among(&mut conn, &[service])
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            subscribers::level_of(&mut conn, service, w.thread)
                .await
                .unwrap(),
            "off"
        );
    }
}

#[tokio::test]
async fn a_service_identity_of_another_tree_stays_outside_even_with_a_role_here() {
    let w = World::new().await;
    let outside_root = new_id();
    let service = new_id();
    let mut conn = w.conn().await;
    insert_node(&mut conn, outside_root, "org", None, "Other", w.boss)
        .await
        .unwrap();
    identity::create_service_account(
        &mut conn,
        service,
        outside_root,
        "other-bot",
        "Other bot",
        w.boss,
        w.now.get(),
    )
    .await
    .unwrap();
    drop(conn);
    let role = w.give(w.topic, service, Role::Viewer).await;
    let holder = Holder::User { id: service };
    let mut conn = w.conn().await;
    let flags = access::role_flags(&mut conn, &[role], w.root)
        .await
        .unwrap();
    assert!(flags[0].outside);
    assert!(!flags[0].domain);
    assert!(
        external::any_outside(&mut conn, w.root, std::slice::from_ref(&holder))
            .await
            .unwrap()
    );
    assert!(
        access::members_among(&mut conn, w.root, &[service])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        subscribers::people_among(&mut conn, &[service])
            .await
            .unwrap()
            .is_empty()
    );
    let refused = w
        .change(
            w.maker,
            Target::Live(w.thread),
            json!({"rows": [{"permission": "thread.interact", "add": [{"holder": holder}]}]}),
        )
        .await;
    assert!(matches!(refused, Err(ChangeError::Outside { .. })));
    assert!(
        w.principal(Caller::token(service, TokenGrant::everything()))
            .await
            .member_of()
            .is_empty()
    );
}
