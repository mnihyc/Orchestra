//! Who holds what by the roles they have: where a role is held, groups, members of orgs, owners, and orgs inside orgs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use chrono::Duration;
use orch_access_contract::permissions::Permission as P;
use orch_access_contract::roles::{PlaceRef, Role};
use orch_authz::{Caller, Policy, nodes};
use orch_db::queries::access::testing::{
    give_role, insert_member, name_owner, remove_from_group, suspend_in, take_role_away,
};
use support::World;

#[tokio::test]
async fn a_role_reaches_the_place_it_is_held_in_and_everything_inside_it_and_nothing_beside_or_above()
 {
    let w = World::new().await;
    let d = w.deep().await;
    let places = [
        ("the topic", d.topic, true),
        ("the project", d.project, true),
        ("the org it lies in", d.d3, true),
        ("an org above", d.d2, true),
        ("two orgs above", d.d1, true),
        ("the root org", w.root, true),
        ("a sibling topic", d.other_topic, false),
        ("a department beside an org above", d.ops, false),
        ("a department beside the root's child", d.sales, false),
        ("a project of the root org", w.project, false),
    ];
    for (index, (what, place, expected)) in places.into_iter().enumerate() {
        if place == d.thread {
            continue;
        }
        let who = w.user(&format!("dev{index}")).await;
        w.give(place, who, Role::Developer).await;
        assert_eq!(
            w.can(who, d.thread, P::ThreadManage).await,
            expected,
            "a Developer on {what}"
        );
    }
}

#[tokio::test]
async fn a_role_on_one_department_reaches_the_departments_in_it_and_not_the_root_orgs_own_projects()
{
    let w = World::new().await;
    let d = w.deep().await;
    let viewer = w.user("viewer").await;
    w.give(d.d1, viewer, Role::Viewer).await;
    for (what, node, permission, expected) in [
        ("the department", d.d1, P::OrgView, true),
        ("a department in it", d.d2, P::OrgView, true),
        ("two levels in", d.d3, P::OrgView, true),
        (
            "its sibling's project",
            d.sales_project,
            P::ProjectView,
            false,
        ),
        ("a sibling department", d.sales, P::OrgView, false),
        ("the root org", w.root, P::OrgView, false),
        (
            "the root org's own project",
            w.project,
            P::ProjectView,
            false,
        ),
        ("a project inside", d.project, P::ProjectView, true),
    ] {
        assert_eq!(w.can(viewer, node, permission).await, expected, "{what}");
    }
    let root_viewer = w.user("rootviewer").await;
    w.give(w.root, root_viewer, Role::Viewer).await;
    for node in [d.d3, d.sales, d.ops] {
        assert!(
            w.can(root_viewer, node, P::OrgView).await,
            "a role on the root org reaches every org in it"
        );
    }
    assert!(w.can(root_viewer, d.project, P::ProjectView).await);
    assert!(w.can(root_viewer, w.project, P::ProjectView).await);
}

#[tokio::test]
async fn each_role_holds_its_rows_and_enabled_admins_also_manage_pages() {
    let w = World::new().await;
    let d = w.deep().await;
    // (role, thread.view, thread.interact, thread.manage, access.manage)
    for (role, view, interact, manage, access) in [
        (None, false, false, false, false),
        (Some(Role::Viewer), true, false, false, false),
        (Some(Role::Interactor), true, true, false, false),
        (Some(Role::Developer), true, true, true, false),
        (Some(Role::Admin), true, true, true, true),
    ] {
        let who = w
            .user(&format!("p-{}", role.map_or("none", Role::as_str)))
            .await;
        if let Some(role) = role {
            w.give(d.project, who, role).await;
        }
        let caller = Caller::session(who).with_admin_mode(role == Some(Role::Admin));
        assert_eq!(w.can(who, d.thread, P::ThreadView).await, view);
        assert_eq!(w.can(who, d.thread, P::ThreadInteract).await, interact);
        assert_eq!(w.can(who, d.thread, P::ThreadManage).await, manage);
        assert!(!w.can(who, d.thread, P::AccessManage).await);
        assert_eq!(w.can_as(caller, d.thread, P::AccessManage).await, access);
    }
}

#[tokio::test]
async fn a_role_given_to_a_group_is_held_by_its_people_for_as_long_as_they_are_in_it() {
    let w = World::new().await;
    let d = w.deep().await;
    let (member, outsider) = (w.member("member").await, w.user("elsewhere").await);
    let group = w.group(d.d2, "Reviewers", &[member]).await;
    let mut conn = w.conn().await;
    give_role(
        &mut conn,
        d.project,
        "group",
        group,
        "developer",
        w.boss,
        None,
    )
    .await
    .unwrap();
    drop(conn);
    assert!(
        w.can(member, d.thread, P::ThreadManage).await,
        "a group kept by a department"
    );
    assert!(!w.can(outsider, d.thread, P::ThreadManage).await);
    let mut conn = w.conn().await;
    remove_from_group(&mut conn, group, member).await.unwrap();
    drop(conn);
    assert!(
        !w.can(member, d.thread, P::ThreadManage).await,
        "taken out of the group, nothing is held"
    );
}

