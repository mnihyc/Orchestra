//! Archive and deletion settle work atomically and restoration never starts a turn.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::{Node, NodeArchive, NodeGet, NodeUnarchive};
use orch_api_contract::ownership::{NodeDelete, NodeDeleteBody, NodeRestore};
use orch_api_contract::threads::{Delivery, ThreadSyncGet, ThreadSyncGetQuery};
use orch_link_contract::commands::CommandKind;
use orch_types::Id;
use rig::{Scene, eventually};

struct Errors;
impl tracing::Subscriber for Errors {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::ERROR
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        eprintln!("{event:?}");
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

async fn scene() -> Scene {
    static TRACE: std::sync::Once = std::sync::Once::new();
    TRACE.call_once(|| {
        let _ = tracing::subscriber::set_global_default(Errors);
    });
    Scene::new().await
}

async fn delete(scene: &Scene, node: Id) {
    let reply = scene
        .rig
        .world
        .app
        .call(NodeDelete, &[&node])
        .token(&scene.ada.token)
        .body(&NodeDeleteBody::default())
        .send()
        .await;
    assert_eq!(reply.status, 200, "{:?}", reply.body);
}

async fn scalar(scene: &Scene, statement: &str, thread: Id) -> i64 {
    // Every caller passes a fixed SQL literal from this fixture; no request supplies SQL.
    sqlx::query_scalar(sqlx::AssertSqlSafe(statement.to_owned()))
        .bind(thread)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap()
}

async fn side(scene: &Scene, parent: Id, maker: Id) -> Id {
    let id = orch_types::new_id();
    let mut conn = scene.rig.db.acquire().await.unwrap();
    assert!(
        orch_db::queries::threads::forking::insert_side_thread(
            &mut conn,
            &orch_db::queries::threads::forking::NewSide {
                id,
                parent,
                name: "Parent (side)",
                owner: maker,
                from_turn: 0,
                at: scene.rig.world.now()
            }
        )
        .await
        .unwrap()
    );
    // A lane runs in its parent's environment, without having a page of its own.
    sqlx::query(
        "UPDATE threads s SET environment_id=p.environment_id,agent_id=p.agent_id,
        provider_id=p.provider_id,arrangement=p.arrangement,executor=p.executor,tool_set=p.tool_set
        FROM threads p WHERE s.node_id=$1 AND p.node_id=$2",
    )
    .bind(id)
    .bind(parent)
    .execute(&mut *conn)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn a_side_thread_is_closed_by_its_maker_parent_page_manager_or_environment_manager() {
    use orch_api_contract::access::{AccessPreset, ThreadAccess};
    use orch_api_contract::binding::EnvironmentChoice;
    use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
    let scene = scene().await;
    let parent = scene
        .rig
        .world
        .app
        .call(ThreadCreate, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(&CreateThread {
            name: Some("Private parent".to_owned()),
            access: Some(ThreadAccess::Preset(AccessPreset::Private)),
            binding: Some(CreateThreadBinding {
                agent_id: scene.on.agent,
                provider_id: scene.on.provider,
                arrangement: None,
            }),
            environment: Some(EnvironmentChoice::Use(scene.on.environment)),
            model: Some(rig::MODEL.to_owned()),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok()
        .base
        .id;
    let mut conn = scene.rig.db.acquire().await.unwrap();
    orch_db::queries::access::testing::add_entry(
        &mut conn,
        &orch_db::queries::access::PageKey::new("node", parent, "live"),
        "any",
        "thread.manage",
        ("user", Some(scene.cai.id()), None),
        None,
        scene.rig.world.now(),
    )
    .await
    .unwrap();
    orch_db::queries::access::testing::set_row(
        &mut conn,
        &orch_db::queries::access::PageKey::new("environment", scene.on.environment, "live"),
        "any",
        "environment.manage",
        false,
        false,
    )
    .await
    .unwrap();
    drop(conn);
    for by in [&scene.bea, &scene.cai, &scene.eve] {
        let lane = side(&scene, parent, scene.bea.id()).await;
        let denied = scene
            .rig
            .world
            .app
            .call(NodeDelete, &[&lane])
            .token(&scene.dan.token)
            .body(&NodeDeleteBody::default())
            .send()
            .await
            .problem();
        assert_eq!(denied.code, ErrorCode::AuthzDenied);
        let reply = scene
            .rig
            .world
            .app
            .call(NodeDelete, &[&lane])
            .token(&by.token)
            .body(&NodeDeleteBody::default())
            .send()
            .await;
        assert_eq!(reply.status, 200, "{:?}", reply.body);
        let deleted: bool =
            sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM nodes WHERE id=$1")
                .bind(lane)
                .fetch_one(scene.rig.db.pool())
                .await
                .unwrap();
        assert!(deleted);
        settled(&scene, lane).await;
    }
    let lane = side(&scene, parent, scene.bea.id()).await;
    let read_token = scene
        .rig
        .world
        .app
        .call(orch_api_contract::auth::MeTokenCreate, &[])
        .token(&scene.bea.token)
        .body(&orch_api_contract::auth::ApiTokenCreateBody {
            name: "Read only".to_owned(),
            scopes: Some(vec!["thread.view".to_owned()]),
            node_id: None,
            expires_at: None,
            proof: orch_api_contract::auth::SensitiveProof {
                password: Some(support::PASSWORD.to_owned()),
                ..Default::default()
            },
        })
        .send()
        .await
        .ok();
    let refused = scene
        .rig
        .world
        .app
        .call(NodeDelete, &[&lane])
        .token(read_token.token.as_deref().unwrap())
        .body(&NodeDeleteBody::default())
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    let reply = scene
        .rig
        .world
        .app
        .call(NodeDelete, &[&lane])
        .token(&scene.bea.token)
        .body(&NodeDeleteBody::default())
        .send()
        .await;
    assert_eq!(reply.status, 200, "{:?}", reply.body);
    scene.rig.engine.stop();
}

#[tokio::test]
async fn a_new_engine_delivers_archive_controls_committed_by_a_stopped_predecessor() {
    use orch_engine::{Engine, EngineParts};
    use std::sync::Arc;
    let scene = scene().await;
    let thread = scene.thread().await.base.id;
    scene
        .say(&scene.ada, thread, "running", Delivery::Queue, "go")
        .await
        .ok();
    let turn = scene.rig.turn_start(thread, 0).await;
    scene.rig.say(thread, rig::turn_began(turn.turn_id)).await;
    scene.rig.wait_turn(turn.turn_id, "running").await;
    scene.rig.engine.stop();
    scene
        .rig
        .world
        .app
        .call(NodeArchive, &[&thread])
        .token(&scene.ada.token)
        .body(&places::archive_body(&scene.rig.world, &scene.ada.token, thread, true).await)
        .send()
        .await
        .ok();
    assert_eq!(scalar(&scene,"SELECT count(*) FROM commands WHERE body->>'thread_id'=$1::uuid::text AND kind='session.close'",thread).await,1);
    let clock = Arc::clone(&scene.rig.world.app.app.clock);
    let resumed = Engine::new(EngineParts {
        db: scene.rig.db.clone(),
        clock: Arc::clone(&clock),
        config: rig::engine_config(),
        commands: scene.rig.commands.clone(),
        access: Arc::new(orch_api_core::thread_access::AuthzAccess::new(clock)),
        live: Arc::new(orch_engine::live::Discard),
    });
    resumed.start();
    eventually("committed session close delivered",||async {
        scene.rig.seen.of_kind(CommandKind::SessionClose).len()==1
        && scalar(&scene,"SELECT count(*) FROM commands WHERE body->>'thread_id'=$1::uuid::text AND kind='session.close' AND state='succeeded'",thread).await==1
    }).await;
    settled(&scene, thread).await;
    assert_eq!(scene.rig.turn_starts(thread).len(), 1);
    assert_eq!(scene.rig.seen.of_kind(CommandKind::TurnInterrupt).len(), 1);
    resumed.stop();
}

async fn settled(scene: &Scene, thread: Id) {
    assert_eq!(
        scalar(
            scene,
            "SELECT count(*) FROM turns WHERE thread_id = $1 AND state IN ('starting','running')",
            thread
        )
        .await,
        0
    );
    assert_eq!(scalar(scene,"SELECT count(*) FROM thread_inputs WHERE thread_id = $1 AND state IN ('pending','delivering')",thread).await,0);
    assert_eq!(
        scalar(
            scene,
            "SELECT count(*) FROM approvals WHERE thread_id = $1 AND state = 'pending'",
            thread
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            scene,
            "SELECT count(*) FROM leases WHERE thread_id = $1 AND closed_at IS NULL",
            thread
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            scene,
            "SELECT count(*) FROM segments WHERE thread_id = $1 AND ended_at IS NULL",
            thread
        )
        .await,
        0
    );
}

#[tokio::test]
async fn archiving_a_running_thread_cancels_its_queue_and_approvals_once_and_keeps_its_content_readable()
 {
    let scene = scene().await;
    let thread = scene.thread().await.base.id;
    scene
        .say(&scene.ada, thread, "work", Delivery::Queue, "work")
        .await
        .ok();
    let turn = scene.rig.turn_start(thread, 0).await;
    let original_segment = scene.rig.segment_of(thread).await;
    scene.rig.say(thread, rig::turn_began(turn.turn_id)).await;
    scene.rig.wait_turn(turn.turn_id, "running").await;
    scene
        .say(&scene.ada, thread, "waiting", Delivery::Queue, "waiting")
        .await
        .ok();
    scene
        .rig
        .say(
            thread,
            rig::command_asked(turn.turn_id, "ask-before-archive"),
        )
        .await;
    scene.rig.wait_state(thread, "waiting_approval").await;
    let archived = scene
        .rig
        .world
        .app
        .call(NodeArchive, &[&thread])
        .token(&scene.ada.token)
        .body(&places::archive_body(&scene.rig.world, &scene.ada.token, thread, true).await)
        .send()
        .await
        .ok();
    assert!(matches!(archived.node, Node::Thread(ref row) if row.base.archived));
    settled(&scene, thread).await;
    let waiting: (String, Option<String>) = sqlx::query_as(
        "SELECT state,state_detail FROM thread_inputs WHERE thread_id=$1 AND client_id='waiting'",
    )
    .bind(thread)
    .fetch_one(scene.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(
        waiting,
        ("cancelled".to_owned(), Some("archived".to_owned()))
    );
    eventually("native session close", || async {
        scene.rig.seen.of_kind(CommandKind::SessionClose).len() == 1
    })
    .await;
    scene
        .rig
        .world
        .app
        .call(NodeArchive, &[&thread])
        .token(&scene.ada.token)
        .body(&places::archive_body(&scene.rig.world, &scene.ada.token, thread, true).await)
        .send()
        .await
        .ok();
    assert_eq!(scene.rig.seen.of_kind(CommandKind::SessionClose).len(), 1);
    assert_eq!(scene.rig.seen.of_kind(CommandKind::TurnInterrupt).len(), 1);
    assert_eq!(
        scalar(
            &scene,
            "SELECT count(*) FROM items WHERE thread_id=$1 AND payload->>'marker'='archived'",
            thread
        )
        .await,
        1
    );
    let synced = scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.ada.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok();
    assert!(
        rig::said(&synced.items)
            .iter()
            .any(|line| line == "user: work")
    );
    assert_eq!(synced.turns.len(), 1);
    scene
        .rig
        .world
        .app
        .call(NodeUnarchive, &[&thread])
        .token(&scene.ada.token)
        .body(&places::archive_body(&scene.rig.world, &scene.ada.token, thread, false).await)
        .send()
        .await
        .ok();
    settled(&scene, thread).await;
    assert_eq!(scene.rig.turn_starts(thread).len(), 1);
    assert_eq!(scene.rig.state(thread).await.0, "idle");
    scene
        .say(
            &scene.ada,
            thread,
            "resume explicitly",
            Delivery::Queue,
            "resume",
        )
        .await
        .ok();
    scene.rig.turn_start(thread, 1).await;
    assert_ne!(scene.rig.segment_of(thread).await, original_segment);
    let opens = scene.rig.seen.of_kind(CommandKind::SessionOpen);
    let orch_link_contract::commands::Command::SessionOpen(open) = opens.last().unwrap() else {
        panic!("a session was opened")
    };
    assert_eq!(open.mode, orch_link_contract::commands::SessionMode::Resume);
    scene.rig.engine.stop();
}

#[tokio::test]
async fn idle_and_running_ordinary_threads_can_be_deleted_and_restored_archived_without_restarting()
{
    let scene = scene().await;
    for running in [false, true] {
        let thread = scene
            .thread_by(&scene.ada, if running { "Running" } else { "Idle" })
            .await
            .base
            .id;
        if running {
            scene
                .say(&scene.ada, thread, "go", Delivery::Queue, "start")
                .await
                .ok();
            let start = scene.rig.turn_start(thread, 0).await;
            scene.rig.say(thread, rig::turn_began(start.turn_id)).await;
            scene.rig.wait_turn(start.turn_id, "running").await;
            scene
                .say(&scene.ada, thread, "later", Delivery::Queue, "later")
                .await
                .ok();
        }
        let refused = scene
            .rig
            .world
            .app
            .call(NodeDelete, &[&thread])
            .token(&scene.cai.token)
            .body(&NodeDeleteBody::default())
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied);
        assert_eq!(refused.extra["required"], "thread.manage");
        delete(&scene, thread).await;
        settled(&scene, thread).await;
        let missing = scene
            .rig
            .world
            .app
            .call(NodeGet, &[&thread])
            .token(&scene.ada.token)
            .send()
            .await
            .problem();
        assert_eq!(missing.code, ErrorCode::NotFound);
        let restored = scene
            .rig
            .world
            .app
            .call(NodeRestore, &[&thread])
            .token(&scene.ada.token)
            .send()
            .await
            .ok();
        assert!(matches!(restored,Node::Thread(ref row) if row.base.archived));
        assert_eq!(scene.rig.turn_starts(thread).len(), usize::from(running));
        settled(&scene, thread).await;
        let (state, blocked) = scene.rig.state(thread).await;
        assert_eq!(state, "idle", "running: {running}, blocked: {blocked:?}");
    }
    scene.rig.engine.stop();
}

#[tokio::test]
async fn archiving_a_place_settles_its_threads_but_leaves_their_environment_registered() {
    let scene = scene().await;
    let first = scene.thread_by(&scene.ada, "First").await.base.id;
    let second = scene.thread_by(&scene.ada, "Second").await.base.id;
    for thread in [first, second] {
        scene
            .say(&scene.ada, thread, "work", Delivery::Queue, "go")
            .await
            .ok();
        let turn = scene.rig.turn_start(thread, 0).await;
        scene.rig.say(thread, rig::turn_began(turn.turn_id)).await;
        scene.rig.wait_turn(turn.turn_id, "running").await;
    }
    scene
        .rig
        .world
        .app
        .call(NodeArchive, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(
            &places::archive_body(
                &scene.rig.world,
                &scene.ada.token,
                scene.tree.topic.base.id,
                true,
            )
            .await,
        )
        .send()
        .await
        .ok();
    for thread in [first, second] {
        settled(&scene, thread).await;
    }
    let live:i64=sqlx::query_scalar("SELECT count(*) FROM environments WHERE id=$1 AND deleted_at IS NULL AND remove_at IS NULL")
        .bind(scene.on.environment).fetch_one(scene.rig.db.pool()).await.unwrap();
    assert_eq!(live, 1);
    eventually("both native sessions closed", || async {
        scene.rig.seen.of_kind(CommandKind::SessionClose).len() == 2
    })
    .await;
    scene
        .rig
        .world
        .app
        .call(NodeUnarchive, &[&scene.tree.topic.base.id])
        .token(&scene.ada.token)
        .body(
            &places::archive_body(
                &scene.rig.world,
                &scene.ada.token,
                scene.tree.topic.base.id,
                false,
            )
            .await,
        )
        .send()
        .await
        .ok();
    assert_eq!(scene.rig.turn_starts(first).len(), 1);
    assert_eq!(scene.rig.turn_starts(second).len(), 1);
    scene.rig.engine.stop();
}

#[tokio::test]
async fn an_archive_whose_audit_cannot_be_written_commits_no_state_or_native_stop() {
    let scene = scene().await;
    let thread = scene.thread().await.base.id;
    scene
        .say(&scene.ada, thread, "work", Delivery::Queue, "go")
        .await
        .ok();
    let turn = scene.rig.turn_start(thread, 0).await;
    scene.rig.say(thread, rig::turn_began(turn.turn_id)).await;
    scene.rig.wait_turn(turn.turn_id, "running").await;
    sqlx::raw_sql("CREATE FUNCTION reject_archive() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'archive audit refused'; END $$;
        CREATE TRIGGER reject_archive BEFORE INSERT ON audit_log FOR EACH ROW WHEN (NEW.action='node.archived') EXECUTE FUNCTION reject_archive();")
        .execute(scene.rig.db.pool()).await.unwrap();
    let reply = scene
        .rig
        .world
        .app
        .call(NodeArchive, &[&thread])
        .token(&scene.ada.token)
        .body(&places::archive_body(&scene.rig.world, &scene.ada.token, thread, true).await)
        .send()
        .await;
    assert_eq!(reply.status, 500);
    let archived: bool =
        sqlx::query_scalar("SELECT archived_at IS NOT NULL FROM nodes WHERE id=$1")
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
    assert!(!archived);
    assert_eq!(scene.rig.turn_state(turn.turn_id).await, "running");
    assert_eq!(scene.rig.seen.of_kind(CommandKind::TurnInterrupt).len(), 0);
    assert_eq!(scene.rig.seen.of_kind(CommandKind::SessionClose).len(), 0);
    assert_eq!(scalar(&scene,"SELECT count(*) FROM commands WHERE body->>'thread_id'=$1::uuid::text AND kind IN ('turn.interrupt','session.close')",thread).await,0);
    scene.rig.engine.stop();
}

#[tokio::test]
async fn confirmed_environment_deletion_settles_idle_and_running_bindings_and_preserves_segment_history()
 {
    use orch_api_contract::environment::{
        EnvironmentDelete, EnvironmentDeleteQuery, EnvironmentKeep,
    };
    for running in [false, true] {
        let scene = scene().await;
        let thread = scene.thread().await.base.id;
        let original = if running {
            scene
                .say(&scene.ada, thread, "work", Delivery::Queue, "start")
                .await
                .ok();
            let turn = scene.rig.turn_start(thread, 0).await;
            scene.rig.say(thread, rig::turn_began(turn.turn_id)).await;
            scene.rig.wait_turn(turn.turn_id, "running").await;
            scene
                .say(&scene.ada, thread, "waiting", Delivery::Queue, "waiting")
                .await
                .ok();
            Some(scene.rig.segment_of(thread).await)
        } else {
            None
        };
        let at = orch_types::Time::new(scene.rig.world.now() + chrono::Duration::hours(1));
        let query = EnvironmentDeleteQuery {
            confirm: "Workspace".to_owned(),
            at: Some(at),
        };
        let refused = scene
            .rig
            .world
            .app
            .call(EnvironmentDelete, &[&scene.on.environment])
            .token(&scene.cai.token)
            .query(&query)
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::AuthzDenied);
        let result = scene
            .rig
            .world
            .app
            .call(EnvironmentDelete, &[&scene.on.environment])
            .token(&scene.ada.token)
            .query(&query)
            .send()
            .await;
        assert_eq!(result.status, 202, "{:?}", result.body);
        let result = result.ok();
        assert_eq!(result.remove_at, Some(at));
        let frozen = result.removal.clone().unwrap();
        assert_eq!(
            frozen.phase,
            orch_api_contract::environment::EnvironmentRemovalPhase::Scheduled
        );
        assert_eq!(frozen.keep_days, 7);
        settled(&scene, thread).await;
        let current: Option<Id> =
            sqlx::query_scalar("SELECT environment_id FROM threads WHERE node_id=$1")
                .bind(thread)
                .fetch_one(scene.rig.db.pool())
                .await
                .unwrap();
        assert_eq!(current, None);
        assert_eq!(
            scene.rig.state(thread).await,
            ("blocked".to_owned(), Some("unbound".to_owned()))
        );
        if let Some(segment) = original {
            let historical: Id =
                sqlx::query_scalar("SELECT environment_id FROM segments WHERE id=$1")
                    .bind(segment)
                    .fetch_one(scene.rig.db.pool())
                    .await
                    .unwrap();
            assert_eq!(historical, scene.on.environment);
        }
        let jobs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE environment_id=$1 AND kind='environment_remove'",
        )
        .bind(scene.on.environment)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
        assert_eq!(
            jobs, 0,
            "future removal has a frozen promise before any runnable job"
        );
        let repeated = scene
            .rig
            .world
            .app
            .call(EnvironmentDelete, &[&scene.on.environment])
            .token(&scene.ada.token)
            .query(&query)
            .send()
            .await
            .ok();
        assert_eq!(
            repeated.removal,
            Some(frozen),
            "repeating deletion preserves the complete frozen promise"
        );
        let jobs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE environment_id=$1 AND kind='environment_remove'",
        )
        .bind(scene.on.environment)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
        assert_eq!(jobs, 0, "repeating future deletion queues no early removal");
        scene
            .rig
            .world
            .app
            .call(EnvironmentKeep, &[&scene.on.environment])
            .token(&scene.ada.token)
            .send()
            .await
            .ok();
        let queued:i64=sqlx::query_scalar("SELECT count(*) FROM jobs WHERE environment_id=$1 AND kind='environment_remove' AND state IN ('queued','running')")
            .bind(scene.on.environment).fetch_one(scene.rig.db.pool()).await.unwrap();
        assert_eq!(
            queued, 0,
            "Keep it withdraws the queued cleanup before it begins"
        );
        assert_eq!(scene.rig.turn_starts(thread).len(), usize::from(running));
        scene.rig.engine.stop();
    }
}

#[tokio::test]
async fn deleting_a_home_commits_a_cleanup_plan_together_with_its_threads_and_node() {
    let scene = scene().await;
    let thread = scene.thread().await.base.id;
    delete(&scene, scene.tree.topic.base.id).await;
    let mut conn = scene.rig.db.acquire().await.unwrap();
    let promise =
        orch_db::queries::environments::retained::removal(&mut conn, scene.on.environment)
            .await
            .unwrap()
            .unwrap();
    drop(conn);
    assert_eq!(promise.removed_keep_days, Some(7));
    assert_eq!(promise.deleted_by, Some(scene.ada.id()));
    assert_eq!(promise.removal_revision, 1);
    assert!(
        promise.removal_phase.is_some(),
        "the durable cleanup promise commits with the deleted home"
    );
    assert_eq!(
        scalar(
            &scene,
            "SELECT count(*) FROM nodes WHERE id=$1 AND deleted_at IS NOT NULL",
            thread
        )
        .await,
        1
    );
    let current: Option<Id> =
        sqlx::query_scalar("SELECT environment_id FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
    assert_eq!(current, None);
    assert_eq!(scene.rig.turn_starts(thread).len(), 0);
    scene.rig.engine.stop();
}

#[tokio::test]
async fn keeping_an_environment_cannot_withdraw_a_removal_that_begins_while_the_request_waits() {
    use orch_api_contract::environment::{
        EnvironmentDelete, EnvironmentDeleteQuery, EnvironmentKeep,
    };
    let scene = scene().await;
    let environment = scene.on.environment;
    let at = orch_types::Time::new(scene.rig.world.now() + chrono::Duration::hours(1));
    scene
        .rig
        .world
        .app
        .call(EnvironmentDelete, &[&environment])
        .token(&scene.ada.token)
        .query(&EnvironmentDeleteQuery {
            confirm: "Workspace".to_owned(),
            at: Some(at),
        })
        .send()
        .await
        .ok();
    scene.rig.engine.stop();
    let mut beginning = scene.rig.db.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut beginning)
        .await
        .unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *beginning)
        .await
        .unwrap();
    orch_db::queries::environments::hosting::lock_environment(&mut beginning, environment)
        .await
        .unwrap()
        .unwrap();
    let keeper = scene
        .rig
        .world
        .app
        .call(EnvironmentKeep, &[&environment])
        .token(&scene.ada.token)
        .send();
    tokio::pin!(keeper);
    let waiting = async {
        eventually("Keep to wait for the removal's decision", || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity
                WHERE datname=current_database() AND $1=ANY(pg_blocking_pids(pid)))",
            )
            .bind(blocker)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap()
        })
        .await;
    };
    tokio::pin!(waiting);
    tokio::select! {
        reply = &mut keeper => panic!("Keep ended before the removal decision: {}", reply.status),
        () = &mut waiting => {}
    }
    scene.rig.world.advance(chrono::Duration::hours(1));
    let now = scene.rig.world.now();
    let job = orch_engine::jobs::remove::queue(&mut beginning, now, environment, None)
        .await
        .unwrap()
        .id;
    assert!(
        orch_db::queries::jobs::claim(
            &mut beginning,
            job,
            orch_types::new_id(),
            now + chrono::Duration::minutes(1),
            now
        )
        .await
        .unwrap()
    );
    beginning.commit().await.unwrap();
    assert_eq!(keeper.await.problem().code, ErrorCode::PlanBlocked);
    let still_scheduled: bool =
        sqlx::query_scalar("SELECT remove_at IS NOT NULL FROM environments WHERE id=$1")
            .bind(environment)
            .fetch_one(scene.rig.db.pool())
            .await
            .unwrap();
    assert!(
        still_scheduled,
        "Keep cannot withdraw a removal that already began"
    );
}

