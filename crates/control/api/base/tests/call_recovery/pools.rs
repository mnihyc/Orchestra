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
    rig::eventually("the pool admits its configured replacement", || async {
        if s.rig.turn_starts(thread).len() >= 2 {
            return true;
        }
        let ended: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT state,COALESCE(context->>'skipped',context->>'detail') FROM model_call_recovery WHERE node_id=$1 AND purpose='agent' AND state IN ('stopped','waiting','dismissed')",
        ).bind(thread).fetch_optional(s.rig.db.pool()).await.unwrap();
        assert!(ended.is_none(), "recovery ended before admitting its pooled turn: {ended:?}");
        false
    }).await;
    let picked = s.rig.turn_start(thread, 1).await;
    s.rig.say(thread, rig::turn_began(picked.turn_id)).await;
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
            environment: Some(EnvironmentChoice::Use(s.on.environment)),
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
    let mut tx = s.rig.db.begin().await.unwrap();
    sqlx::query("UPDATE permission_entries SET removed_at=$3 WHERE object_kind='pool' AND object_id=$1 AND permission='pool.use' AND holder_kind='user' AND holder_id=$2 AND removed_at IS NULL")
        .bind(pool).bind(s.ada.id()).bind(s.rig.world.now()).execute(&mut *tx).await.unwrap();
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