#[tokio::test]
async fn a_group_of_another_root_org_is_named_like_any_other() {
    let w = World::new().await;
    let guest = w.user("guest").await;
    let other_root = {
        let mut conn = w.conn().await;
        let id = orch_types::new_id();
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn, id, "org", None, "Globex", w.boss,
        )
        .await
        .unwrap();
        id
    };
    let group = w.group(other_root, "Contractors", &[guest]).await;
    let mut conn = w.conn().await;
    give_role(
        &mut conn,
        w.project,
        "group",
        group,
        "interactor",
        w.boss,
        None,
    )
    .await
    .unwrap();
    drop(conn);
    assert!(w.can(guest, w.thread, P::ThreadInteract).await);
    assert!(!w.can(guest, w.thread, P::ThreadManage).await);
}

#[tokio::test]
async fn the_members_of_an_org_hold_a_role_given_to_it_by_the_domain_they_belong_to() {
    let w = World::new().await;
    let d = w.deep().await;
    // Members by domain: three levels down, the department above that, the root, a sibling, and none.
    let (deep_member, ml_member, root_member, sales_member, nobody) = (
        w.user("deepmember").await,
        w.user("mlmember").await,
        w.user("rootmember").await,
        w.user("salesmember").await,
        w.user("nobody").await,
    );
    let mut conn = w.conn().await;
    for (user, domain) in [
        (deep_member, d.d3),
        (ml_member, d.d2),
        (root_member, w.root),
        (sales_member, d.sales),
    ] {
        insert_member(&mut conn, w.root, user, domain)
            .await
            .unwrap();
    }
    // The members of ML (d2) are Interactors in the project.
    give_role(
        &mut conn,
        d.project,
        "members",
        d.d2,
        "interactor",
        w.boss,
        None,
    )
    .await
    .unwrap();
    drop(conn);
    assert!(
        w.can(deep_member, d.thread, P::ThreadInteract).await,
        "their domain is inside ML"
    );
    assert!(
        w.can(ml_member, d.thread, P::ThreadInteract).await,
        "ML itself"
    );
    assert!(
        !w.can(root_member, d.thread, P::ThreadInteract).await,
        "a member of the root is no member of ML"
    );
    assert!(
        !w.can(sales_member, d.thread, P::ThreadInteract).await,
        "a member of a sibling department"
    );
    assert!(!w.can(nobody, d.thread, P::ThreadInteract).await);
    // Everyone of the root org.
    let mut conn = w.conn().await;
    give_role(
        &mut conn,
        d.sales_project,
        "members",
        w.root,
        "viewer",
        w.boss,
        None,
    )
    .await
    .unwrap();
    drop(conn);
    for member in [deep_member, ml_member, root_member, sales_member] {
        assert!(w.can(member, d.sales_project, P::ProjectView).await);
    }
    assert!(
        !w.can(nobody, d.sales_project, P::ProjectView).await,
        "no member of the tree"
    );
}

#[tokio::test]
async fn a_member_three_levels_down_is_a_member_of_every_org_above_and_sees_their_names_and_nothing_else()
 {
    let w = World::new().await;
    let d = w.deep().await;
    let member = w.user("member").await;
    let stranger = w.user("stranger").await;
    let mut conn = w.conn().await;
    insert_member(&mut conn, w.root, member, d.d3)
        .await
        .unwrap();
    drop(conn);
    let principal = w.principal(Caller::session(member)).await;
    let mut expected = vec![w.root, d.d1, d.d2, d.d3];
    expected.sort_unstable();
    assert_eq!(
        principal.member_of(),
        expected.as_slice(),
        "the domain and every org above it, and none beside"
    );
    let mut conn = w.conn().await;
    let ancestors = nodes::ancestors(&mut conn, &principal, d.thread, Policy::default())
        .await
        .unwrap();
    let named: Vec<(bool, bool)> = ancestors
        .iter()
        .map(|a| (a.name.is_some(), a.redacted))
        .collect();
    // root, d1, d2, d3 are orgs they belong to; the project, the topic hold nothing for them.
    assert_eq!(ancestors.len(), 6);
    assert_eq!(
        &named[..4],
        &[(true, false); 4],
        "the names of the orgs they are members of"
    );
    assert_eq!(
        &named[4..],
        &[(false, true); 2],
        "the project and the topic are redacted references"
    );
    let principal = w.principal(Caller::session(stranger)).await;
    let ancestors = nodes::ancestors(&mut conn, &principal, d.thread, Policy::default())
        .await
        .unwrap();
    assert!(ancestors.iter().all(|a| a.redacted && a.name.is_none()));
}

