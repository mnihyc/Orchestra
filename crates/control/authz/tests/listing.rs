//! Lists: which of many nodes a caller may see, the children of a node, restricted nodes, and what a decision costs in statements.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::permissions::{Permission as P, PermissionName};
use orch_access_contract::roles::{NodeKind, Role};
use orch_authz::nodes::Page;
use orch_authz::{Act, Caller, Policy, nodes};
use orch_types::Id;
use support::{Statements, World};

fn named(text: &str) -> PermissionName {
    text.parse().unwrap()
}

async fn holding(w: &World, who: Id, ids: &[Id], name: &str) -> Vec<Id> {
    let principal = w.principal(Caller::session(who)).await;
    let mut conn = w.conn().await;
    nodes::holding(
        &mut conn,
        &principal,
        ids,
        named(name),
        Act::Use,
        Policy::default(),
    )
    .await
    .unwrap()
    .into_iter()
    .map(|listed| listed.id)
    .collect()
}

#[tokio::test]
async fn of_many_nodes_the_ones_the_caller_may_see_are_listed_in_the_order_asked() {
    let w = World::new().await;
    let d = w.deep().await;
    let viewer = w.user("viewer").await;
    w.give(d.project, viewer, Role::Viewer).await;
    let hidden = w.node("thread", d.topic, "Hidden").await;
    w.row(hidden, P::ThreadView, false).await;
    let ids = [
        w.thread,
        d.thread,
        hidden,
        d.topic,
        d.sales_project,
        d.project,
        Id::nil(),
    ];
    let seen = holding(&w, viewer, &ids, "node.view").await;
    assert_eq!(
        seen,
        [d.thread, d.topic, d.project],
        "the order asked, and not what is hidden or is not there"
    );
    // The shorthand means the permission of each node's own area.
    let boss = holding(&w, w.boss, &ids[..6], "node.manage").await;
    assert_eq!(boss.len(), 6);
    // The same permission by its name, for the kind that has it.
    let interactors = holding(&w, viewer, &ids, "thread.interact").await;
    assert!(interactors.is_empty());
}

#[tokio::test]
async fn a_list_says_which_nodes_are_restricted_and_which_are_archived() {
    let w = World::new().await;
    let d = w.deep().await;
    let other = w.node("thread", d.topic, "Other").await;
    let cut_by_default = w
        .node(
            "thread",
            w.node("topic", d.project, "Quiet").await,
            "Quiet thread",
        )
        .await;
    w.row(d.thread, P::ThreadView, false).await;
    // A default on the way that is cut loose restricts what follows it.
    let quiet_topic = w.node("topic", d.project, "Quieter").await;
    w.default_row(quiet_topic, P::ThreadView, "managed", false, false)
        .await;
    let quiet_thread = w.node("thread", quiet_topic, "Fix").await;
    {
        let mut conn = w.conn().await;
        orch_db::queries::hierarchy::archive(&mut conn, other, w.boss, w.now.get())
            .await
            .unwrap();
    }
    let principal = w.principal(Caller::session(w.boss)).await;
    let mut conn = w.conn().await;
    let listed = nodes::holding(
        &mut conn,
        &principal,
        &[d.thread, other, cut_by_default, quiet_thread, d.topic],
        named("node.view"),
        Act::Use,
        Policy::default(),
    )
    .await
    .unwrap();
    let by_id = |id: Id| listed.iter().find(|item| item.id == id).unwrap();
    assert!(by_id(d.thread).restricted, "its own row");
    assert!(!by_id(other).restricted && by_id(other).archived);
    assert!(!by_id(cut_by_default).restricted);
    assert!(by_id(quiet_thread).restricted, "a default on the way");
    assert!(!by_id(d.topic).restricted);
    assert_eq!(by_id(d.thread).kind, NodeKind::Thread);
    assert_eq!(by_id(d.thread).name, "Train");
}

