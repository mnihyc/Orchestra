//! People and roles of another org: a thread shared on its own, a role given in a project, another org's role named on a page; and the admin against a page that follows and one made private.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::Decision;
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::{Permission as P, PermissionName};
use orch_access_contract::roles::{NodeKind, Role};
use orch_authz::pages::Target;
use orch_authz::{Act, Caller, Error, Policy, nodes};
use orch_db::queries::access::testing::{give_role, take_role_away};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_types::{Id, new_id};
use serde_json::json;
use support::{World, user_holder};

async fn another_root_org(w: &World, name: &str) -> Id {
    let mut conn = w.conn().await;
    let id = new_id();
    insert_node(&mut conn, id, "org", None, name, w.boss)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn somebody_named_on_one_thread_holds_that_thread_and_its_ancestors_appear_to_them_redacted()
{
    let w = World::new().await;
    let guest = w.user("guest").await;
    let policy = Policy {
        owners_name_outsiders: true,
        ..Policy::default()
    };
    w.change_as(
        Caller::session(w.maker),
        Target::Live(w.thread),
        json!({ "name": [{ "holders": [user_holder(guest)], "choice": "full_view" }] }),
        policy,
    )
    .await
    .unwrap();
    assert!(w.can(guest, w.thread, P::ThreadView).await);
    assert!(
        !w.can(guest, w.topic, P::TopicView).await,
        "nothing on the topic"
    );
    assert!(!w.can(guest, w.project, P::ProjectView).await);
    let principal = w.principal(Caller::session(guest)).await;
    let mut conn = w.conn().await;
    let ancestors = nodes::ancestors(&mut conn, &principal, w.thread, Policy::default())
        .await
        .unwrap();
    assert_eq!(ancestors.len(), 3);
    assert!(
        ancestors
            .iter()
            .all(|ancestor| ancestor.redacted && ancestor.name.is_none())
    );
    assert_eq!(
        ancestors
            .iter()
            .map(|ancestor| ancestor.kind)
            .collect::<Vec<_>>(),
        [NodeKind::Org, NodeKind::Project, NodeKind::Topic]
    );
    // The thread is the only child of the topic they see.
    let other = w.node("thread", w.topic, "Other").await;
    let listed = nodes::holding(
        &mut conn,
        &principal,
        &[other, w.thread],
        PermissionName::OfNode(orch_access_contract::permissions::NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        listed.iter().map(|item| item.id).collect::<Vec<_>>(),
        [w.thread]
    );
    // Holding anything there shows a name: the guest is also given Viewer on the topic.
    w.give(w.topic, guest, Role::Viewer).await;
    let ancestors = nodes::ancestors(&mut conn, &principal, w.thread, Policy::default())
        .await
        .unwrap();
    assert_eq!(ancestors[2].name.as_deref(), Some("Login"));
    assert!(ancestors[0].redacted && ancestors[1].redacted);
}

#[tokio::test]
async fn a_person_of_another_org_given_a_role_in_a_project_holds_what_its_pages_give_that_role_and_nothing_else_of_the_tree()
 {
    let w = World::new().await;
    let guest = w.user("guest").await;
    let other_project = w.node("project", w.root, "Elsewhere").await;
    let other_thread = w
        .node(
            "thread",
            w.node("topic", other_project, "Else").await,
            "Else",
        )
        .await;
    w.give(w.project, guest, Role::Developer).await;
    assert!(w.can(guest, w.thread, P::ThreadManage).await);
    assert!(w.can(guest, w.topic, P::TopicManage).await);
    assert!(
        !w.can(guest, other_thread, P::ThreadView).await,
        "not its other projects"
    );
    assert!(!w.can(guest, w.root, P::OrgView).await, "nor the org");
    assert!(
        w.principal(Caller::session(guest))
            .await
            .member_of()
            .is_empty(),
        "a role makes nobody a member"
    );
}

#[tokio::test]
async fn another_orgs_role_named_on_a_page_is_held_for_as_long_as_it_is_held_in_that_org() {
    let w = World::new().await;
    let globex = another_root_org(&w, "Globex").await;
    let (visitor, outsider) = (w.user("visitor").await, w.user("outsider").await);
    let mut conn = w.conn().await;
    let given = give_role(
        &mut conn,
        globex,
        "user",
        visitor,
        "developer",
        w.boss,
        None,
    )
    .await
    .unwrap();
    drop(conn);
    // "The Developers of Globex" in the row of a thread of Acme.
    w.name(
        w.thread,
        P::ThreadInteract,
        Holder::Role {
            role: Role::Developer,
            at: Some(globex),
        },
    )
    .await;
    assert!(
        w.can(visitor, w.thread, P::ThreadInteract).await,
        "they hold that role there"
    );
    assert!(!w.can(outsider, w.thread, P::ThreadInteract).await);
    assert!(
        !w.can(visitor, w.thread, P::ThreadManage).await,
        "only what that row gives"
    );
    // Lose the role in their own org and it is lost here.
    let mut conn = w.conn().await;
    take_role_away(&mut conn, given, w.boss, w.now.get())
        .await
        .unwrap();
    drop(conn);
    assert!(!w.can(visitor, w.thread, P::ThreadInteract).await);
    // A person suspended in that org holds nothing there, and so no role of it.
    let mut conn = w.conn().await;
    give_role(
        &mut conn,
        globex,
        "user",
        visitor,
        "developer",
        w.boss,
        None,
    )
    .await
    .unwrap();
    assert!(w.can(visitor, w.thread, P::ThreadInteract).await);
    orch_db::queries::access::testing::suspend_in(&mut conn, globex, visitor, w.boss)
        .await
        .unwrap();
    drop(conn);
    assert!(!w.can(visitor, w.thread, P::ThreadInteract).await);
}

#[tokio::test]
async fn each_kind_of_administrator_selects_native_reach_explicitly_on_following_and_private_pages()
{
    let w = World::new().await;
    let d = w.deep().await;
    let kinds: Vec<(&str, Option<(Id, Role)>)> = vec![
        ("an Admin of the root org", Some((w.root, Role::Admin))),
        ("an Admin of an org inside", Some((d.d3, Role::Admin))),
        ("an Admin of the project", Some((d.project, Role::Admin))),
        ("an Admin of the topic", Some((d.topic, Role::Admin))),
        ("a system admin", None),
    ];
    for (index, (what, place)) in kinds.into_iter().enumerate() {
        let admin = w.member(&format!("admin{index}")).await;
        match place {
            Some((place, role)) => {
                w.give(place, admin, role).await;
            }
            None => {
                let mut conn = w.conn().await;
                orch_db::queries::access::testing::make_system_admin(&mut conn, admin)
                    .await
                    .unwrap();
            }
        }
        // A page that follows its defaults: what the row gives Developers and Viewers, and the page itself.
        for held in [
            P::ThreadView,
            P::ThreadInteract,
            P::ThreadManage,
            P::AccessManage,
        ] {
            assert!(
                w.can_as(Caller::session(admin).with_admin_mode(true), d.thread, held)
                    .await,
                "{what}: {held} on a page that follows"
            );
        }
        // A private page stays closed ordinarily and is available through explicit native reach.
        let private = w.node("thread", d.topic, "Private").await;
        w.change(
            w.maker,
            Target::Live(private),
            json!({ "whole": "private" }),
        )
        .await
        .unwrap();
        assert!(
            !w.can(admin, private, P::ThreadView).await,
            "{what}: the content is refused"
        );
        assert!(!w.can(admin, private, P::AccessManage).await);
        for permission in [
            P::ThreadView,
            P::ThreadInteract,
            P::ThreadManage,
            P::AccessManage,
        ] {
            assert!(
                w.can_as(
                    Caller::session(admin).with_admin_mode(true),
                    private,
                    permission
                )
                .await,
                "{what}: {permission}"
            );
        }
        let Decision::Refused(refusal) = w
            .decide(Caller::session(admin), private, P::ThreadView, Act::Use)
            .await
        else {
            panic!("{what}: refused")
        };
        assert!(
            !refusal.r#override,
            "{what}: an ordinary session offers no administrative override"
        );
    }
    // Somebody who is no admin of the place is not offered the override.
    let developer = w.member("developer").await;
    w.give(d.project, developer, Role::Developer).await;
    let private = w.node("thread", d.topic, "Closed").await;
    w.change(
        w.maker,
        Target::Live(private),
        json!({ "whole": "private" }),
    )
    .await
    .unwrap();
    let Decision::Refused(refusal) = w
        .decide(
            Caller::session(developer),
            private,
            P::ThreadInteract,
            Act::Use,
        )
        .await
    else {
        panic!("refused")
    };
    assert!(!refusal.r#override);
}

#[tokio::test]
async fn a_permission_is_named_by_the_shorthand_for_the_kind_of_node_it_is_asked_of() {
    let w = World::new().await;
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut conn = w.conn().await;
    let name = |text: &str| text.parse::<PermissionName>().unwrap();
    for (node, text) in [
        (w.thread, "node.interact"),
        (w.topic, "node.interact"),
        (w.project, "node.manage"),
        (w.root, "node.view"),
    ] {
        let decision = nodes::check_named(
            &mut conn,
            &principal,
            node,
            name(text),
            Act::Use,
            Policy::default(),
        )
        .await
        .unwrap();
        assert!(decision.is_allowed(), "{text} on {node}");
    }
    // The archive shorthand includes an org and follows its Manage level.
    let archive = nodes::check_named(
        &mut conn,
        &principal,
        w.root,
        name("node.archive"),
        Act::Use,
        Policy::default(),
    )
    .await;
    assert!(archive.unwrap().is_allowed(), "the owner manages this org");
    let no_such_row = nodes::check_named(
        &mut conn,
        &principal,
        w.root,
        name("thread.interact"),
        Act::Use,
        Policy::default(),
    )
    .await;
    assert!(matches!(no_such_row, Err(Error::NoSuchPermission { .. })));
    // A node that is not there, or was deleted, is not found.
    let missing = nodes::check(
        &mut conn,
        &principal,
        Id::nil(),
        P::ThreadView,
        Act::Use,
        Policy::default(),
    )
    .await;
    assert!(matches!(missing, Err(Error::NotFound(_))));
    orch_db::queries::hierarchy::testing::soft_delete(&mut conn, w.thread, w.boss, w.now.get())
        .await
        .unwrap();
    let deleted = nodes::check(
        &mut conn,
        &principal,
        w.thread,
        P::ThreadView,
        Act::Use,
        Policy::default(),
    )
    .await;
    assert!(matches!(deleted, Err(Error::NotFound(_))));
}

#[tokio::test]
async fn a_new_things_page_is_its_makers_owners_row_and_nothing_else() {
    let w = World::new().await;
    let thread = {
        let mut conn = w.conn().await;
        let id = new_id();
        insert_node(&mut conn, id, "thread", Some(w.topic), "Brand new", w.maker)
            .await
            .unwrap();
        let before = orch_authz::epoch::current(&mut conn).await.unwrap();
        orch_authz::pages::start(
            &mut conn,
            orch_access_contract::pages::StoredKind::Node,
            id,
            w.maker,
            w.now,
        )
        .await
        .unwrap();
        assert_eq!(
            orch_authz::epoch::current(&mut conn).await.unwrap(),
            before + 1
        );
        id
    };
    let page = w.read(w.maker, Target::Live(thread)).await;
    assert_eq!(page.owners.len(), 1);
    assert_eq!(page.state, orch_access_contract::pages::PageState::Follows);
    assert!(
        page.rows
            .iter()
            .all(|row| row.inherit && row.entries.is_empty())
    );
    assert!(
        w.can(w.maker, thread, P::ThreadManage).await,
        "its maker owns it"
    );
    assert!(
        !w.can(w.maker, thread, P::AccessExternal).await,
        "owning does not give the right to name outsiders"
    );
}
