//! Native recovery and real current ACLs retain pool Use independently from direct member Use.
use super::*;
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_api_contract::{
    binding::EnvironmentChoice,
    nodes::{CreateThread, CreateThreadBinding, ThreadCreate},
    threads::Delivery,
};
use orch_authz::ownership::{NewAccess, start_resource};
use orch_link_contract::events::{Event, TurnCompleted, TurnError, TurnOutcome};
use orch_types::{Id, Time, new_id};

/// The scripted harness transfers history as a file when switching providers.
fn accept_history_handoff(scene: &Scene, thread: Id) {
    use orch_link_contract::commands::{
        Command, CommandKind, CommandResult, FsMaterializeResult, MaterializePurpose,
    };
    let blobs = scene.rig.world.app.app.blobs.clone();
    let environment = scene.on.environment;
    scene
        .rig
        .machine
        .handler
        .on(CommandKind::FsMaterialize, move |command| {
            let Command::FsMaterialize(write) = command else {
                unreachable!()
            };
            assert_eq!(write.environment_id, environment);
            assert_eq!(write.purpose, MaterializePurpose::Delivery);
            let bytes = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    let encoded = blobs
                        .read_whole(&write.manifest_blob, i64::MAX)
                        .await
                        .unwrap();
                    let manifest = orch_engine::manifest::Manifest::parse(&encoded).unwrap();
                    let [entry] = manifest.entries() else {
                        panic!("expected one history transcript")
                    };
                    assert_eq!(entry.path, orch_engine::fork_history::handoff_path(thread));
                    let transcript = blobs
                        .read_whole(&entry.hash.unwrap(), entry.size)
                        .await
                        .unwrap();
                    assert_eq!(i64::try_from(transcript.len()).unwrap(), entry.size);
                    entry.size
                })
            });
            Ok(CommandResult::FsMaterialize(FsMaterializeResult {
                files: 1,
                skipped: 0,
                bytes,
            }))
        });
}

