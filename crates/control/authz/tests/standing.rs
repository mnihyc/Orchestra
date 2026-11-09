//! Archive, suspension, system admins, tokens, levels, and the refusals that say everything that is lacking.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::decisions::{Closed, Decision};
use orch_access_contract::pages::Holder;
use orch_access_contract::permissions::Permission as P;
use orch_access_contract::roles::{PlaceRef, Role};
use orch_authz::{Act, Caller, Error, Policy, TokenGrant, nodes};
use orch_db::queries::access::testing::{
    give_role, lift_suspension, make_system_admin, suspend_in,
};
use orch_db::queries::hierarchy::archive;
use orch_db::queries::identity::testing::suspend;
use serde_json::json;
use support::{World, role_holder};

async fn archive_node(w: &World, node: orch_types::Id) {
    let mut conn = w.conn().await;
    assert!(archive(&mut conn, node, w.boss, w.now.get()).await.unwrap());
}

async fn check(
    w: &World,
    caller: Caller,
    node: orch_types::Id,
    permission: P,
    act: Act,
) -> Result<Decision, Error> {
    let principal = w.principal(caller).await;
    let mut conn = w.conn().await;
    nodes::check(
        &mut conn,
        &principal,
        node,
        permission,
        act,
        Policy::default(),
    )
    .await
}

/// The refusal of `thread.interact` on the thread for `who`.
async fn refusal_of(w: &World, who: orch_types::Id) -> orch_access_contract::decisions::Refusal {
    let Decision::Refused(refusal) = check(
        w,
        Caller::session(who),
        w.thread,
        P::ThreadInteract,
        Act::Use,
    )
    .await
    .unwrap() else {
        panic!("refused")
    };
    refusal
}

#[tokio::test]
async fn an_archived_node_keeps_looking_and_saying_who_can_and_what_deletes_or_brings_it_back() {
    let w = World::new().await;
    let dev = w.user("dev").await;
    w.give(w.project, dev, Role::Developer).await;
    assert!(w.can(dev, w.thread, P::ThreadInteract).await);
    archive_node(&w, w.thread).await;
    let held = w.holds(dev, w.thread).await;
    for kept in [
        P::ThreadView,
        P::ChatView,
        P::DocView,
        P::ThreadManage,
        P::ThreadArchive,
    ] {
        assert!(held.contains(&kept), "still held under archive: {kept}");
    }
    for gone in [
        P::ThreadInteract,
        P::ChatInteract,
        P::DocEdit,
        P::DocManage,
        P::ThreadFork,
    ] {
        assert!(!held.contains(&gone), "read-only under archive: {gone}");
    }
    // A mutation is refused as one under an archived node, once it is known that they hold it.
    let refused = check(
        &w,
        Caller::session(dev),
        w.thread,
        P::ThreadInteract,
        Act::Use,
    )
    .await;
    assert!(
        matches!(refused, Err(Error::Archived { node }) if node == w.thread),
        "{refused:?}"
    );
    assert!(
        check(&w, Caller::session(dev), w.thread, P::ThreadView, Act::Use)
            .await
            .unwrap()
            .is_allowed()
    );
    assert!(
        check(
            &w,
            Caller::session(dev),
            w.thread,
            P::ThreadManage,
            Act::Delete
        )
        .await
        .unwrap()
        .is_allowed()
    );
    assert!(
        check(
            &w,
            Caller::session(dev),
            w.thread,
            P::ThreadArchive,
            Act::Unarchive
        )
        .await
        .unwrap()
        .is_allowed()
    );
    assert!(
        matches!(
            check(
                &w,
                Caller::session(dev),
                w.thread,
                P::ThreadManage,
                Act::Use
            )
            .await,
            Err(Error::Archived { .. })
        ),
        "managing is not deleting"
    );
    // Somebody who would be refused anyway learns nothing of the archive.
    let nobody = w.user("nobody").await;
    let refused = check(
        &w,
        Caller::session(nobody),
        w.thread,
        P::ThreadInteract,
        Act::Use,
    )
    .await
    .unwrap();
    assert!(!refused.is_allowed());
    // Saying who can stays.
    assert!(w.can(w.maker, w.thread, P::AccessManage).await);
}