#[tokio::test]
async fn the_children_of_a_node_are_the_ones_the_caller_may_see_a_page_at_a_time() {
    let w = World::new().await;
    let viewer = w.user("viewer").await;
    w.give(w.topic, viewer, Role::Viewer).await;
    let mut children = Vec::new();
    for index in 0..7 {
        let child = w.node("thread", w.topic, &format!("T{index}")).await;
        children.push(child);
    }
    // Some are cut loose and so not seen by a Viewer.
    for hidden in [children[1], children[2], children[5]] {
        w.row(hidden, P::ThreadView, false).await;
    }
    let seen: Vec<Id> = [children[0], children[3], children[4], children[6]].to_vec();
    let principal = w.principal(Caller::session(viewer)).await;
    let mut conn = w.conn().await;
    let first = nodes::children(
        &mut conn,
        &principal,
        w.topic,
        named("node.view"),
        Act::Use,
        Policy::default(),
        Page {
            after: None,
            limit: 3,
        },
    )
    .await
    .unwrap();
    let ids = |page: &nodes::Children| page.items.iter().map(|item| item.id).collect::<Vec<_>>();
    // The thread of the world comes first (it was made first).
    let mut all = vec![w.thread];
    all.extend(seen.iter().copied());
    assert_eq!(ids(&first), all[..3]);
    assert_eq!(first.next, Some(all[2]));
    let second = nodes::children(
        &mut conn,
        &principal,
        w.topic,
        named("node.view"),
        Act::Use,
        Policy::default(),
        Page {
            after: first.next,
            limit: 3,
        },
    )
    .await
    .unwrap();
    assert_eq!(ids(&second), all[3..]);
    assert_eq!(second.next, None, "no more");
    // The boss sees every child.
    let boss = w.principal(Caller::session(w.boss)).await;
    let everything = nodes::children(
        &mut conn,
        &boss,
        w.topic,
        named("node.view"),
        Act::Use,
        Policy::default(),
        Page {
            after: None,
            limit: 50,
        },
    )
    .await
    .unwrap();
    assert_eq!(everything.items.len(), 8);
    assert_eq!(everything.next, None);
    let missing = nodes::children(
        &mut conn,
        &boss,
        Id::nil(),
        named("node.view"),
        Act::Use,
        Policy::default(),
        Page {
            after: None,
            limit: 5,
        },
    )
    .await;
    assert!(matches!(missing, Err(orch_authz::Error::NotFound(_))));
}

#[tokio::test]
async fn what_a_person_holds_on_a_node_is_listed_with_how_and_where_their_role_comes_from() {
    let w = World::new().await;
    let dev = w.user("dev").await;
    w.give(w.project, dev, Role::Developer).await;
    let principal = w.principal(Caller::session(dev)).await;
    let mut conn = w.conn().await;
    let effective = nodes::effective(&mut conn, &principal, w.thread, Policy::default())
        .await
        .unwrap();
    let role = effective.role.unwrap();
    assert_eq!(role.role, Role::Developer);
    assert_eq!(
        role.at,
        orch_access_contract::roles::PlaceRef::Node(w.project),
        "where it is held"
    );
    assert!(!effective.administers && !effective.owner && !effective.restricted);
    let manage = effective
        .held
        .iter()
        .find(|held| held.permission == P::ThreadManage)
        .unwrap();
    assert!(matches!(
        manage.through[..],
        [orch_access_contract::decisions::Through::Entry(_)]
    ));
    let view = effective
        .held
        .iter()
        .find(|held| held.permission == P::ThreadView)
        .unwrap();
    assert!(
        view.through.iter().any(|way| matches!(
            way,
            orch_access_contract::decisions::Through::Level {
                of: P::ThreadManage
            }
        )),
        "a higher level holds it too"
    );
    assert!(effective.held.iter().all(|held| !held.through.is_empty()));
}

/// The statements a decision takes, counted over the whole of it.
async fn statements_of(w: &World, who: Id, ids: &[Id]) -> usize {
    let principal = w.principal(Caller::session(who)).await;
    let mut conn = w.conn().await;
    let before = Statements::now();
    let listed = nodes::holding(
        &mut conn,
        &principal,
        ids,
        named("node.view"),
        Act::Use,
        Policy::default(),
    )
    .await
    .unwrap();
    assert!(!listed.is_empty());
    Statements::now() - before
}

