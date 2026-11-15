//! Accepted choices have durable policy state; an inspector never consumes a turn.
use super::*;
use orch_authz::{Caller, Principal};
use orch_engine::resource_choice::{
    ChoiceRequest,
    database::{Database, Requirements},
};
use orch_provider_contract::pools::{Kind, Leaf, Policy, Target};

struct Eligible;
#[async_trait::async_trait]
impl Requirements for Eligible {
    async fn model(
        &mut self,
        _: &mut orch_db::Conn,
        _: Id,
        _: &str,
    ) -> Result<Leaf, orch_engine::EngineError> {
        Err(orch_engine::EngineError::unreadable(
            "this fixture selects hosts",
        ))
    }
    async fn reason(
        &mut self,
        _: &mut orch_db::Conn,
        _: &Principal,
        _: &Leaf,
        _: &[orch_engine::resource_choice::Hop],
    ) -> Result<Option<String>, orch_engine::EngineError> {
        Ok(None)
    }
}
async fn pool(world: &World, owner: &Who, root: Id, policy: Policy) -> (Id, Vec<Leaf>) {
    let id = resource(world, owner, root, "pool", "Host choices").await;
    let mut leaves = Vec::new();
    for name in ["First", "Second"] {
        leaves.push(Leaf::Host {
            id: resource(world, owner, root, "host", name).await,
        });
    }
    let mut tx = world.db.begin().await.unwrap();
    sqlx::query("UPDATE pools SET kind='host',definition=$2 WHERE id=$1").bind(id).bind(json!({"kind":"host","enabled":true,"policy":policy,"lend_members":false,"members":[],"config":{}})).execute(&mut *tx).await.unwrap();
    for (position, leaf) in leaves.iter().enumerate() {
        sqlx::query("INSERT INTO pool_members(pool_id,id,position,rule) VALUES($1,$2,$3,$4)")
            .bind(id)
            .bind(orch_types::new_id())
            .bind(i64::try_from(position).unwrap())
            .bind(json!({"type":"single","resource":leaf}))
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    (id, leaves)
}
async fn choose(
    world: &World,
    owner: &Who,
    pool: Id,
    id: Id,
) -> orch_engine::resource_choice::Choice {
    let mut tx = world.db.begin().await.unwrap();
    let principal = Principal::load(&mut tx, Caller::session(owner.id()), Time::new(world.now()))
        .await
        .unwrap();
    let choice = Database::new(&mut tx, &principal, Eligible)
        .reserve(&ChoiceRequest {
            id,
            target: Target::Pool {
                id: pool,
                kind: Kind::Host,
            },
            source: json!({"kind":"placement","request":id}),
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    choice
}
#[tokio::test]
async fn replaying_one_round_robin_choice_does_not_take_the_next_members_turn() {
    let world = World::new().await;
    let owner = person(&world, "round-robin-owner").await;
    let root = make_org(&world, &owner, "Rotation", "rotation")
        .await
        .base
        .id;
    let (pool, leaves) = pool(&world, &owner, root, Policy::RoundRobin).await;
    let id = orch_types::new_id();
    let first = choose(&world, &owner, pool, id).await;
    let replay = choose(&world, &owner, pool, id).await;
    assert_eq!(first.leaf, Some(leaves[0].clone()));
    assert_eq!(first, replay);
    let second = choose(&world, &owner, pool, orch_types::new_id()).await;
    assert_eq!(second.leaf, Some(leaves[1].clone()));
}
#[tokio::test]
async fn inspecting_members_never_advances_round_robin() {
    use orch_engine::resource_choice::Resolver;
    let world = World::new().await;
    let owner = person(&world, "previewing-rotation").await;
    let root = make_org(&world, &owner, "Preview", "preview").await.base.id;
    let (pool, leaves) = pool(&world, &owner, root, Policy::RoundRobin).await;
    for _ in 0..2 {
        let mut conn = world.conn().await;
        let principal = Principal::load(
            &mut conn,
            Caller::session(owner.id()),
            Time::new(world.now()),
        )
        .await
        .unwrap();
        let mut read = Resolver::new(
            Database::new(&mut conn, &principal, Eligible),
            &Target::Pool {
                id: pool,
                kind: Kind::Host,
            },
            Kind::Host,
        )
        .await
        .unwrap();
        while read.next().await.unwrap().is_some() {}
    }
    assert_eq!(
        choose(&world, &owner, pool, orch_types::new_id())
            .await
            .leaf,
        Some(leaves[0].clone())
    );
}