#[tokio::test]
async fn a_node_below_an_archived_one_is_archived_with_it_and_unarchiving_clears_only_its_own() {
    let w = World::new().await;
    archive_node(&w, w.project).await;
    let held = w.holds(w.maker, w.thread).await;
    assert!(held.contains(&P::ThreadView) && !held.contains(&P::ThreadInteract));
    let blocked = check(
        &w,
        Caller::session(w.maker),
        w.thread,
        P::ThreadInteract,
        Act::Use,
    )
    .await;
    assert!(
        matches!(blocked, Err(Error::Archived { node }) if node == w.project),
        "the nearest archived node is named: {blocked:?}"
    );
    archive_node(&w, w.topic).await;
    let nearer = check(
        &w,
        Caller::session(w.maker),
        w.thread,
        P::ThreadInteract,
        Act::Use,
    )
    .await;
    assert!(matches!(nearer, Err(Error::Archived { node }) if node == w.topic));
    let mut conn = w.conn().await;
    orch_db::queries::hierarchy::unarchive(&mut conn, w.topic, w.now.get())
        .await
        .unwrap();
    drop(conn);
    let still = check(
        &w,
        Caller::session(w.maker),
        w.thread,
        P::ThreadInteract,
        Act::Use,
    )
    .await;
    assert!(
        matches!(still, Err(Error::Archived { node }) if node == w.project),
        "the project's own archive is untouched"
    );
}

#[tokio::test]
async fn a_thread_and_a_topic_are_archived_and_brought_back_by_a_developer_and_a_project_by_an_admin()
 {
    let w = World::new().await;
    let (dev, admin) = (w.user("dev").await, w.user("admin").await);
    w.give(w.project, dev, Role::Developer).await;
    w.give(w.root, admin, Role::Admin).await;
    assert!(
        w.can(dev, w.thread, P::ThreadArchive).await,
        "whoever manages a thread archives it"
    );
    assert!(w.can(dev, w.topic, P::TopicArchive).await);
    assert!(
        !w.can(dev, w.project, P::ProjectArchive).await,
        "refused to a Developer as shipped"
    );
    assert!(
        w.can(admin, w.project, P::ProjectArchive).await,
        "an admin of the org it lies in"
    );
    assert!(
        w.can(w.boss, w.project, P::ProjectArchive).await,
        "and one of its owners"
    );
    // Once the row names Developers they may.
    w.name(
        w.project,
        P::ProjectArchive,
        Holder::Role {
            role: Role::Developer,
            at: None,
        },
    )
    .await;
    assert!(w.can(dev, w.project, P::ProjectArchive).await);
    // The row cut loose names nobody; managing still archives.
    w.row(w.project, P::ProjectArchive, false).await;
    assert!(
        w.can(admin, w.project, P::ProjectArchive).await,
        "through project.manage"
    );
    assert!(
        w.can(dev, w.project, P::ProjectArchive).await,
        "its entry is still on the row"
    );
}

#[tokio::test]
async fn nothing_is_held_where_a_person_is_suspended_and_everything_is_held_elsewhere() {
    let w = World::new().await;
    let d = w.deep().await;
    let person = w.member("person").await;
    w.give(w.root, person, Role::Developer).await;
    w.give(d.d2, person, Role::Admin).await;
    let held_before = w.holds(person, d.thread).await;
    assert!(held_before.contains(&P::ThreadManage));
    // Suspended in a department above the thread: the thread is closed to them.
    let mut conn = w.conn().await;
    suspend_in(&mut conn, d.d1, person, w.boss).await.unwrap();
    drop(conn);
    assert!(w.holds(person, d.thread).await.is_empty());
    let refused = check(
        &w,
        Caller::session(person),
        d.thread,
        P::ThreadView,
        Act::Use,
    )
    .await;
    assert!(
        matches!(refused, Err(Error::Suspended { org: Some(org) }) if org == d.d1),
        "{refused:?}"
    );
    // Everything outside that org, and above it, is held: the root's own project, a sibling department.
    assert!(
        w.can(person, w.thread, P::ThreadView).await,
        "a thread of the root org's project"
    );
    assert!(
        w.can(person, d.sales_project, P::ProjectView).await,
        "a sibling department's project"
    );
    // Suspended in a sibling org or in another root org: no effect here.
    let mut conn = w.conn().await;
    lift_suspension(&mut conn, d.d1, person).await.unwrap();
    suspend_in(&mut conn, d.sales, person, w.boss)
        .await
        .unwrap();
    drop(conn);
    assert_eq!(w.holds(person, d.thread).await, held_before);
    // Altogether: nothing anywhere.
    let mut conn = w.conn().await;
    suspend(&mut conn, person).await.unwrap();
    drop(conn);
    for node in [d.thread, w.thread, d.sales_project, w.root] {
        assert!(w.holds(person, node).await.is_empty(), "{node}");
    }
    let refused = check(
        &w,
        Caller::session(person),
        w.thread,
        P::ThreadView,
        Act::Use,
    )
    .await;
    assert!(matches!(refused, Err(Error::Suspended { org: None })));
}

