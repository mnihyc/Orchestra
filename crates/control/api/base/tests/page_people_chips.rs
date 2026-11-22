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
            q: None,
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
            q: None,
            holder: format!("role:{}:viewer", page.base.id),
            limit: None,
            cursor: None,
        })
        .send()
        .await
        .ok();
    assert!(people.items.iter().any(|person| person.id == reader.id()));
}

#[tokio::test]
async fn a_people_directory_search_reaches_an_admin_after_the_refusal_preview_budget() {
    let world = World::new().await;
    let reader = person(&world, "directory-reader").await;
    let org = make_org(&world, &reader, "Readers", "readers").await;
    let page = make_project(&world, &reader, org.base.id, "Visible").await;
    let mut conn = world.conn().await;
    let mut ids = Vec::new();
    for index in 0..501_u128 {
        let id = orch_types::Id::from_u128(1000 + index);
        orch_db::queries::identity::testing::insert_user(
            &mut conn,
            id,
            &format!("directory-admin-{index:03}"),
        )
        .await
        .unwrap();
        ids.push(id);
    }
    sqlx::query("UPDATE users SET is_system_admin=true WHERE id=ANY($1)")
        .bind(&ids)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let found = world
        .app
        .call(PermissionPeopleList, &[&"nodes", &page.base.id])
        .token(&reader.token)
        .query(&PermissionPeopleListQuery {
            holder: "admins".into(),
            q: Some("directory-admin-500".into()),
            limit: Some(1),
            cursor: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(found.items.len(), 1);
    assert_eq!(found.items[0].id, ids[500]);
    assert!(found.next_cursor.is_none());
}