#[tokio::test]
async fn restoring_a_parent_does_not_reopen_a_side_closed_before_its_deletion_at_the_same_clock_time()
 {
    let scene = scene().await;
    let parent = scene.thread().await.base.id;
    let old = side(&scene, parent, scene.bea.id()).await;
    delete(&scene, old).await;
    let fresh = side(&scene, parent, scene.bea.id()).await;
    delete(&scene, parent).await;
    scene
        .rig
        .world
        .app
        .call(NodeRestore, &[&parent])
        .token(&scene.ada.token)
        .send()
        .await
        .ok();
    let deleted: bool = sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM nodes WHERE id=$1")
        .bind(old)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
    assert!(
        deleted,
        "a prior closed lane is outside the parent's deletion group"
    );
    let live: bool = sqlx::query_scalar("SELECT deleted_at IS NULL FROM nodes WHERE id=$1")
        .bind(fresh)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
    assert!(
        live,
        "a lane taken by this deletion comes back with its parent"
    );
    scene.rig.engine.stop();
}

#[tokio::test]
async fn repeating_an_older_archive_settles_work_that_its_hierarchy_only_implementation_left_running()
 {
    let scene = scene().await;
    let thread = scene.thread().await.base.id;
    scene
        .say(&scene.ada, thread, "running", Delivery::Queue, "go")
        .await
        .ok();
    let turn = scene.rig.turn_start(thread, 0).await;
    scene.rig.say(thread, rig::turn_began(turn.turn_id)).await;
    scene.rig.wait_turn(turn.turn_id, "running").await;
    sqlx::query("UPDATE nodes SET archived_at=$2,archived_by=$3 WHERE id=$1")
        .bind(thread)
        .bind(scene.rig.world.now())
        .bind(scene.ada.id())
        .execute(scene.rig.db.pool())
        .await
        .unwrap();
    scene
        .rig
        .world
        .app
        .call(NodeArchive, &[&thread])
        .token(&scene.ada.token)
        .body(&places::archive_body(&scene.rig.world, &scene.ada.token, thread, true).await)
        .send()
        .await
        .ok();
    settled(&scene, thread).await;
    eventually("legacy native session closed", || async {
        scene.rig.seen.of_kind(CommandKind::SessionClose).len() == 1
    })
    .await;
    scene
        .rig
        .world
        .app
        .call(NodeArchive, &[&thread])
        .token(&scene.ada.token)
        .body(&places::archive_body(&scene.rig.world, &scene.ada.token, thread, true).await)
        .send()
        .await
        .ok();
    assert_eq!(scene.rig.seen.of_kind(CommandKind::SessionClose).len(), 1);
    scene.rig.engine.stop();
}