#[tokio::test]
async fn the_statements_of_a_listing_do_not_grow_with_the_number_of_nodes() {
    let w = World::new().await;
    let viewer = w.user("viewer").await;
    w.give(w.root, viewer, Role::Viewer).await;
    // Many threads across several topics of several projects, some cut loose.
    let mut threads = vec![w.thread];
    for project in 0..4 {
        let project = w.node("project", w.root, &format!("P{project}")).await;
        for topic in 0..3 {
            let topic = w.node("topic", project, &format!("T{topic}")).await;
            for thread in 0..5 {
                let thread = w.node("thread", topic, &format!("H{thread}")).await;
                if thread.as_u128() % 3 == 0 {
                    w.row(thread, P::ThreadView, false).await;
                }
                threads.push(thread);
            }
        }
    }
    assert_eq!(threads.len(), 61);
    let few = statements_of(&w, viewer, &threads[..3]).await;
    let many = statements_of(&w, viewer, &threads).await;
    assert_eq!(few, many, "the same statements for 3 nodes and for 61");
    assert!(many <= 8, "a listing takes {many} statements");
    // The same for a person who holds nothing by role but is named in places.
    let named_viewer = w.user("named").await;
    w.name(
        threads[10],
        P::ThreadView,
        orch_access_contract::pages::Holder::User { id: named_viewer },
    )
    .await;
    w.name(
        threads[20],
        P::ThreadView,
        orch_access_contract::pages::Holder::Role {
            role: Role::Developer,
            at: Some(w.project),
        },
    )
    .await;
    let few = statements_of_any(&w, named_viewer, &threads[..3]).await;
    let many = statements_of_any(&w, named_viewer, &threads).await;
    assert_eq!(few, many);
}

async fn statements_of_any(w: &World, who: Id, ids: &[Id]) -> usize {
    let principal = w.principal(Caller::session(who)).await;
    let mut conn = w.conn().await;
    let before = Statements::now();
    nodes::holding(
        &mut conn,
        &principal,
        ids,
        named("node.view"),
        Act::Use,
        Policy::default(),
    )
    .await
    .unwrap();
    Statements::now() - before
}

#[tokio::test]
async fn the_statements_of_a_listing_of_children_do_not_grow_with_the_children() {
    let w = World::new().await;
    let viewer = w.user("viewer").await;
    w.give(w.root, viewer, Role::Viewer).await;
    let count = |extra: usize| {
        let w = &w;
        async move {
            let topic = w.node("topic", w.project, "Many").await;
            for index in 0..extra {
                w.node("thread", topic, &format!("c{index}")).await;
            }
            let principal = w.principal(Caller::session(viewer)).await;
            let mut conn = w.conn().await;
            let before = Statements::now();
            let page = nodes::children(
                &mut conn,
                &principal,
                topic,
                named("node.view"),
                Act::Use,
                Policy::default(),
                Page {
                    after: None,
                    limit: 90,
                },
            )
            .await
            .unwrap();
            assert_eq!(page.items.len(), extra);
            Statements::now() - before
        }
    };
    assert_eq!(count(5).await, count(80).await);
}

#[tokio::test]
async fn one_decision_takes_a_handful_of_statements_whoever_asks_and_whatever_the_answer() {
    let w = World::new().await;
    let d = w.deep().await;
    let dev = w.user("dev").await;
    w.give(d.project, dev, Role::Developer).await;
    let principal = w.principal(Caller::session(dev)).await;
    let mut conn = w.conn().await;
    let start = Statements::now();
    let allowed = nodes::check(
        &mut conn,
        &principal,
        d.thread,
        P::ThreadManage,
        Act::Use,
        Policy::default(),
    )
    .await
    .unwrap();
    let allowed_statements = Statements::now() - start;
    assert!(allowed.is_allowed());
    assert!(
        allowed_statements <= 7,
        "an allowed decision takes {allowed_statements} statements"
    );
    let before = Statements::now();
    let refused = nodes::check(
        &mut conn,
        &principal,
        d.thread,
        P::AccessManage,
        Act::Use,
        Policy::default(),
    )
    .await
    .unwrap();
    assert!(!refused.is_allowed());
    let refused_statements = Statements::now() - before;
    assert!(
        refused_statements <= 20,
        "a refusal, with who can give it, takes {refused_statements} statements"
    );
}