#[tokio::test(flavor = "multi_thread")]
async fn withdrawing_pool_use_stops_only_the_admitted_pool_turn_without_a_new_member_or_paid_attempt()
 {
    let s = Scene::new().await;
    let other = s
        .rig
        .resources_named(&s.ada, &s.tree, "Direct peer workspace")
        .await;
    sqlx::query("UPDATE providers SET agent_id=$2 WHERE id=$1")
        .bind(other.provider)
        .bind(s.on.agent)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let mut conn = s.rig.db.acquire().await.unwrap();
    let pool = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "pool",
        Some(s.eve.id()),
        Some(s.tree.topic.base.id),
        "Scoped runtime providers",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Pool,
            id: pool,
        },
        s.eve.id(),
        NewAccess::OnlyMe,
        Time::new(s.rig.world.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE pools SET kind='provider',definition=$2 WHERE id=$1").bind(pool)
        .bind(json!({"kind":"provider","enabled":true,"policy":{"type":"ordered"},"lend_members":false,"members":[],"config":{}}))
        .execute(&mut *conn).await.unwrap();
    sqlx::query(
        "INSERT INTO pool_members(pool_id,id,position,rule,enabled) VALUES($1,$2,0,$3,true)",
    )
    .bind(pool)
    .bind(new_id())
    .bind(json!({"type":"single","resource":{"kind":"provider","id":other.provider}}))
    .execute(&mut *conn)
    .await
    .unwrap();
    drop(conn);
    s.rig
        .lets_use("pool", pool, "pool.use", &s.ada, json!({"provider":{}}))
        .await;
    s.rig
        .lets_use("pool", pool, "pool.view", &s.ada, json!({}))
        .await;
    let thread = s.thread().await.base.id;
    accept_history_handoff(&s, thread);
    s.rig.world.app.call(NodeSettingSet, &[&s.tree.topic.base.id, &"calls.rules"]).token(&s.ada.token).body(&NodeSettingSetBody { model_choice: None,
        value: json!([{"id":"provider-pool","when":{"kind":"provider_busy"},"steps":[{"type":"failover","candidate":{"target":{"type":"pool","kind":"provider","id":pool},"params":{}}},{"type":"wait","until":"person"}]}]),
        locked: None, expected_revision: None,
    }).send().await.ok();
    s.say(
        &s.ada,
        thread,
        "Work through the configured pool",
        Delivery::Queue,
        "pooled-access-original",
    )
    .await
    .ok();
    let first = s.rig.turn_start(thread, 0).await;
    s.rig.say(thread, rig::turn_began(first.turn_id)).await;
    s.rig.say(thread, Event::TurnCompleted(TurnCompleted {
        turn_id: first.turn_id, status: TurnOutcome::Failed,
        error: Some(serde_json::from_value::<TurnError>(json!({"code":"provider_busy","message":"Synthetic capacity","retryable":true})).unwrap()),
    })).await;
    s.rig.wait_turn(first.turn_id, "failed").await;
    let admitted = tokio::time::timeout(rig::PATIENCE, async {
        while s.rig.turn_starts(thread).len() < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    if admitted.is_err() {
        let report: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT jsonb_build_object('recovery',r.state,'detail',r.context->>'detail','skipped',r.context->>'skipped',
                'due',r.due_at,'held',t.queue_held,'thread',t.state,'current_turn',t.current_turn_id IS NOT NULL,
                'switch',j.state,'phase',j.progress->>'phase','job_error',j.error->>'code','retry',j.plan->'lifecycle_wait'->>'reason_code')
             FROM model_call_recovery r JOIN threads t ON t.node_id=r.node_id
             LEFT JOIN jobs j ON j.id::text=r.context->>'switch_job'
             WHERE r.node_id=$1 AND r.purpose='agent' ORDER BY r.updated_at DESC LIMIT 1"
        ).bind(thread).fetch_optional(s.rig.db.pool()).await.unwrap();
        panic!("the expected pooled turn did not start: {report:?}");
    }
    let picked = s.rig.turn_start(thread, 1).await;
    s.rig.say(thread, rig::turn_began(picked.turn_id)).await;
    s.rig.wait_turn(picked.turn_id, "running").await;
    let admitted_choice: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pool_attempt_scopes a
         JOIN pool_choices c ON c.id=a.choice_id
         WHERE a.attempt_id=$1 AND c.target->>'id'=$2)",
    )
    .bind(picked.turn_id)
    .bind(pool.to_string())
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert!(admitted_choice, "the running turn retains its pool choice");
    let actual: Id = sqlx::query_scalar(
        "SELECT provider_id FROM segments WHERE id=(SELECT segment_id FROM turns WHERE id=$1)",
    )
    .bind(picked.turn_id)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(actual, other.provider);
    let issued = s.rig.turn_starts(thread).len();
    for _ in 0..2 {
        use orch_api_contract::pools::{
            PoolChoiceDetailsGet, PoolChoiceDetailsQuery, PoolChoiceState, PoolChoicesGet,
            PoolChoicesQuery,
        };
        let history = s
            .rig
            .world
            .app
            .call(PoolChoicesGet, &[&pool])
            .token(&s.ada.token)
            .query(&PoolChoicesQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(history.items.len(), 1);
        assert_eq!(history.items[0].state, PoolChoiceState::Selected);
        let details = s
            .rig
            .world
            .app
            .call(PoolChoiceDetailsGet, &[&pool, &history.items[0].id])
            .token(&s.ada.token)
            .query(&PoolChoiceDetailsQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(details.items.len(), 1);
        assert_eq!(
            s.rig.turn_starts(thread).len(),
            issued,
            "reconnecting an actual admitted model choice never starts another turn"
        );
    }
    let direct = s
        .rig
        .world
        .app
        .call(ThreadCreate, &[&s.tree.topic.base.id])
        .token(&s.ada.token)
        .body(&CreateThread {
            name: Some("Direct member work".into()),
            binding: Some(CreateThreadBinding {
                agent_id: s.on.agent,
                provider_id: other.provider,
                arrangement: None,
            }),
            model: Some(rig::MODEL.into()),
            environment: Some(EnvironmentChoice::existing(s.on.environment)),
            directory: Some("web".into()),
            ..CreateThread::default()
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap()
        .base
        .id;
    s.say(
        &s.ada,
        direct,
        "Keep using the provider directly",
        Delivery::Queue,
        "direct-access-original",
    )
    .await
    .ok();
    let direct_turn = s.rig.turn_start(direct, 0).await;
    s.rig
        .say(direct, rig::turn_began(direct_turn.turn_id))
        .await;
    s.rig.wait_turn(direct_turn.turn_id, "running").await;
    let mut tx = s.rig.db.begin().await.unwrap();
    let withdrawn = sqlx::query("UPDATE permission_entries SET removed_at=$3 WHERE object_kind='pool' AND object_id=$1 AND permission='pool.use' AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL")
        .bind(pool).bind(s.ada.id()).bind(s.rig.world.now()).execute(&mut *tx).await.unwrap();
    assert_eq!(withdrawn.rows_affected(), 1);
    orch_authz::epoch::bump(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    s.rig.engine.kick();
    rig::eventually(
        "the real native interrupt reaches only the pooled turn",
        || async {
            s.rig.seen.all().iter().any(|command| {
                matches!(command,
            orch_link_contract::commands::Command::TurnInterrupt(body)
                if body.thread_id == thread && body.turn_id == picked.turn_id)
            })
        },
    )
    .await;
    s.rig
        .say(
            thread,
            Event::TurnCompleted(TurnCompleted {
                turn_id: picked.turn_id,
                status: TurnOutcome::Interrupted,
                error: None,
            }),
        )
        .await;
    s.rig.wait_turn(picked.turn_id, "interrupted").await;
    let cause: Option<String> = sqlx::query_scalar("SELECT interrupt_cause FROM turns WHERE id=$1")
        .bind(picked.turn_id)
        .fetch_one(s.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(cause.as_deref(), Some("pool_access_changed"));
    let direct_running: bool = sqlx::query_scalar(
        "SELECT state='running' AND interrupted_by IS NULL FROM turns WHERE id=$1",
    )
    .bind(direct_turn.turn_id)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert!(
        direct_running,
        "direct Use survives another work item's pool withdrawal"
    );
    assert_eq!(
        s.rig.turn_starts(thread).len(),
        2,
        "withdrawal cannot choose or pay for another member"
    );
    let notices: i64 = sqlx::query_scalar("SELECT count(*) FROM items WHERE turn_id=$1 AND kind='error' AND payload->>'code'='pool_access_changed'")
        .bind(picked.turn_id).fetch_one(s.rig.db.pool()).await.unwrap();
    assert_eq!(
        notices, 1,
        "the same ordinary stop says its safe cause once"
    );
}