#[tokio::test]
async fn what_a_suspended_person_owns_stays_theirs_and_serves_others_and_the_orgs_admins_administer_it()
 {
    let w = World::new().await;
    let (viewer, admin) = (w.member("viewer").await, w.member("admin").await);
    w.give(w.root, viewer, Role::Viewer).await;
    w.give(w.root, admin, Role::Admin).await;
    let mut conn = w.conn().await;
    suspend_in(&mut conn, w.root, w.maker, w.boss)
        .await
        .unwrap();
    drop(conn);
    assert!(
        w.holds(w.maker, w.thread).await.is_empty(),
        "they hold nothing there while suspended"
    );
    assert!(
        w.can(viewer, w.thread, P::ThreadView).await,
        "it still serves the people its page names"
    );
    assert!(
        w.can_as(
            Caller::session(admin).with_admin_mode(true),
            w.thread,
            P::AccessManage
        )
        .await,
        "and the org's admins administer it"
    );
    let principal = w
        .principal(Caller::session(admin).with_admin_mode(true))
        .await;
    let mut conn = w.conn().await;
    let first = orch_authz::pages::read(
        &mut conn,
        &principal,
        orch_authz::pages::Target::Live(w.thread),
        Policy::default(),
    )
    .await
    .unwrap();
    drop(conn);
    assert_eq!(first.first_owner, Some(w.maker), "it is still theirs");
    let mut conn = w.conn().await;
    lift_suspension(&mut conn, w.root, w.maker).await.unwrap();
    drop(conn);
    assert!(
        w.can(w.maker, w.thread, P::ThreadManage).await,
        "lifted, everything is back: nothing was removed"
    );
}

#[tokio::test]
async fn a_system_admin_opts_this_session_in_and_never_gives_native_reach_to_a_token() {
    let w = World::new().await;
    let d = w.deep().await;
    let sysadmin = w.user("sysadmin").await;
    let mut conn = w.conn().await;
    make_system_admin(&mut conn, sysadmin).await.unwrap();
    drop(conn);
    for node in [d.thread, d.sales_project, w.root] {
        assert!(
            !w.can(sysadmin, node, P::AccessManage).await,
            "new sessions start off"
        );
        assert!(
            w.can_as(
                Caller::session(sysadmin).with_admin_mode(true),
                node,
                P::AccessManage
            )
            .await,
            "this session explicitly acts as admin"
        );
    }
    assert!(
        w.can_as(
            Caller::session(sysadmin).with_admin_mode(true),
            d.thread,
            P::ThreadManage
        )
        .await,
        "native management, without a self-naming override"
    );
    let token = Caller::token(sysadmin, TokenGrant::everything());
    for node in [d.thread, d.sales_project, w.root] {
        assert!(
            !w.can_as(token.clone(), node, P::AccessManage).await,
            "with a token: none of it"
        );
        assert!(!w.can_as(token.clone(), node, P::OrgView).await || node == w.root && false);
    }
    // Their own roles and entries still count with a token.
    w.give(d.project, sysadmin, Role::Developer).await;
    assert!(w.can_as(token.clone(), d.thread, P::ThreadManage).await);
    assert!(!w.can_as(token, d.thread, P::AccessManage).await);
}

