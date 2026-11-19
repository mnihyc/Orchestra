//! Prospective bounds match real creation pages and the ordinary Share writer.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
mod support;

use orch_access_contract::pages::{Holder, PageChange, StoredKind, Whole};
use orch_access_contract::permissions::{NodePermission, Permission as P, Profile};
use orch_access_contract::roles::PlaceRef;
use orch_authz::pages::{self, Target};
use orch_authz::{Act, Caller, Policy, Principal, TokenGrant, creation, ownership};
use orch_db::queries::{access, hierarchy, threads};
use orch_types::{Id, new_id};
use serde_json::json;
use support::World;

fn policy() -> Policy {
    Policy {
        holders_share: true,
        owners_name_outsiders: true,
    }
}

async fn creator(w: &World, member: bool) -> Id {
    let user = if member {
        w.member("creator").await
    } else {
        w.user("outsidecreator").await
    };
    w.change_as(Caller::session(w.boss), Target::Live(w.project), json!({"rows":[{"permission":"project.create_topics","add":[{"holder":{"kind":"user","id":user}}]},{"permission":"project.view","add":[{"holder":{"kind":"user","id":user}}]}]}), policy()).await.unwrap();
    assert!(w.can(user, w.project, P::ProjectCreateTopics).await);
    assert!(
        w.can(user, w.project, P::ProjectView).await,
        "the scoped viewers audience has an independent View premise"
    );
    assert!(!w.can(user, w.project, P::DocManage).await);
    user
}

async fn node(conn: &mut orch_db::Conn, principal: &Principal, parent: Id, thread: bool) -> Id {
    let first = ownership::owner_of_new(conn, principal.user(), Some(parent))
        .await
        .unwrap();
    let id = new_id();
    if thread {
        assert!(
            threads::insert_thread(
                conn,
                id,
                parent,
                "Actual created thread",
                principal.user(),
                principal.now().get()
            )
            .await
            .unwrap()
        );
    } else {
        hierarchy::create(
            conn,
            &hierarchy::NewNode {
                id,
                kind: "topic",
                parent: Some(parent),
                name: "Actual created topic",
                name_by_person: true,
                description: "",
                description_by_person: false,
                owner: first,
                org: None,
                at: principal.now().get(),
            },
        )
        .await
        .unwrap();
    }
    pages::start(
        conn,
        StoredKind::Node,
        id,
        principal.user(),
        principal.now(),
    )
    .await
    .unwrap();
    if thread {
        threads::make_node_chat(conn, id, principal.user(), principal.now().get())
            .await
            .unwrap();
    }
    id
}