#[tokio::test]
async fn a_role_that_ended_or_was_taken_away_holds_nothing_and_ends_at_the_moment_it_says() {
    let w = World::new().await;
    let (lasting, ended, removed) = (
        w.user("lasting").await,
        w.user("ended").await,
        w.user("removed").await,
    );
    let mut conn = w.conn().await;
    give_role(
        &mut conn,
        w.project,
        "user",
        lasting,
        "developer",
        w.boss,
        Some(w.now.get() + Duration::hours(1)),
    )
    .await
    .unwrap();
    give_role(
        &mut conn,
        w.project,
        "user",
        ended,
        "developer",
        w.boss,
        Some(w.now.get() - Duration::seconds(1)),
    )
    .await
    .unwrap();
    let gone = give_role(
        &mut conn,
        w.project,
        "user",
        removed,
        "developer",
        w.boss,
        None,
    )
    .await
    .unwrap();
    take_role_away(&mut conn, gone, w.boss, w.now.get())
        .await
        .unwrap();
    drop(conn);
    assert!(w.can(lasting, w.thread, P::ThreadManage).await);
    assert!(!w.can(ended, w.thread, P::ThreadManage).await);
    assert!(!w.can(removed, w.thread, P::ThreadManage).await);
    // It ends from the moment of its end on.
    assert!(
        w.can_at(
            lasting,
            w.thread,
            P::ThreadManage,
            w.later(Duration::minutes(59))
        )
        .await
    );
    assert!(
        !w.can_at(
            lasting,
            w.thread,
            P::ThreadManage,
            w.later(Duration::hours(1))
        )
        .await
    );
}

#[tokio::test]
async fn the_owners_of_a_place_count_as_its_admins_while_they_are_members_of_its_root_org() {
    let w = World::new().await;
    let d = w.deep().await;
    let (owner, outsider_owner) = (w.member("owner").await, w.user("outsiderowner").await);
    let mut conn = w.conn().await;
    name_owner(&mut conn, d.d2, ("user", owner), w.now.get())
        .await
        .unwrap();
    name_owner(&mut conn, d.d2, ("user", outsider_owner), w.now.get())
        .await
        .unwrap();
    drop(conn);
    for permission in [
        P::ThreadManage,
        P::ThreadArchive,
        P::AccessManage,
        P::ProjectManage,
    ] {
        let node = if permission == P::ProjectManage {
            d.project
        } else {
            d.thread
        };
        assert_eq!(
            w.can(owner, node, permission).await,
            permission != P::AccessManage,
            "ordinary ownership still follows the shipped roles: {permission}"
        );
        assert!(
            w.can_as(
                Caller::session(owner).with_admin_mode(true),
                node,
                permission
            )
            .await,
            "an owner of an org above is an Admin there: {permission}"
        );
    }
    assert!(
        !w.can_as(
            Caller::session(owner).with_admin_mode(true),
            d.sales_project,
            P::ProjectManage
        )
        .await,
        "and only there"
    );
    assert!(
        !w.can_as(
            Caller::session(outsider_owner).with_admin_mode(true),
            d.thread,
            P::ThreadManage
        )
        .await,
        "owning counts while they are a member of the root org"
    );
}

#[tokio::test]
async fn the_first_owner_and_a_further_owner_of_a_thing_hold_all_of_its_page_but_naming_outsiders()
{
    let w = World::new().await;
    let further = w.member("further").await;
    let mut conn = w.conn().await;
    name_owner(&mut conn, w.thread, ("user", further), w.now.get())
        .await
        .unwrap();
    drop(conn);
    for owner in [w.maker, further] {
        for permission in support::permissions_of(orch_access_contract::roles::NodeKind::Thread) {
            let expected = permission != P::AccessExternal || false;
            let held = w.can(owner, w.thread, permission).await;
            // AccessExternal is held through its row, as shipped to Developers: owners alone do not.
            if permission == P::AccessExternal {
                assert!(!held);
            } else {
                assert_eq!(held, expected, "{permission}");
            }
        }
        assert!(
            !w.can(owner, w.topic, P::TopicManage).await,
            "and only on what they own"
        );
    }
}

