//! Shipped thread rows follow control of the environment section, with shared human, agent and inventory answers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_access_contract::{
    pages::Holder,
    permissions::{Permission, Placement},
    roles::Role,
};
use orch_authz::{Caller, Frame, Policy, engine::Graph};
use orch_db::queries::{
    access, everything,
    resources::{self, access_fixtures},
};
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use support::World;

async fn report(w: &World, machine: Id, delegated: &[&str], label: &str) {
    let mut tx = w.db.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    resources::set_machine_facts(
        &mut tx,
        machine,
        &json!({"hostname":label,
        "environment_descriptor":{"lifecycle":"attach"},"policy":{"policy.delegated":delegated}}),
        (0, 0),
        0.5,
        w.now.get(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

async fn attach(w: &World, thread: Id, placement: Placement, delegated: &[&str]) -> Option<Id> {
    let mut conn = w.conn().await;
    let environment = access_fixtures::insert_environment(
        &mut conn,
        w.maker,
        Some(w.topic),
        &format!("Column environment {thread}"),
        placement.as_str(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE threads SET environment_id=$2 WHERE node_id=$1")
        .bind(thread)
        .bind(environment)
        .execute(&mut *conn)
        .await
        .unwrap();
    let machine: Option<Id> = sqlx::query_scalar("SELECT machine_id FROM environments WHERE id=$1")
        .bind(environment)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    if let Some(machine) = machine {
        report(w, machine, delegated, "first report").await;
    }
    machine
}

async fn held(w: &World, person: Id, threads: &[Id]) -> HashMap<Id, HashSet<Permission>> {
    let principal = w.principal(Caller::session(person)).await;
    let mut conn = w.conn().await;
    let graph = Graph::load(&mut conn, threads).await.unwrap();
    let sites: Vec<_> = threads
        .iter()
        .map(|id| graph.site_of(*id).unwrap())
        .collect();
    let frame = Frame::load(
        &mut conn,
        &principal,
        Policy::default(),
        graph,
        &sites,
        Permission::ALL,
    )
    .await
    .unwrap();
    sites
        .iter()
        .map(|site| {
            (
                site.id,
                frame
                    .effective(site)
                    .into_iter()
                    .map(|held| held.permission)
                    .collect(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_full_access_directory_uses_the_units_rows_while_a_kept_directory_stays_with_its_owners()
{
    let w = World::new().await;
    let interactor = w.member("interactor").await;
    let developer = w.member("developer").await;
    w.give(w.project, interactor, Role::Interactor).await;
    w.give(w.project, developer, Role::Developer).await;
    let unit = w.node("thread", w.topic, "Column unit").await;
    let handed = w.node("thread", w.topic, "Column handed directory").await;
    let kept = w.node("thread", w.topic, "Column kept directory").await;
    let _ = attach(&w, unit, Placement::Managed, &[]).await;
    let _ = attach(&w, handed, Placement::Connected, &["environment.*"]).await;
    let _ = attach(
        &w,
        kept,
        Placement::Connected,
        &["environment.remote_write"],
    )
    .await;
    let threads = [unit, handed, kept];
    for person in [interactor, developer] {
        let rights = held(&w, person, &threads).await;
        assert_eq!(
            rights[&unit], rights[&handed],
            "every observable row agrees with a unit, not just Interact"
        );
        assert!(rights[&handed].contains(&Permission::ThreadInteract));
        assert!(!rights[&kept].contains(&Permission::ThreadInteract));
        assert!(!rights[&kept].contains(&Permission::ThreadManage));
        assert!(rights[&kept].contains(&Permission::ThreadView));
        assert_eq!(
            rights[&handed].contains(&Permission::ThreadManage),
            person == developer
        );
    }
    let owners = held(&w, w.maker, &threads).await;
    for thread in threads {
        assert!(owners[&thread].contains(&Permission::ThreadManage));
    }
    let mut conn = w.conn().await;
    assert_eq!(
        Graph::load(&mut conn, &[handed])
            .await
            .unwrap()
            .site_of(handed)
            .unwrap()
            .placement,
        Placement::Connected,
        "the defaults selector never changes the environment's actual placement"
    );
    drop(conn);

    // A share on only one defaults column must use the same selection for an agent.
    let observer = w.node("thread", w.topic, "Column observer").await;
    w.name_default(
        Some(w.topic),
        Permission::ThreadView,
        "managed",
        Holder::Agents { id: observer },
    )
    .await;
    let mut conn = w.conn().await;
    let agent = orch_authz::reach::Agent::of(&mut conn, observer)
        .await
        .unwrap()
        .unwrap();
    let reach = orch_authz::reach::Reach::load(
        &mut conn,
        agent,
        orch_authz::reach::AgentReach::None,
        w.now,
        &orch_authz::reach::Things {
            nodes: threads.to_vec(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        reach.conversation(unit, false).read,
        Some(orch_authz::reach::Via::Share)
    );
    assert_eq!(
        reach.conversation(handed, false).read,
        Some(orch_authz::reach::Via::Share)
    );
    assert_eq!(reach.conversation(kept, false).read, None);
    drop(conn);

    // Ordinary threads have no exceptional page to load. Both whole-tree and branch
    // inventory projections still apply the physical machine's same reported choice.
    let principal = w.principal(Caller::session(developer)).await;
    let mut conn = w.conn().await;
    for parent in [None, Some(w.topic)] {
        let projection = orch_authz::everything::project(&mut conn, &principal, parent)
            .await
            .unwrap();
        let parent = parent.map(|id| id.to_string());
        let kinds = vec!["thread".to_owned()];
        let query = everything::Query {
            navigation: Default::default(),
            resource: Default::default(),
            admin_access: None,
            participating: false,
            user: developer,
            groups: principal.groups(),
            member_roots: &projection.member_roots,
            grants: &projection.grants,
            parent: parent.as_deref(),
            system: false,
            text: "Column ",
            kinds: &kinds,
            owner: None,
            created_from: None,
            created_to: None,
            active_from: None,
            sort: "name",
            descending: false,
            after: None,
            limit: 20,
            at: w.now.get(),
        };
        let listed = everything::level(&mut conn, &query).await.unwrap();
        let level = |id: Id| -> &Json {
            &listed
                .iter()
                .find(|row| row.id == id.to_string())
                .unwrap()
                .grant["access"]
        };
        assert_eq!(level(unit).as_str(), Some("manage"));
        assert_eq!(level(handed).as_str(), Some("manage"));
        assert_eq!(level(kept).as_str(), Some("view"));
    }
}

#[tokio::test]
async fn changing_the_reported_section_control_ends_cached_answers_but_repeating_facts_does_not() {
    let w = World::new().await;
    let machine = attach(&w, w.thread, Placement::Connected, &[])
        .await
        .unwrap();
    let mut conn = w.conn().await;
    let before = access::epoch(&mut conn).await.unwrap();
    drop(conn);
    report(&w, machine, &["environment.*"], "handed over").await;
    let mut conn = w.conn().await;
    let handed = access::epoch(&mut conn).await.unwrap();
    drop(conn);
    assert!(
        handed > before,
        "a previously denied/allowed frame must be decided again"
    );
    for delegated in [&["environment.*"][..], &["*"][..]] {
        report(&w, machine, delegated, "only the hostname changed").await;
        let mut conn = w.conn().await;
        assert_eq!(access::epoch(&mut conn).await.unwrap(), handed);
    }
    report(
        &w,
        machine,
        &["environment.remote_write"],
        "section kept again",
    )
    .await;
    let mut conn = w.conn().await;
    assert!(access::epoch(&mut conn).await.unwrap() > handed);
}