async fn can_gift(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    subject: Id,
    recipient: Id,
    inside: bool,
    wanted: &[P],
) -> bool {
    let ordinary = principal.ordinary(conn).await.unwrap();
    let mut trial = access::begin_trial(conn).await.unwrap();
    let graph = orch_authz::engine::Graph::load(&mut trial, &[subject])
        .await
        .unwrap();
    let site = graph.site_of(subject).unwrap();
    let view = NodePermission::View.on(site.kind.node().unwrap()).unwrap();
    let sees_source =
        orch_authz::nodes::check(&mut trial, &ordinary, subject, view, Act::Use, policy())
            .await
            .unwrap()
            .is_allowed();
    let sees_recipient = orch_authz::nodes::check(
        &mut trial,
        principal,
        recipient,
        P::ThreadView,
        Act::Use,
        policy(),
    )
    .await
    .unwrap()
    .is_allowed();
    if !sees_source || !sees_recipient {
        access::end_trial(trial).await.unwrap();
        return false;
    }
    // These are the actual rows the ordinary Share producer writes, including its profile selection.
    let mut allowed = true;
    for defaults in [false, true] {
        if defaults && !inside {
            continue;
        }
        let rows: Vec<_> = wanted
            .iter()
            .copied()
            .filter(|permission| {
                defaults || !matches!(permission, P::ThreadView | P::ThreadInteract)
            })
            .flat_map(|permission| {
                if defaults && permission.profiles() {
                    vec![
                        (permission, Profile::Managed),
                        (permission, Profile::Connected),
                    ]
                } else {
                    vec![(permission, Profile::Any)]
                }
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        let target = if defaults {
            Target::Defaults(PlaceRef::Node(subject))
        } else {
            Target::Live(subject)
        };
        let rows = match pages::judge_share(&mut trial, &ordinary, target, &rows, policy())
            .await
            .unwrap()
        {
            pages::ShareJudgement::Allowed(rows) => rows,
            pages::ShareJudgement::Refused(_) => {
                allowed = false;
                break;
            }
        };
        let change: PageChange = serde_json::from_value(json!({"rows":rows.iter().map(|(permission, profile)|json!({"permission":permission.as_str(), "profile":if *profile == Profile::Any { None } else { Some(profile.as_str()) }, "add":[{"holder":{"kind":"agents","id":recipient}}]})).collect::<Vec<_>>()})).unwrap();
        if pages::change_by_share(
            &mut trial,
            &ordinary,
            target,
            &change,
            policy(),
            inside.then_some(true),
            &Default::default(),
        )
        .await
        .is_err()
        {
            allowed = false;
            break;
        }
    }
    access::end_trial(trial).await.unwrap();
    allowed
}

async fn compare(w: &World, caller: Caller, context: Option<&PageChange>) -> [Vec<P>; 3] {
    compare_placed(
        w,
        caller,
        context,
        orch_access_contract::permissions::Placement::Managed,
        None,
    )
    .await
}

async fn compare_placed(
    w: &World,
    caller: Caller,
    context: Option<&PageChange>,
    placement: orch_access_contract::permissions::Placement,
    environment: Option<Id>,
) -> [Vec<P>; 3] {
    let mut conn = w.conn().await;
    let principal = Principal::load(&mut conn, caller.clone(), w.now)
        .await
        .unwrap();
    let before: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM nodes),(SELECT count(*) FROM permission_entries),(SELECT count(*) FROM audit_log)").fetch_one(&mut *conn).await.unwrap();
    let predicted = creation::agent_permissions(
        &mut conn,
        &principal,
        w.project,
        None,
        context,
        placement,
        environment,
    )
    .await
    .unwrap();
    let after: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM nodes),(SELECT count(*) FROM permission_entries),(SELECT count(*) FROM audit_log)").fetch_one(&mut *conn).await.unwrap();
    assert_eq!(
        after, before,
        "a prospective frame persists no node, ACL entry or audit"
    );
    let topic = node(&mut conn, &principal, w.project, false).await;
    // The real first-thread producer reloads the same caller after its new topic exists.
    let principal = Principal::load(&mut conn, caller, w.now).await.unwrap();
    let thread = node(&mut conn, &principal, topic, true).await;
    if let Some(environment) = environment {
        threads::set_environment(&mut conn, thread, environment, "", 0)
            .await
            .unwrap();
    }
    let expected_owner = if principal.is_in_tree(w.root) {
        principal.user()
    } else {
        w.boss
    };
    for (id, manage) in [(topic, P::TopicManage), (thread, P::ThreadManage)] {
        let owner: Id = sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id=$1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(owner, expected_owner);
        let entries = access::entries_of(&mut conn, &access::PageKey::new("node", id, "live"))
            .await
            .unwrap();
        assert!(entries.iter().any(|entry| entry.permission == "access.own"
            && entry.holder_id == Some(expected_owner)
            && entry.created_by == Some(principal.user())
            && entry.source == "creation"));
        if expected_owner != principal.user() {
            assert!(!entries.iter().any(|entry| entry.permission == "access.own"
                && entry.holder_id == Some(principal.user())));
            assert!(
                entries
                    .iter()
                    .any(|entry| entry.permission == manage.as_str()
                        && entry.holder_id == Some(principal.user())
                        && entry.source == "creation")
            );
        }
    }
    if let Some(change) = context {
        pages::change(
            &mut conn,
            &principal,
            Target::Live(thread),
            change,
            policy(),
            &Default::default(),
        )
        .await
        .unwrap();
    }
    for (index, subject, inside) in [(0, thread, false), (1, topic, true), (2, w.project, true)] {
        for group in [
            &[P::DocView, P::DocEdit, P::DocManage][..],
            &[P::ChatView, P::ChatInteract][..],
            &[P::ThreadView, P::ThreadInteract][..],
        ] {
            if index == 0 && group[0] == P::ThreadView {
                continue;
            }
            for length in 1..=group.len() {
                let written = can_gift(
                    &mut conn,
                    &principal,
                    subject,
                    thread,
                    inside,
                    &group[..length],
                )
                .await;
                assert_eq!(
                    predicted[index].contains(&group[length - 1]),
                    written,
                    "source {index}, {:?}, actor {}",
                    &group[..length],
                    principal.user()
                );
            }
        }
    }
    predicted
}

