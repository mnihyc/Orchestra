//! The real turn writer and recovery cursor own every pool attempt, including after restart.
use super::*;
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_authz::ownership::{NewAccess, start_resource};

pub(super) async fn model_pool(world: &World) -> Id {
    let mut conn = world.pool.acquire().await.unwrap();
    let pool = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "pool",
        Some(world.ada),
        Some(world.project),
        "Recovery models",
    )
    .await
    .unwrap();
    start_resource(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Pool,
            id: pool,
        },
        world.ada,
        NewAccess::OnlyMe,
        orch_types::Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE pools SET definition=$2 WHERE id=$1").bind(pool).bind(json!({"kind":"model","enabled":true,"policy":{"type":"ordered"},"lend_members":false,"members":[],"config":{}})).execute(&mut *conn).await.unwrap();
    for (position, model) in [common::MODEL, "first-pool-model", "second-pool-model"]
        .iter()
        .enumerate()
    {
        if *model != common::MODEL {
            sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,enabled,source,created_by) SELECT $1,provider_id,$3,$3,parameters,true,'manual',created_by FROM provider_models WHERE provider_id=$2 AND model_name=$4").bind(orch_types::new_id()).bind(world.provider).bind(model).bind(common::MODEL).execute(&mut *conn).await.unwrap();
        }
        let resource = json!({"kind":"model","backend":{"route":"generation.model","offering":{"provider":world.provider,"offering":model},"config":{}}});
        sqlx::query(
            "INSERT INTO pool_members(pool_id,id,position,rule,enabled) VALUES($1,$2,$3,$4,true)",
        )
        .bind(pool)
        .bind(orch_types::new_id())
        .bind(i64::try_from(position).unwrap())
        .bind(json!({"type":"single","resource":resource}))
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    pool
}
async fn use_pool(world: &World, pool: Id) {
    world.set(world.topic,"calls.rules",json!([{"id":"selected-pool","when":{"kind":"provider_busy"},"steps":[{"type":"failover","candidate":{"target":{"type":"pool","kind":"model","id":pool},"params":{}}},{"type":"wait","until":"person"}]}])).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_zero_pool_attempt_limit_waits_without_starting_another_member() {
    let world = World::new().await;
    let pool = model_pool(&world).await;
    let thread = world.thread("A pool with no additional attempts").await;
    use_pool(&world, pool).await;
    world.set(world.root, "pools.attempts", json!(0)).await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Make the change"),
        )
        .await
        .unwrap();
    let original = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(original.turn_id)).await;
    failed(&world, thread, original.turn_id, "provider_busy").await;
    eventually(
        "the explicit zero waits without choosing a member",
        || async { recovery(&world, thread).await.state == "waiting" },
    )
    .await;
    assert_eq!(world.turn_starts(thread).len(), 1);
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM pool_choices WHERE source->>'node'=$1::uuid::text",
                thread
            )
            .await,
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_configured_pool_uses_each_member_once_and_keeps_its_admitted_choice_after_restart() {
    let world = World::new().await;
    let pool = model_pool(&world).await;
    let thread = world.thread("Pool recovery").await;
    use_pool(&world, pool).await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Make the change"),
        )
        .await
        .unwrap();
    let original = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(original.turn_id)).await;
    failed(&world, thread, original.turn_id, "provider_busy").await;
    let first = world.turn_start(thread, 1).await;
    assert_eq!(first.model, "first-pool-model");
    world.say(thread, turn_began(first.turn_id)).await;
    let before = recovery(&world, thread).await.context["candidate"].clone();
    world.restart_api().await;
    assert_eq!(recovery(&world, thread).await.context["candidate"], before);
    assert_eq!(world.turn_starts(thread).len(), 2);
    failed(&world, thread, first.turn_id, "provider_busy").await;
    let second = world.turn_start(thread, 2).await;
    assert_eq!(second.model, "second-pool-model");
    world.say(thread, turn_began(second.turn_id)).await;
    failed(&world, thread, second.turn_id, "provider_busy").await;
    eventually("the configured pool is exhausted", || async {
        recovery(&world, thread).await.state == "waiting"
    })
    .await;
    assert_eq!(world.turn_starts(thread).len(), 3);
    let root: Id = world
        .scalar("SELECT input_id FROM turns WHERE id=$1", original.turn_id)
        .await;
    let attribution: (Id, i64) =
        sqlx::query_as("SELECT min(author_id::text)::uuid,count(*) FROM thread_inputs WHERE id=$1")
            .bind(root)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(attribution, (world.ada, 1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_callers_repeat_guard_stops_pool_recovery_before_another_member_starts() {
    let world = World::new().await;
    let pool = model_pool(&world).await;
    let thread = world.thread("A bounded pool recovery").await;
    use_pool(&world, pool).await;
    world.set(world.topic, "calls.repeat_limit", json!(1)).await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Make the change"),
        )
        .await
        .unwrap();
    let original = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(original.turn_id)).await;
    failed(&world, thread, original.turn_id, "provider_busy").await;
    let first = world.turn_start(thread, 1).await;
    world.say(thread, turn_began(first.turn_id)).await;
    failed(&world, thread, first.turn_id, "provider_busy").await;
    eventually("the ordinary repeat guard stops the pool", || async {
        recovery(&world, thread).await.state == "stopped"
    })
    .await;
    assert_eq!(world.turn_starts(thread).len(), 2);
    assert_eq!(
        recovery(&world, thread).await.action.unwrap()["detail"],
        "repeat_limit"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn choice_detail_expiry_keeps_a_live_native_attempt_and_its_settled_health_receipt() {
    let world = World::new().await;
    let pool = model_pool(&world).await;
    let thread = world.thread("Retained pool attempt").await;
    use_pool(&world, pool).await;
    world.set_system("pools.choice_detail_days", json!(1)).await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Make the change"),
        )
        .await
        .unwrap();
    let original = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(original.turn_id)).await;
    failed(&world, thread, original.turn_id, "provider_busy").await;
    let picked = world.turn_start(thread, 1).await;
    world.say(thread, turn_began(picked.turn_id)).await;
    let choice: Id = world
        .scalar(
            "SELECT DISTINCT choice_id FROM pool_attempt_scopes WHERE attempt_id=$1",
            picked.turn_id,
        )
        .await;
    sqlx::query("UPDATE pool_choices SET created_at=$2,updated_at=$2 WHERE id=$1")
        .bind(choice)
        .bind(world.clock.now() - chrono::Duration::days(2))
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().run_retention().await.unwrap();
    assert!(
        world
            .count(
                "SELECT count(*) FROM pool_choice_members WHERE choice_id=$1",
                choice
            )
            .await
            > 0,
        "the live native attempt retains its choice details"
    );
    world
        .say(
            thread,
            common::turn_ended(picked.turn_id, TurnOutcome::Completed),
        )
        .await;
    world.wait_turn(picked.turn_id, "completed").await;
    eventually("the native health receipt settles", || async {
        world.count("SELECT count(*) FROM pool_attempt_scopes WHERE attempt_id=$1 AND settled_at IS NULL", picked.turn_id).await == 0
    }).await;
    let receipt: Json = world.scalar("SELECT jsonb_agg(to_jsonb(a) ORDER BY scope_id) FROM pool_attempt_scopes a WHERE attempt_id=$1", picked.turn_id).await;
    assert!(receipt.as_array().is_some_and(|rows| !rows.is_empty()));
    world.engine().run_retention().await.unwrap();
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM pool_choice_members WHERE choice_id=$1",
                choice
            )
            .await,
        0
    );
    assert_eq!(world.scalar::<Json>("SELECT jsonb_agg(to_jsonb(a) ORDER BY scope_id) FROM pool_attempt_scopes a WHERE attempt_id=$1", picked.turn_id).await, receipt);
    assert_eq!(world.count("SELECT count(*) FROM pool_choices WHERE id=$1 AND state='selected' AND leaf IS NOT NULL AND path IS NOT NULL", choice).await, 1);
    assert_eq!(
        world.turn_starts(thread).len(),
        2,
        "a retention read cannot start another paid turn"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabling_a_pool_stops_its_admitted_turn_without_stopping_a_direct_user_of_the_same_model()
{
    use orch_api_contract::resource_facts::FactTarget;
    use orch_engine::resource_pause::ResourceStop;
    let world = World::new().await;
    let pool = model_pool(&world).await;
    let thread = world.thread("Work through the pool").await;
    use_pool(&world, pool).await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Make the change"),
        )
        .await
        .unwrap();
    let original = world.turn_start(thread, 0).await;
    world.say(thread, turn_began(original.turn_id)).await;
    failed(&world, thread, original.turn_id, "provider_busy").await;
    let picked = world.turn_start(thread, 1).await;
    world.say(thread, turn_began(picked.turn_id)).await;
    let direct = world.thread("Direct work on the same member").await;
    world.set(direct, "agent.model", json!(picked.model)).await;
    world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            direct,
            queued("Keep working directly"),
        )
        .await
        .unwrap();
    let direct_turn = world.turn_start(direct, 0).await;
    assert_eq!(direct_turn.model, picked.model);
    world.say(direct, turn_began(direct_turn.turn_id)).await;
    world.wait_turn(picked.turn_id, "running").await;
    world.wait_turn(direct_turn.turn_id, "running").await;
    let mut tx = world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE pools SET definition=jsonb_set(definition,'{enabled}','false') WHERE id=$1",
    )
    .bind(pool)
    .execute(&mut *tx)
    .await
    .unwrap();
    let (job, finished) = world
        .engine()
        .stop_resource_in(
            &mut tx,
            ResourceStop {
                target: FactTarget {
                    object: ObjectRef {
                        kind: ObjectKind::Pool,
                        id: pool,
                    },
                    model_id: None,
                },
                name: "Recovery models",
                by: world.ada,
                note: None,
                deleted: false,
            },
            world.clock.now(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for thread in finished.wakes {
        world.engine().wake(thread);
    }
    world.engine().stop_resource_now(job.id).await.unwrap();
    eventually("only the pooled turn receives the accepted resource interrupt", || async {
        world.count("SELECT count(*) FROM turns WHERE id=$1 AND interrupt_cause='resource_disabled'", picked.turn_id).await == 1
    }).await;
    let direct_state: (String, Option<String>) =
        sqlx::query_as("SELECT state,interrupt_cause FROM turns WHERE id=$1")
            .bind(direct_turn.turn_id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(direct_state, ("running".to_owned(), None));
    assert_eq!(
        world
            .count(
                "SELECT count(*) FROM providers WHERE id=$1 AND status='ready'",
                world.provider
            )
            .await,
        1
    );
    let plan: Json = world
        .scalar("SELECT plan FROM jobs WHERE id=$1", job.id)
        .await;
    assert_eq!(plan["quiesce"]["turns"], json!([[thread, picked.turn_id]]));
    for resource in ["harnesses", "keepers", "units"] {
        assert_eq!(
            plan["quiesce"][resource],
            json!([]),
            "a pool cannot stop shared member resources"
        );
    }
}
