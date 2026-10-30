//! Audit readers retain thread creation and unfamiliar stored actions without losing the rest of the trail.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::admin::{AdminAuditList, AdminAuditListQuery};
use orch_api_contract::nodes::{AuditEntry, AuditKind, NodeAuditList, NodeAuditListQuery};
use orch_types::{Id, new_id};

fn keeps_both(rows: &[AuditEntry], thread: Id, unfamiliar: Id) {
    let created = rows
        .iter()
        .find(|row| row.action == "thread.created" && row.object.id == Some(thread))
        .unwrap();
    assert_eq!(created.kind, Some(AuditKind::Made));
    assert!(created.history);
    let retained = rows.iter().find(|row| row.id == unfamiliar).unwrap();
    assert_eq!(retained.action, "future.reindexed");
    assert_eq!(
        retained.kind, None,
        "an unfamiliar action receives no guessed classification"
    );
    assert_eq!(retained.object.id, Some(thread));
    assert_eq!(retained.summary, "A retained observation");
}

#[tokio::test]
async fn a_created_thread_and_an_unfamiliar_action_are_read_in_org_thread_and_management_trails() {
    let scene = rig::Scene::new().await;
    let world = &scene.rig.world;
    world.make_admin(&scene.ada.person).await;
    let thread = scene
        .thread_by(&scene.ada, "An audited thread")
        .await
        .base
        .id;
    let unfamiliar = new_id();
    // As every writer does, the row keeps the places it lay in when it was written, which is what
    // the trails of those places read.
    sqlx::query("INSERT INTO audit_log(id,at,actor_kind,action,object_kind,object_id,node_id,scope_node_ids,summary) SELECT $1,$2,'system','future.reindexed','thread',$3,$3,n.path,'A retained observation' FROM nodes n WHERE n.id=$3")
        .bind(unfamiliar).bind(world.now()).bind(thread).execute(&world.db.pool).await.unwrap();

    for node in [scene.tree.org.base.id, thread] {
        let rows = world
            .app
            .call(NodeAuditList, &[&node])
            .token(&scene.ada.token)
            .query(&NodeAuditListQuery::default())
            .send()
            .await
            .ok();
        keeps_both(&rows.items, thread, unfamiliar);
    }
    // The system's trail is read as its admin, with the session's As admin switch on.
    places::admin_mode(world, &scene.ada, true).await;
    let system = world
        .app
        .call(AdminAuditList, &[])
        .token(&scene.ada.token)
        .query(&AdminAuditListQuery::default())
        .send()
        .await
        .ok();
    keeps_both(&system.items, thread, unfamiliar);

    let made = world
        .app
        .call(NodeAuditList, &[&thread])
        .token(&scene.ada.token)
        .query(&NodeAuditListQuery {
            kind: Some(AuditKind::Made),
            ..Default::default()
        })
        .send()
        .await
        .ok();
    assert!(made.items.iter().any(|row| row.action == "thread.created"));
    assert!(
        made.items
            .iter()
            .all(|row| row.kind == Some(AuditKind::Made))
    );
    assert!(made.items.iter().all(|row| row.id != unfamiliar));
}