#[tokio::test]
async fn a_member_creators_new_topic_bounds_do_not_borrow_the_existing_projects_giveability() {
    let w = World::new().await;
    let user = creator(&w, true).await;
    let predicted = compare(&w, Caller::session(user), None).await;
    assert!(predicted[0].contains(&P::DocManage));
    assert!(predicted[1].contains(&P::DocManage));
    assert!(!predicted[2].contains(&P::DocManage));
}

#[tokio::test]
async fn an_outside_creators_conditional_manage_is_not_ownership_and_private_custom_pages_can_reduce_area_bounds()
 {
    let w = World::new().await;
    let user = creator(&w, false).await;
    // A genuine scoped viewers audience at the real project includes this page-only viewer.
    // It supplies readable material and page management, without ownership or membership.
    for permission in [P::DocView, P::ChatView, P::AccessManage] {
        w.name_default(
            Some(w.project),
            permission,
            "any",
            Holder::Role {
                role: orch_access_contract::roles::Role::Viewer,
                at: Some(w.project),
            },
        )
        .await;
    }
    let before = compare(&w, Caller::session(user), None).await;
    assert!(!before[0].contains(&P::DocManage));
    assert!(!before[1].contains(&P::DocManage));
    assert!(before[0].contains(&P::DocView));
    let private = PageChange {
        whole: Some(Whole::Private),
        ..PageChange::default()
    };
    let after_private = compare(&w, Caller::session(user), Some(&private)).await;
    assert!(!after_private[0].contains(&P::DocView));
    let custom: PageChange = serde_json::from_value(json!({"rows":[{"permission":"doc.view","inherit":false},{"permission":"doc.edit","inherit":false},{"permission":"doc.manage","inherit":false}]})).unwrap();
    let after_custom = compare(&w, Caller::session(user), Some(&custom)).await;
    assert!(!after_custom[0].contains(&P::DocView));
    assert!(
        after_custom[0].contains(&P::ChatView),
        "a document cut does not cut the separate chat area"
    );
}

#[tokio::test]
async fn prospective_owned_pages_still_obey_parent_locks_and_token_scopes() {
    let w = World::new().await;
    let user = creator(&w, true).await;
    w.default_row(w.project, P::DocManage, "any", false, true)
        .await;
    let locked = compare(&w, Caller::session(user), None).await;
    assert!(locked[0].contains(&P::DocEdit));
    assert!(!locked[0].contains(&P::DocManage));
    assert!(!locked[1].contains(&P::DocManage));
    let scopes = [
        "project.create_topics",
        "topic.start_threads",
        "topic.view",
        "thread.view",
        "doc.view",
        "chat.view",
    ]
    .map(str::to_owned);
    let clipped = compare(
        &w,
        Caller::token(
            user,
            TokenGrant::from_stored(&scopes, Some(w.project)).unwrap(),
        ),
        None,
    )
    .await;
    assert!(clipped[0].contains(&P::DocView));
    assert!(!clipped[0].contains(&P::DocEdit));
    assert!(!clipped[1].contains(&P::ThreadInteract));
}

