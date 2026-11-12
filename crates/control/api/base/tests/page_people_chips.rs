//! A permission page answers people queries only for holder chips it shows.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::access::{PermissionPeopleList, PermissionPeopleListQuery};
use orch_api_contract::common::ErrorCode;
use places::{make_org, make_project, person};
use support::World;

#[tokio::test]
async fn viewing_one_page_does_not_list_the_members_of_an_unrelated_org() {
    let world = World::new().await;
    let reader = person(&world, "reader").await;
    let owner = person(&world, "owner").await;
    let own_org = make_org(&world, &reader, "Readers", "readers").await;
    let page = make_project(&world, &reader, own_org.base.id, "Visible").await;
    let other_org = make_org(&world, &owner, "Elsewhere", "elsewhere").await;
    let problem = world
        .app
        .call(PermissionPeopleList, &[&"nodes", &page.base.id])
        .token(&reader.token)
        .query(&PermissionPeopleListQuery {
            holder: format!("org_members:{}", other_org.base.id),
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(problem.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_role_chip_that_the_page_shows_still_lists_its_people() {
    let world = World::new().await;
    let reader = person(&world, "reader").await;
    let org = make_org(&world, &reader, "Readers", "readers").await;
    let page = make_project(&world, &reader, org.base.id, "Visible").await;
    let people = world
        .app
        .call(PermissionPeopleList, &[&"nodes", &page.base.id])
        .token(&reader.token)
        .query(&PermissionPeopleListQuery {
            holder: format!("role:{}:viewer", page.base.id),
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .ok();
    assert!(people.items.iter().any(|person| person.id == reader.id()));
}
