//! Configured retention changes only eligible work and keeps private history and recoverable deletion intact.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use chrono::Duration;
use orch_api_contract::nodes::{
    AuditActor, NewNodeAccess, Node, NodeAuditList, NodeAuditListQuery, NodeGet, TopicCreate,
    TopicCreateBody, TopicCreateReply,
};
use orch_api_contract::ownership::{NodeRestore, NodeRestoreBody};
use orch_api_contract::threads::Delivery;
use orch_db::queries::settings;
use orch_engine::{Engine, EngineParts};
use orch_link_contract::commands::CommandKind;
use orch_types::{Id, Json};
use rig::{Scene, eventually};
use serde_json::json;
use std::sync::Arc;

async fn policy(s: &Scene, node: Id, key: &str, days: i64) {
    let mut conn = s.rig.db.acquire().await.unwrap();
    settings::put(
        &mut conn,
        Some(node),
        key,
        &settings::Stored {
            value: &json!(days),
            locked: false,
            by: Some(s.ada.id()),
            at: s.rig.world.now(),
        },
        None,
    )
    .await
    .unwrap();
}

async fn topic(s: &Scene, name: &str, private: bool) -> Id {
    let reply: TopicCreateReply = s
        .rig
        .world
        .app
        .call(TopicCreate, &[&s.tree.project.base.id])
        .token(&s.ada.token)
        .body(&TopicCreateBody {
            name: Some(name.into()),
            access: private.then_some(NewNodeAccess::Private),
            ..TopicCreateBody::default()
        })
        .send()
        .await
        .ok();
    match reply {
        TopicCreateReply::Topic(topic) => topic.base.id,
        TopicCreateReply::WithThread(_) => panic!("no first thread requested"),
    }
}

async fn old(s: &Scene, ids: &[Id], days: i64) {
    let mut conn = s.rig.db.acquire().await.unwrap();
    sqlx::query("UPDATE nodes SET created_at=$2,updated_at=$2 WHERE id=ANY($1)")
        .bind(ids)
        .bind(s.rig.world.now() - Duration::days(days))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET last_activity_at=$2 WHERE node_id=ANY($1)")
        .bind(ids)
        .bind(s.rig.world.now() - Duration::days(days))
        .execute(&mut *conn)
        .await
        .unwrap();
}

async fn shown(s: &Scene, id: Id) -> Json {
    let node: Node = s
        .rig
        .world
        .app
        .call(NodeGet, &[&id])
        .token(&s.ada.token)
        .send()
        .await
        .ok();
    serde_json::to_value(node).unwrap()
}

fn resumed(s: &Scene) -> Engine {
    let clock = Arc::clone(&s.rig.world.app.app.clock);
    Engine::new(EngineParts {
        db: s.rig.db.clone(),
        clock: Arc::clone(&clock),
        config: rig::engine_config(),
        commands: s.rig.commands.clone(),
        access: Arc::new(orch_api_core::thread_access::AuthzAccess::new(clock)),
        live: Arc::new(orch_engine::live::Discard),
    })
}