#[tokio::test]
async fn a_creation_context_rejected_by_the_normal_page_writer_is_not_treated_as_defaults() {
    let w = World::new().await;
    let user = creator(&w, false).await;
    let caller = Caller::session(user);
    let mut conn = w.conn().await;
    let principal = Principal::load(&mut conn, caller.clone(), w.now)
        .await
        .unwrap();
    assert!(
        creation::agent_permissions(
            &mut conn,
            &principal,
            w.project,
            None,
            None,
            orch_access_contract::permissions::Placement::Managed,
            None
        )
        .await
        .is_ok(),
        "omission retains ordinary creation defaults"
    );
    let topic = node(&mut conn, &principal, w.project, false).await;
    let principal = Principal::load(&mut conn, caller, w.now).await.unwrap();
    let thread = node(&mut conn, &principal, topic, true).await;
    for context in [PageChange::default(), serde_json::from_value(json!({"rows":[{"permission":"doc.manage","add":[{"holder":{"kind":"user","id":user}}]}]})).unwrap()] {
        let actual = pages::change(&mut conn, &principal, Target::Live(thread), &context, policy(), &Default::default()).await;
        assert!(actual.is_err(), "the ordinary page producer must reject this context: {actual:?}");
        assert!(creation::agent_permissions(&mut conn, &principal, w.project, None, Some(&context), orch_access_contract::permissions::Placement::Managed, None).await.is_err(), "an explicit illegal context cannot be collapsed into omitted defaults");
    }
}

#[tokio::test]
async fn resolved_connected_placement_is_used_after_the_same_normal_reset_context() {
    use orch_access_contract::permissions::Placement;
    let w = World::new().await;
    let user = creator(&w, false).await;
    for (permission, profile) in [
        (P::ThreadView, "managed"),
        (P::DocView, "any"),
        (P::ChatView, "any"),
        (P::AccessManage, "any"),
    ] {
        w.name_default(
            Some(w.project),
            permission,
            profile,
            Holder::Role {
                role: orch_access_contract::roles::Role::Viewer,
                at: Some(w.project),
            },
        )
        .await;
    }
    let reset = PageChange {
        whole: Some(Whole::Reset),
        ..PageChange::default()
    };
    let managed = compare_placed(
        &w,
        Caller::session(user),
        Some(&reset),
        Placement::Managed,
        None,
    )
    .await;
    assert!(
        managed[0].contains(&P::DocView),
        "the managed viewers row supplies recipient View after Reset removes the creator entry"
    );
    let mut conn = w.conn().await;
    let environment = orch_db::queries::resources::access_fixtures::insert_environment(
        &mut conn,
        w.boss,
        Some(w.project),
        "A kept connected directory",
        "connected",
    )
    .await
    .unwrap();
    drop(conn);
    let connected = compare_placed(
        &w,
        Caller::session(user),
        Some(&reset),
        Placement::Connected,
        Some(environment),
    )
    .await;
    assert!(
        connected.iter().all(Vec::is_empty),
        "connected placement has no managed-only recipient View; no source gift may bypass that door"
    );
}

