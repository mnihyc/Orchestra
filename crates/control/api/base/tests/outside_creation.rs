//! Creation by an outside holder retains its creator without granting ownership or membership.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{ProjectCreate, ProjectCreateBody};
use orch_db::queries::access::{self, PageKey};
use places::{admin, make_org, make_project, person};
use support::World;

#[tokio::test]
async fn an_outside_administrators_new_project_names_an_active_owner_and_its_creator() {
    let w = World::new().await;
    let owner = person(&w, "owner").await;
    let creator = admin(&w, "creator").await;
    let org = make_org(&w, &owner, "Team", "team").await;
    let project = make_project(&w, &creator, org.base.id, "Outside creation").await;
    assert_eq!(project.base.owner.id, owner.id());
    assert!(
        !project.base.access.owns,
        "creation grants no effective ownership to the outsider"
    );
    let mut conn = w.conn().await;
    assert!(
        access::members_among(&mut conn, org.base.id, &[creator.id()])
            .await
            .unwrap()
            .is_empty()
    );
    let entries = access::entries_of(&mut conn, &PageKey::new("node", project.base.id, "live"))
        .await
        .unwrap();
    assert!(entries.iter().any(|entry| entry.permission == "access.own"
        && entry.holder_id == Some(owner.id())
        && entry.created_by == Some(creator.id())));
    assert!(
        entries
            .iter()
            .any(|entry| entry.permission == "project.manage"
                && entry.holder_id == Some(creator.id())
                && entry.source == "creation")
    );
    let actor: orch_types::Id = sqlx::query_scalar(
        "SELECT actor_id FROM audit_log WHERE action = 'node.created' AND object_id = $1",
    )
    .bind(project.base.id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(actor, creator.id());
}

#[tokio::test]
async fn creation_without_an_active_first_owner_is_refused_without_a_new_thing() {
    let w = World::new().await;
    let owner = person(&w, "owner").await;
    let creator = admin(&w, "creator").await;
    let org = make_org(&w, &owner, "Team", "team").await;
    let mut conn = w.conn().await;
    sqlx::query("DELETE FROM org_members WHERE org_id = $1")
        .bind(org.base.id)
        .execute(&mut *conn)
        .await
        .unwrap();
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE parent_id = $1")
        .bind(org.base.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let refused = w
        .app
        .call(ProjectCreate, &[&org.base.id])
        .token(&creator.token)
        .body(&ProjectCreateBody {
            name: "Cannot be owned".into(),
            description: None,
            access: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::RequestInvalid);
    assert_eq!(refused.extra["errors"][0]["path"], "owner");
    let mut conn = w.conn().await;
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM nodes WHERE parent_id = $1")
        .bind(org.base.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(after, before);
}