#[tokio::test]
async fn a_system_admin_with_session_administration_on_is_an_admin_everywhere_and_a_member_of_none()
{
    let w = World::new().await;
    let d = w.deep().await;
    let root_admin = w.user("sysadmin").await;
    let mut conn = w.conn().await;
    orch_db::queries::access::testing::make_system_admin(&mut conn, root_admin)
        .await
        .unwrap();
    drop(conn);
    assert!(!w.can(root_admin, w.root, P::AccessManage).await);
    for node in [d.thread, d.sales_project, w.thread, d.d3, w.root] {
        assert!(
            w.can_as(
                Caller::session(root_admin).with_admin_mode(true),
                node,
                P::AccessManage
            )
            .await,
            "they administer every org, a root org too: {node}"
        );
    }
    assert!(
        w.can_as(
            Caller::session(root_admin).with_admin_mode(true),
            d.thread,
            P::ThreadManage
        )
        .await,
        "they hold what the row gives Developers"
    );
    let principal = w
        .principal(Caller::session(root_admin).with_admin_mode(true))
        .await;
    assert!(
        principal.member_of().is_empty(),
        "not in the member list, not matched by org_members"
    );
    let _ = PlaceRef::System;
}

#[tokio::test]
async fn the_roles_of_a_place_are_listed_with_those_held_above_it_and_what_ended_is_left_out() {
    let w = World::new().await;
    let d = w.deep().await;
    let (a, b, c, gone) = (
        w.user("aaa").await,
        w.user("bbb").await,
        w.user("ccc").await,
        w.user("ddd").await,
    );
    let in_project = w.give(d.project, a, Role::Developer).await;
    let in_org = w.give(d.d2, b, Role::Admin).await;
    let in_root = w.give(w.root, c, Role::Viewer).await;
    let mut conn = w.conn().await;
    give_role(
        &mut conn,
        d.project,
        "user",
        gone,
        "viewer",
        w.boss,
        Some(w.now.get() - Duration::hours(1)),
    )
    .await
    .unwrap();
    let here = orch_authz::holders::roles_here(&mut conn, d.project, w.now)
        .await
        .unwrap();
    assert_eq!(
        here.own.iter().map(|held| held.id).collect::<Vec<_>>(),
        [in_project]
    );
    assert_eq!(
        here.above.iter().map(|held| held.id).collect::<Vec<_>>(),
        [in_org, in_root],
        "the nearest place first"
    );
    assert_eq!(here.own[0].role, Role::Developer);
    assert_eq!(
        here.own[0].subject,
        orch_access_contract::roles::Subject::user(a)
    );
    // A thread has no roles of its own.
    let thread = orch_authz::holders::roles_here(&mut conn, d.thread, w.now)
        .await
        .unwrap();
    assert!(thread.own.is_empty());
    assert_eq!(thread.above.len(), 3);
}

#[tokio::test]
async fn everyone_who_holds_a_permission_is_found_behind_roles_groups_and_owners_but_not_the_suspended()
 {
    let w = World::new().await;
    let (dev, viewer, grouped, named, suspended) = (
        w.user("dev").await,
        w.user("viewer").await,
        w.member("grouped").await,
        w.user("named").await,
        w.user("suspended").await,
    );
    w.give(w.project, dev, Role::Developer).await;
    w.give(w.project, viewer, Role::Viewer).await;
    w.give(w.project, suspended, Role::Developer).await;
    let group = w.group(w.root, "Ops", &[grouped]).await;
    w.name(
        w.thread,
        P::ThreadManage,
        orch_access_contract::pages::Holder::Group { id: group },
    )
    .await;
    w.name(
        w.thread,
        P::ThreadManage,
        orch_access_contract::pages::Holder::User { id: named },
    )
    .await;
    let mut conn = w.conn().await;
    suspend_in(&mut conn, w.root, suspended, w.boss)
        .await
        .unwrap();
    drop(conn);
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut conn = w.conn().await;
    let (frame, sites) = support::frame_for(&mut conn, &principal, &[w.thread]).await;
    let site = &sites[0];
    let mut managers = orch_authz::holders::who_holds(&mut conn, &frame, site, P::ThreadManage)
        .await
        .unwrap();
    managers.sort_unstable();
    let mut expected = vec![w.maker, w.boss, dev, grouped, named];
    expected.sort_unstable();
    assert_eq!(
        managers, expected,
        "the owners (the boss owns the topic above), Developers, the group and the person"
    );
    let viewers = orch_authz::holders::who_holds(&mut conn, &frame, site, P::ThreadView)
        .await
        .unwrap();
    assert!(
        viewers.contains(&viewer) && viewers.contains(&dev),
        "a level holds those below it"
    );
    assert!(
        !viewers.contains(&suspended),
        "somebody suspended where the thread is holds nothing"
    );
}