#[tokio::test]
async fn a_token_carries_its_scopes_within_its_subtree_and_never_more_than_its_user_holds() {
    let w = World::new().await;
    let d = w.deep().await;
    let dev = w.user("dev").await;
    w.give(w.root, dev, Role::Developer).await;
    let session = Caller::session(dev);
    let all = Caller::token(dev, TokenGrant::everything());
    let scoped = Caller::token(
        dev,
        TokenGrant::from_stored(&["thread.interact".to_owned()], None).unwrap(),
    );
    let kept = Caller::token(
        dev,
        TokenGrant::new(
            orch_access_contract::decisions::Scopes::All,
            Some(d.project),
        ),
    );
    for (what, caller, node, permission, expected) in [
        (
            "a session",
            session.clone(),
            d.thread,
            P::ThreadManage,
            true,
        ),
        (
            "a token with every scope",
            all.clone(),
            d.thread,
            P::ThreadManage,
            true,
        ),
        (
            "a scope that is named",
            scoped.clone(),
            d.thread,
            P::ThreadInteract,
            true,
        ),
        (
            "a scope that is not named",
            scoped.clone(),
            d.thread,
            P::ThreadManage,
            false,
        ),
        (
            "a level is not its lower levels",
            scoped.clone(),
            d.thread,
            P::ThreadView,
            false,
        ),
        (
            "a token in its subtree",
            kept.clone(),
            d.thread,
            P::ThreadManage,
            true,
        ),
        (
            "a token outside its subtree",
            kept.clone(),
            w.thread,
            P::ThreadManage,
            false,
        ),
        (
            "a token above its subtree",
            kept.clone(),
            w.root,
            P::OrgView,
            false,
        ),
        (
            "what the user lacks, with every scope",
            all.clone(),
            d.thread,
            P::AccessManage,
            false,
        ),
    ] {
        assert_eq!(w.can_as(caller, node, permission).await, expected, "{what}");
    }
    // A token outside its subtree is refused with nobody to ask.
    let Decision::Refused(refusal) = check(&w, kept, w.thread, P::ThreadView, Act::Use)
        .await
        .unwrap()
    else {
        panic!("refused")
    };
    assert!(!refusal.requestable && !refusal.r#override && refusal.missing.is_empty());
}

#[tokio::test]
async fn a_service_account_acts_by_the_roles_it_was_given_and_is_a_member_of_nothing() {
    let w = World::new().await;
    let account = w.user("service").await;
    sqlx::query("UPDATE users SET kind = 'service', service_org_id = $2 WHERE id = $1")
        .bind(account)
        .bind(w.root)
        .execute(&w.db.pool)
        .await
        .unwrap();
    assert!(
        !w.can(account, w.thread, P::ThreadView).await,
        "a member of nothing until it is given a role"
    );
    w.give(w.project, account, Role::Interactor).await;
    let token = Caller::token(account, TokenGrant::everything());
    assert!(w.can_as(token.clone(), w.thread, P::ThreadInteract).await);
    assert!(!w.can_as(token, w.thread, P::ThreadManage).await);
    assert!(
        w.principal(Caller::session(account))
            .await
            .member_of()
            .is_empty()
    );
}

#[tokio::test]
async fn manage_of_an_area_holds_the_levels_below_it_and_its_advanced_rows_and_nothing_across_areas_but_the_environments_five_doors()
 {
    let w = World::new().await;
    let (thread_manager, files_manager, environment_manager) = (
        w.member("threadmgr").await,
        w.member("filesmgr").await,
        w.member("envmgr").await,
    );
    let environment = w.thing("environment", w.boss, w.topic, "Main").await;
    w.put_in(w.thread, environment).await;
    w.row(w.thread, P::ThreadView, false).await;
    w.row(w.thread, P::ThreadInteract, false).await;
    w.row(w.thread, P::ThreadManage, false).await;
    for permission in [
        P::EnvironmentView,
        P::EnvironmentManage,
        P::FilesView,
        P::FilesManage,
        P::TerminalView,
        P::TerminalManage,
        P::BrowserView,
        P::BrowserManage,
        P::VncView,
        P::VncManage,
    ] {
        w.thing_row("environment", environment, permission, false)
            .await;
    }
    w.name(
        w.thread,
        P::ThreadManage,
        Holder::User { id: thread_manager },
    )
    .await;
    w.grant(
        "environment",
        environment,
        P::FilesManage,
        Holder::User { id: files_manager },
    )
    .await;
    w.grant(
        "environment",
        environment,
        P::EnvironmentManage,
        Holder::User {
            id: environment_manager,
        },
    )
    .await;
    for permission in [
        P::ThreadView,
        P::ThreadInteract,
        P::ThreadManage,
        P::ThreadFork,
        P::ThreadArchive,
    ] {
        assert!(
            w.can(thread_manager, w.thread, permission).await,
            "{permission}"
        );
    }
    for permission in [P::ChatView, P::DocView] {
        assert!(
            !w.can(thread_manager, w.thread, permission).await,
            "thread.manage does not open {permission}"
        );
    }
    assert!(
        w.holds_on(thread_manager, "environment", environment)
            .await
            .is_empty(),
        "managing a thread opens nothing of its environment: no file, no shell, no browser"
    );
    // The Manage of one door holds that door's View, and nothing else of the environment.
    assert_eq!(
        w.holds_on(files_manager, "environment", environment).await,
        [P::FilesView, P::FilesManage]
    );
    assert!(
        !w.can(files_manager, w.thread, P::ThreadInteract).await,
        "files.manage does not send a message"
    );
    assert!(!w.can(files_manager, w.thread, P::ThreadView).await);
    // Managing the environment is the one thing that crosses: it holds the Manage of all five doors,
    // with nobody named in them, and gives no word in any thread that runs there.
    let held = w
        .holds_on(environment_manager, "environment", environment)
        .await;
    assert_eq!(
        held,
        [
            P::EnvironmentView,
            P::EnvironmentUse,
            P::EnvironmentManage,
            P::FilesView,
            P::FilesManage,
            P::TerminalView,
            P::TerminalManage,
            P::BrowserView,
            P::BrowserManage,
            P::VncView,
            P::VncManage,
            P::PortsView,
            P::PortsManage,
        ]
    );
    for door in [
        P::FilesManage,
        P::TerminalManage,
        P::BrowserManage,
        P::VncManage,
        P::PortsManage,
    ] {
        assert!(held.contains(&door), "{door}");
    }
    assert!(!w.can(environment_manager, w.thread, P::ThreadView).await);
    assert!(
        !w.can(environment_manager, w.thread, P::ThreadInteract)
            .await
    );
}

#[tokio::test]
async fn a_refusal_names_what_is_lacking_who_can_give_it_and_whether_the_caller_may_force_it() {
    let w = World::new().await;
    let (viewer, stranger, admin, dev) = (
        w.user("viewer").await,
        w.user("stranger").await,
        w.member("admin").await,
        w.user("dev").await,
    );
    w.give(w.project, viewer, Role::Viewer).await;
    w.give(w.root, admin, Role::Admin).await;
    w.give(w.project, dev, Role::Developer).await;
    let refused = refusal_of(&w, viewer).await;
    assert_eq!(refused.required, P::ThreadInteract);
    assert_eq!(refused.object.id, w.thread);
    assert!(refused.requestable && !refused.r#override);
    let missing = &refused.missing[0];
    assert_eq!(missing.permissions, vec![P::ThreadInteract]);
    assert_eq!(
        missing.object.name.as_deref(),
        Some("Fix"),
        "they hold something there, which shows the name"
    );
    assert_eq!(missing.object.owner, Some(w.maker));
    assert!(missing.ask.contains(&w.maker), "the owner can give it");
    assert!(
        missing.ask.contains(&w.boss),
        "and so can the admins above: the owner of the place counts"
    );
    assert!(missing.closed.is_none() && !missing.r#override);
    // Somebody who holds nothing learns neither the name nor the owner.
    let blind = refusal_of(&w, stranger).await;
    assert_eq!(blind.missing[0].object.name, None);
    assert_eq!(blind.missing[0].object.owner, None);
    // An admin of the place holds what the page gives Admins, and so is not refused here.
    assert!(w.can(admin, w.thread, P::ThreadInteract).await);
    // A private page is still closed in ordinary mode. Enabling this admin's
    // native reach opens and manages it without changing the page.
    w.change(
        w.maker,
        orch_authz::pages::Target::Live(w.thread),
        json!({ "whole": "private" }),
    )
    .await
    .unwrap();
    let ordinary = refusal_of(&w, admin).await;
    assert!(
        !ordinary.r#override,
        "ordinary mode never offers native authority"
    );
    for permission in [P::ThreadView, P::ThreadInteract, P::ThreadManage] {
        assert!(
            w.can_as(
                Caller::session(admin).with_admin_mode(true),
                w.thread,
                permission
            )
            .await
        );
    }
    let refused = refusal_of(&w, dev).await;
    assert!(!refused.r#override, "a Developer is no admin of the place");
}

#[tokio::test]
async fn nobody_can_give_what_a_lock_above_decides_and_the_refusal_names_the_place_that_locked_it()
{
    let w = World::new().await;
    let viewer = w.user("viewer").await;
    w.give(w.project, viewer, Role::Viewer).await;
    w.change_defaults(
        w.boss,
        w.root,
        json!({ "rows": [{ "permission": "thread.interact", "profile": "managed", "locked": true, "add": [{ "holder": role_holder(Role::Admin) }] }] }),
    )
    .await
    .unwrap();
    let Decision::Refused(refusal) = check(
        &w,
        Caller::session(viewer),
        w.thread,
        P::ThreadInteract,
        Act::Use,
    )
    .await
    .unwrap() else {
        panic!("refused")
    };
    let missing = &refusal.missing[0];
    assert_eq!(missing.closed, Some(Closed::Locked));
    assert_eq!(missing.locked_by, Some(PlaceRef::Node(w.root)));
    assert!(missing.ask.is_empty() && !refusal.requestable && !refusal.r#override);
}

#[tokio::test]
async fn an_admin_of_one_department_has_no_say_over_a_thread_of_its_sibling() {
    let w = World::new().await;
    let d = w.deep().await;
    let sales_thread = w
        .node(
            "thread",
            w.node("topic", d.sales_project, "Calls").await,
            "Call",
        )
        .await;
    let admin = w.user("admin").await;
    w.give(d.d1, admin, Role::Admin).await;
    let caller = Caller::session(admin).with_admin_mode(true);
    assert!(w.can_as(caller.clone(), d.thread, P::AccessManage).await);
    assert!(
        !w.can_as(caller.clone(), sales_thread, P::AccessManage)
            .await
    );
    assert!(!w.can_as(caller, sales_thread, P::ThreadView).await);
    let _ = give_role;
}

#[tokio::test]
async fn a_suspension_closes_the_org_the_thing_lies_in_and_touches_no_other_root_org() {
    let w = World::new().await;
    let d = w.deep().await;
    let person = w.member("person").await;
    w.give(d.project, person, Role::Developer).await;
    w.give(w.project, person, Role::Developer).await;
    let globex = {
        let mut conn = w.conn().await;
        let id = orch_types::new_id();
        orch_db::queries::hierarchy::testing::insert_node(
            &mut conn, id, "org", None, "Globex", w.boss,
        )
        .await
        .unwrap();
        id
    };
    // In another root org: no effect here.
    let mut conn = w.conn().await;
    suspend_in(&mut conn, globex, person, w.boss).await.unwrap();
    drop(conn);
    assert!(w.can(person, d.thread, P::ThreadManage).await);
    // In the org the project lies in: closed, and named.
    let mut conn = w.conn().await;
    suspend_in(&mut conn, d.d3, person, w.boss).await.unwrap();
    drop(conn);
    let refused = check(
        &w,
        Caller::session(person),
        d.thread,
        P::ThreadManage,
        Act::Use,
    )
    .await;
    assert!(
        matches!(refused, Err(Error::Suspended { org: Some(org) }) if org == d.d3),
        "{refused:?}"
    );
    assert!(
        w.can(person, w.thread, P::ThreadManage).await,
        "the root org's own project is open"
    );
    // A change of a page is refused them too, and so is the system's defaults page when it is altogether.
    let refused = w
        .change(
            person,
            orch_authz::pages::Target::Live(d.thread),
            json!({ "whole": "reset" }),
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(orch_authz::pages::ChangeError::Engine(
                Error::Suspended { .. }
            ))
        ),
        "{refused:?}"
    );
    let sysadmin = w.user("sysadmin").await;
    let mut conn = w.conn().await;
    make_system_admin(&mut conn, sysadmin).await.unwrap();
    suspend(&mut conn, sysadmin).await.unwrap();
    drop(conn);
    let system = orch_authz::pages::Target::Defaults(PlaceRef::System);
    let refused = w
        .change(sysadmin, system, json!({ "whole": "reset" }))
        .await;
    assert!(
        matches!(
            refused,
            Err(orch_authz::pages::ChangeError::Engine(Error::Suspended {
                org: None
            }))
        ),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_token_that_cannot_change_a_page_is_refused_with_nobody_to_ask() {
    let w = World::new().await;
    let scoped = Caller::token(
        w.maker,
        TokenGrant::from_stored(&["thread.view".to_owned()], None).unwrap(),
    );
    let refused = w
        .change_as(
            scoped,
            orch_authz::pages::Target::Live(w.thread),
            json!({ "whole": "private" }),
            Policy::default(),
        )
        .await;
    let Err(orch_authz::pages::ChangeError::Denied(refusal)) = refused else {
        panic!("{refused:?}")
    };
    assert!(refusal.missing.is_empty() && !refusal.requestable && !refusal.r#override);
    // The same user in a session changes it.
    w.change(
        w.maker,
        orch_authz::pages::Target::Live(w.thread),
        json!({ "whole": "private" }),
    )
    .await
    .unwrap();
}
