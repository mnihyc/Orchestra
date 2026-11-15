//! The real turn writer and recovery cursor own every pool attempt, including after restart.
use super::*;
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_authz::ownership::{NewAccess, start_resource};

async fn model_pool(world: &World) -> Id {
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
        sqlx::query("INSERT INTO pool_members(pool_id,id,position,rule) VALUES($1,$2,$3,$4)")
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
