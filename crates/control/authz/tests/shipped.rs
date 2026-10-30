//! What the defaults as shipped give each role, read from stored rows.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::permissions::{Permission as P, Placement};
use orch_access_contract::roles::{NodeKind, Role};
use orch_authz::Caller;
use support::World;

#[tokio::test]
async fn roles_given_on_a_project_hold_on_a_thread_under_it_what_the_shipped_defaults_give() {
    let w = World::new().await;
    let viewer = w.user("viewer").await;
    let interactor = w.user("interactor").await;
    let developer = w.user("developer").await;
    let admin = w.user("admin").await;
    let nobody = w.user("nobody").await;
    w.give(w.project, viewer, Role::Viewer).await;
    w.give(w.project, interactor, Role::Interactor).await;
    w.give(w.project, developer, Role::Developer).await;
    w.give(w.project, admin, Role::Admin).await;

    // (who, thread.view, topic.interact, thread.interact, topic.manage, access.manage)
    let table = [
        (viewer, true, false, false, false, false),
        (interactor, true, false, true, false, false),
        (developer, true, true, true, true, false),
        (admin, true, true, true, true, true),
        (nobody, false, false, false, false, false),
        (w.boss, true, true, true, true, true),
        (w.maker, true, false, true, false, true),
    ];
    for (who, view, topic_interact, interact, topic_manage, manage) in table {
        let caller = Caller::session(who).with_admin_mode(who == admin || who == w.boss);
        assert_eq!(
            w.can(who, w.thread, P::ThreadView).await,
            view,
            "thread.view"
        );
        assert_eq!(
            w.can(who, w.topic, P::TopicInteract).await,
            topic_interact,
            "topic.interact"
        );
        assert_eq!(
            w.can(who, w.thread, P::ThreadInteract).await,
            interact,
            "thread.interact"
        );
        assert_eq!(
            w.can(who, w.topic, P::TopicManage).await,
            topic_manage,
            "topic.manage"
        );
        if who == admin || who == w.boss {
            assert!(!w.can(who, w.thread, P::AccessManage).await);
        }
        assert_eq!(
            w.can_as(caller, w.thread, P::AccessManage).await,
            manage,
            "access.manage"
        );
    }
}

#[tokio::test]
async fn every_shipped_default_is_read_by_role_on_a_thread_a_topic_a_project_and_an_org() {
    let w = World::new().await;
    let nodes = [
        (w.root, NodeKind::Org),
        (w.project, NodeKind::Project),
        (w.topic, NodeKind::Topic),
        (w.thread, NodeKind::Thread),
    ];
    for role in Role::ALL {
        let who = w.user(&format!("a-{role}")).await;
        w.give(w.root, who, *role).await;
        for (node, kind) in nodes {
            let held = w.holds(who, node).await;
            let administered = w
                .holds_as(
                    Caller::session(who).with_admin_mode(*role == Role::Admin),
                    node,
                )
                .await;
            for permission in support::permissions_of(kind) {
                // A row nobody is named in is the owners' alone. Whoever is named in a level
                // holds the levels below it and the advanced rows that belong with it.
                let by_a_row = std::iter::once(permission)
                    .chain(permission.held_through())
                    .filter_map(|row| row.shipped(Placement::Managed))
                    .any(|named| role.includes(named));
                assert_eq!(
                    held.contains(&permission),
                    by_a_row,
                    "ordinary {role} on a {kind}: {permission}"
                );
                // Enabled administrators manage the place's page as well as pages inside it.
                let by_administering = permission == P::AccessManage && *role == Role::Admin;
                assert_eq!(
                    administered.contains(&permission),
                    by_a_row || by_administering,
                    "{role} on a {kind} with its administration mode selected: {permission}"
                );
            }
        }
    }
}
