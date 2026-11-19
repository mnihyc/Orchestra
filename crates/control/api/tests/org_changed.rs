//! When an org's tree last changed: each org says it, a change anywhere in its tree moves it on, and an org inside
//! another says its root org's, which is what the rail orders a person's orgs by.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::nodes::{Node, Org, OrgList};
use places::{Who, make_department, make_org, make_project, person, read};
use support::World;

async fn listed(world: &World, by: &Who) -> Vec<Org> {
    world
        .app
        .call(OrgList, &[])
        .token(&by.token)
        .send()
        .await
        .ok()
}

#[tokio::test]
async fn a_change_in_an_orgs_tree_moves_its_changed_time_on_and_a_department_says_its_roots() {
    let world = World::new().await;
    let boss = person(&world, "boss").await;
    let first = make_org(&world, &boss, "First", "first").await;
    let second = make_org(&world, &boss, "Second", "second").await;
    let before = listed(&world, &boss).await;
    let first_before = before
        .iter()
        .find(|org| org.base.id == first.base.id)
        .unwrap()
        .changed_at;
    let second_before = before
        .iter()
        .find(|org| org.base.id == second.base.id)
        .unwrap()
        .changed_at;

    make_project(&world, &boss, first.base.id, "Work").await;
    let after = listed(&world, &boss).await;
    let first_after = after
        .iter()
        .find(|org| org.base.id == first.base.id)
        .unwrap()
        .changed_at;
    let second_after = after
        .iter()
        .find(|org| org.base.id == second.base.id)
        .unwrap()
        .changed_at;
    assert!(
        first_after > first_before,
        "a project made in First moves its time on"
    );
    assert_eq!(second_after, second_before, "nothing changed in Second");
    assert!(
        first_after > second_after,
        "First changed last, so it comes first"
    );

    let department = make_department(&world, &boss, first.base.id, "Labs", "labs").await;
    let Node::Org(seen) = read(&world, &boss, department.base.id).await else {
        panic!("a department reads as an org");
    };
    let root = listed(&world, &boss)
        .await
        .into_iter()
        .find(|org| org.base.id == first.base.id)
        .unwrap();
    assert_eq!(
        seen.changed_at, root.changed_at,
        "a department says its root org's time"
    );
}