#[tokio::test]
async fn fork_bounds_use_the_production_copy_overlay_after_the_birth_page_change_without_residue() {
    use orch_access_contract::permissions::Placement;
    let w = World::new().await;
    let user = creator(&w, false).await;
    w.name(w.topic, P::TopicView, Holder::User { id: user })
        .await;
    w.name_default(
        Some(w.project),
        P::ThreadView,
        "managed",
        Holder::Role {
            role: orch_access_contract::roles::Role::Viewer,
            at: Some(w.project),
        },
    )
    .await;
    for permission in [P::DocView, P::ChatView, P::AccessManage] {
        w.name_default(
            Some(w.project),
            permission,
            "any",
            Holder::Role {
                role: orch_access_contract::roles::Role::Viewer,
                at: Some(w.project),
            },
        )
        .await;
    }
    w.name(w.thread, P::ThreadView, Holder::User { id: user })
        .await;
    for permission in [P::DocView, P::DocEdit, P::DocManage] {
        w.row(w.thread, permission, false).await;
    }
    let change: PageChange =
        serde_json::from_value(json!({"rows":[{"permission":"doc.view","inherit":true}]})).unwrap();
    let mut conn = w.conn().await;
    let principal = Principal::load(&mut conn, Caller::session(user), w.now)
        .await
        .unwrap();
    let before = fork_preview_counts(&mut conn).await;
    let defaults = creation::fork_agent_permissions(
        &mut conn,
        &principal,
        w.topic,
        Some(&change),
        None,
        Placement::Managed,
        None,
    )
    .await
    .unwrap();
    let copied = creation::fork_agent_permissions(
        &mut conn,
        &principal,
        w.topic,
        Some(&change),
        Some(w.thread),
        Placement::Managed,
        None,
    )
    .await
    .unwrap();
    assert!(
        defaults[0].contains(&P::DocView),
        "ordinary destination inheritance supplies document Read"
    );
    assert!(
        !copied[0].contains(&P::DocView),
        "the actual source overlay cuts that inheritance after the birth change"
    );
    assert_eq!(
        fork_preview_counts(&mut conn).await,
        before,
        "both trials leave no node, chat, page row, entry or audit"
    );

    let fork = node(&mut conn, &principal, w.topic, true).await;
    pages::change(
        &mut conn,
        &principal,
        Target::Live(fork),
        &change,
        policy(),
        &Default::default(),
    )
    .await
    .unwrap();
    ownership::carry_fork_page(&mut conn, w.thread, fork, user, w.now)
        .await
        .unwrap();
    let entries = access::entries_of(&mut conn, &access::PageKey::new("node", fork, "live"))
        .await
        .unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.permission == "access.own" && entry.holder_id == Some(w.boss))
    );
    assert!(
        !entries
            .iter()
            .any(|entry| entry.permission == "access.own" && entry.holder_id == Some(w.maker)),
        "the source owner is never carried"
    );
    assert!(entries.iter().any(|entry| entry.permission == "thread.view"
        && entry.holder_id == Some(user)
        && entry.source == "creation"));
    assert_eq!(
        copied[0].contains(&P::DocView),
        can_gift(&mut conn, &principal, fork, fork, false, &[P::DocView]).await
    );
    assert_eq!(
        copied[0].contains(&P::ChatView),
        can_gift(&mut conn, &principal, fork, fork, false, &[P::ChatView]).await
    );
}

async fn fork_preview_counts(conn: &mut orch_db::Conn) -> (i64, i64, i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM nodes),(SELECT count(*) FROM chats),(SELECT count(*) FROM permission_rows),(SELECT count(*) FROM permission_entries),(SELECT count(*) FROM audit_log)")
        .fetch_one(conn).await.unwrap()
}

#[tokio::test]
async fn a_fork_trial_keeps_the_selected_topic_and_rejects_an_unresolved_project() {
    use orch_access_contract::permissions::Placement;
    let w = World::new().await;
    let mut conn = w.conn().await;
    let principal = Principal::load(&mut conn, Caller::session(w.boss), w.now)
        .await
        .unwrap();
    assert!(
        creation::fork_agent_permissions(
            &mut conn,
            &principal,
            w.topic,
            None,
            Some(w.thread),
            Placement::Managed,
            None
        )
        .await
        .is_ok()
    );
    assert!(
        creation::fork_agent_permissions(
            &mut conn,
            &principal,
            w.project,
            None,
            Some(w.thread),
            Placement::Managed,
            None
        )
        .await
        .is_err(),
        "an existing-topic preview cannot invent another topic"
    );
}