#[tokio::test]
async fn idle_age_skips_active_recent_and_opted_out_topics_and_never_widens_a_private_page() {
    let s = Scene::new().await;
    let running = s.thread().await.base.id;
    s.say(&s.ada, running, "keep working", Delivery::Queue, "running")
        .await
        .ok();
    let turn = s.rig.turn_start(running, 0).await;
    s.rig.say(running, rig::turn_began(turn.turn_id)).await;
    s.rig.wait_turn(turn.turn_id, "running").await;
    s.rig.engine.stop();
    let (idle, recent, opted_out, private) = (
        topic(&s, "Idle", false).await,
        topic(&s, "Recent", false).await,
        topic(&s, "Keep open", false).await,
        topic(&s, "Private", true).await,
    );
    old(
        &s,
        &[idle, opted_out, private, s.tree.topic.base.id, running],
        2,
    )
    .await;
    let private_page = shown(&s, private).await["page"].clone();
    assert_eq!(
        s.rig.engine.run_retention().await.unwrap().archived,
        0,
        "the shipped policy is disabled"
    );
    policy(&s, s.tree.org.base.id, "retention.archive_idle_days", 1).await;
    policy(&s, opted_out, "retention.archive_idle_days", 0).await;
    // While another process's run holds the job's lease this one leaves the work to it.
    let other = orch_db::queries::job_leases::Lease::new(
        "retention",
        "another process",
        Duration::minutes(10),
    );
    assert!(other.hold(&s.rig.db, s.rig.world.now()).await.unwrap());
    assert_eq!(
        s.rig.engine.run_retention().await.unwrap().archived,
        0,
        "another process's run holds the job"
    );
    other.release(&s.rig.db).await.unwrap();
    let done = s.rig.engine.run_retention().await.unwrap();
    assert_eq!(done.archived, 2);
    for id in [idle, private] {
        let node = shown(&s, id).await;
        assert_eq!(node["archived"], true);
        assert!(node["archived_by"].is_null());
    }
    for id in [recent, opted_out, s.tree.topic.base.id, s.tree.org.base.id] {
        assert_eq!(shown(&s, id).await["archived"], false);
    }
    assert_eq!(shown(&s, private).await["page"], private_page);
    // Archived, the topic is still there, and still refused to whoever could not read it (a refusal
    // says what is missing; only what is not there or is deleted is not found).
    assert_eq!(
        s.rig
            .world
            .app
            .call(NodeGet, &[&private])
            .token(&s.cai.token)
            .send()
            .await
            .status,
        403
    );
    let audit = s
        .rig
        .world
        .app
        .call(NodeAuditList, &[&private])
        .token(&s.ada.token)
        .query(&NodeAuditListQuery {
            action: Some("node.archived".into()),
            ..NodeAuditListQuery::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(audit.items.len(), 1);
    assert_eq!(audit.items[0].actor, AuditActor::System);
    assert_eq!(s.rig.engine.run_retention().await.unwrap().archived, 0);
    assert!(
        s.rig.seen.of_kind(CommandKind::NodeRuntimeStop).is_empty(),
        "archiving never stops an environment"
    );
    // A new process starts with the same policy and finds new eligible work without duplicating old lines.
    let boot = topic(&s, "Made before restart", false).await;
    old(&s, &[boot], 2).await;
    let next = resumed(&s);
    next.start();
    eventually("startup applies configured retention", || async {
        shown(&s, boot).await["archived"] == true
    })
    .await;
    next.stop();
    let audit = s
        .rig
        .world
        .app
        .call(NodeAuditList, &[&private])
        .token(&s.ada.token)
        .query(&NodeAuditListQuery {
            action: Some("node.archived".into()),
            ..NodeAuditListQuery::default()
        })
        .send()
        .await
        .ok();
    assert_eq!(audit.items.len(), 1);
}

#[tokio::test]
async fn archived_age_deletes_recoverably_settles_pending_work_and_a_restart_does_not_repeat_it() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    s.rig.engine.stop();
    s.say(
        &s.ada,
        thread,
        "queued before retention",
        Delivery::Queue,
        "waiting",
    )
    .await
    .ok();
    let now = s.rig.world.now();
    {
        // A restored older database can contain work still waiting below an archived parent.
        let mut conn = s.rig.db.acquire().await.unwrap();
        sqlx::query("UPDATE nodes SET archived_at=$2 WHERE id=$1")
            .bind(s.tree.topic.base.id)
            .bind(now - Duration::days(3))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let recent = topic(&s, "Just put away", false).await;
    let mut conn = s.rig.db.acquire().await.unwrap();
    orch_db::queries::hierarchy::archive(&mut conn, recent, s.ada.id(), now - Duration::hours(1))
        .await
        .unwrap();
    drop(conn);
    policy(&s, s.tree.org.base.id, "retention.delete_archived_days", 2).await;
    assert_eq!(s.rig.engine.run_retention().await.unwrap().deleted, 1);
    assert_eq!(
        s.rig
            .world
            .app
            .call(NodeGet, &[&s.tree.topic.base.id])
            .token(&s.ada.token)
            .send()
            .await
            .status,
        404
    );
    assert_eq!(shown(&s, recent).await["archived"], true);
    let mut conn = s.rig.db.acquire().await.unwrap();
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM thread_inputs WHERE thread_id=$1 AND state IN ('pending','delivering')")
        .bind(thread).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(pending, 0);
    drop(conn);
    let next = resumed(&s);
    assert_eq!(next.run_retention().await.unwrap().deleted, 0);
    let restored = s
        .rig
        .world
        .app
        .call(NodeRestore, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&NodeRestoreBody::default())
        .send()
        .await
        .ok();
    assert_eq!(serde_json::to_value(restored).unwrap()["archived"], true);
    assert!(
        s.rig.seen.of_kind(CommandKind::TurnStart).is_empty(),
        "restoring starts no queued work"
    );
}
