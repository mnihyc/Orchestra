//! Accepted choices have durable policy state; an inspector never consumes a turn.
use super::*;
use orch_authz::{Caller, Principal};
use orch_engine::resource_choice::{
    ChoiceRequest,
    database::{Database, Requirements},
};
use orch_provider_contract::pools::{Kind, Leaf, Policy, Target};

#[path = "history.rs"]
mod history;

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
    let mut tx = world.db.pool.begin().await.unwrap();
    sqlx::query("UPDATE pools SET kind='host',definition=$2 WHERE id=$1").bind(id).bind(json!({"kind":"host","enabled":true,"policy":policy,"lend_members":false,"members":[],"config":{}})).execute(&mut *tx).await.unwrap();
    for (position, leaf) in leaves.iter().enumerate() {
        sqlx::query(
            "INSERT INTO pool_members(pool_id,id,position,rule,enabled) VALUES($1,$2,$3,$4,true)",
        )
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
    let mut tx = world.db.pool.begin().await.unwrap();
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

struct Priced {
    known: Id,
}
#[async_trait::async_trait]
impl Requirements for Priced {
    async fn model(
        &mut self,
        _: &mut orch_db::Conn,
        _: Id,
        _: &str,
    ) -> Result<Leaf, orch_engine::EngineError> {
        Err(orch_engine::EngineError::unreadable(
            "this fixture retains a provider requirement",
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
    async fn measurements(
        &mut self,
        conn: &mut orch_db::Conn,
        _: &Principal,
        leaf: &Leaf,
        _: &[orch_engine::resource_choice::Hop],
    ) -> Result<orch_engine::resource_choice::Measurements, orch_engine::EngineError> {
        use orch_engine::{rates::RateBook, resource_choice::Quote};
        let card = if matches!(leaf,Leaf::Provider{id}if *id==self.known) {
            vec![(
                "Example".into(),
                serde_json::from_value(
                    json!({"input_per_mtok":"2","cached_input_per_mtok":"1","output_per_mtok":"5","tiers":{"fast":"2"},"above":[{"context_tokens":10,"input_per_mtok":"3","output_per_mtok":"7"}]}),
                )?,
            )]
        } else {
            vec![]
        };
        let tokens = orch_db::usage::ledger::Tokens {
            input: 20,
            output: 10,
            total: 30,
            ..Default::default()
        };
        let priced = RateBook::with_card(card)
            .price(conn, None, "Example", &tokens, Some(20), Some("fast"))
            .await?;
        let quote = Quote::from_priced(priced).map(|mut quote| {
            quote.basis["estimate"] = json!("uncached_request");
            quote.basis["input_tokens"] = json!(tokens.input);
            quote
        });
        Ok(orch_engine::resource_choice::Measurements {
            quote,
            ..Default::default()
        })
    }
}
#[tokio::test]
async fn least_cost_prices_each_eligible_provider_once_and_never_treats_an_unknown_price_as_free() {
    let world = World::new().await;
    let owner = person(&world, "priced-pool-owner").await;
    let root = make_org(&world, &owner, "Request cost", "request-cost")
        .await
        .base
        .id;
    let pool = resource(&world, &owner, root, "pool", "Priced providers").await;
    let unknown = resource(&world, &owner, root, "provider", "Unknown price").await;
    let known = resource(&world, &owner, root, "provider", "Known price").await;
    let child = resource(&world, &owner, root, "pool", "Nested providers").await;
    let mut tx = world.db.pool.begin().await.unwrap();
    sqlx::query("UPDATE pools SET kind='provider',definition=$2 WHERE id=$1").bind(pool).bind(json!({"kind":"provider","enabled":true,"policy":{"type":"least_cost"},"lend_members":false,"members":[],"config":{}})).execute(&mut *tx).await.unwrap();
    for (position, id) in [unknown, known].iter().enumerate() {
        sqlx::query(
            "INSERT INTO pool_members(pool_id,id,position,rule,enabled) VALUES($1,$2,$3,$4,true)",
        )
        .bind(pool)
        .bind(orch_types::new_id())
        .bind(i64::try_from(position).unwrap())
        .bind(json!({"type":"single","resource":{"kind":"provider","id":id}}))
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    sqlx::query("UPDATE pools SET kind='provider',definition=$2 WHERE id=$1")
        .bind(child)
        .bind(json!({"kind":"provider","enabled":true,"policy":{"type":"ordered"},"lend_members":false,"members":[],"config":{}}))
        .execute(&mut *tx)
        .await
        .unwrap();
    for (parent, position, rule) in [
        (pool, 2, json!({"type":"pool","kind":"provider","id":child})),
        (
            child,
            0,
            json!({"type":"single","resource":{"kind":"provider","id":known}}),
        ),
    ] {
        sqlx::query(
            "INSERT INTO pool_members(pool_id,id,position,rule,enabled) VALUES($1,$2,$3,$4,true)",
        )
        .bind(parent)
        .bind(orch_types::new_id())
        .bind(i64::from(position))
        .bind(rule)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    let principal = Principal::load(&mut tx, Caller::session(owner.id()), Time::new(world.now()))
        .await
        .unwrap();
    let id = orch_types::new_id();
    let choice = Database::new(&mut tx, &principal, Priced { known })
        .reserve(&ChoiceRequest {
            id,
            target: Target::Pool {
                id: pool,
                kind: Kind::Provider,
            },
            source: json!({"kind":"generation","request":id}),
        })
        .await
        .unwrap();
    assert_eq!(choice.leaf, Some(Leaf::Provider { id: known }));
    let prices: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT reason,cost::text FROM pool_choice_members WHERE choice_id=$1 AND leaf->>'id'=$2 ORDER BY ordinal",
    )
    .bind(id)
    .bind(known.to_string())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        prices,
        vec![
            (None, Some("0.000260".into())),
            (Some("duplicate_member".into()), None)
        ],
        "the skipped second path must not carry a price into choice history"
    );
    let read_id: Id = sqlx::query_scalar("SELECT read_id FROM pool_choices WHERE id=$1")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    use orch_api_contract::pools::{PoolChoiceDetailsGet, PoolChoiceDetailsQuery};
    let pool_only = key(
        &world,
        &owner,
        vec![rule(
            "PoolChoiceDetailsGet",
            "pool",
            pool,
            &["pool"],
            "read",
        )],
    )
    .await;
    for (token, can_read_member) in [(&owner.token, true), (&pool_only, false)] {
        let details = world
            .app
            .call(PoolChoiceDetailsGet, &[&pool, &read_id])
            .token(token)
            .query(&PoolChoiceDetailsQuery::default())
            .send()
            .await
            .ok();
        assert_eq!(details.items.len(), 3);
        assert!(details.next.is_none());
        assert_eq!(
            details
                .items
                .iter()
                .filter(|row| row.cost.is_some())
                .count(),
            usize::from(can_read_member)
        );
        for row in details.items {
            let basis = serde_json::to_value(&row).unwrap()["cost_basis"].clone();
            if can_read_member
                && row.reason.is_none()
                && row.resource == Some(Leaf::Provider { id: known })
            {
                assert_eq!(basis, json!({"source":"rate_card","uncached_input":true}));
            } else {
                assert!(row.cost.is_none() && basis.is_null());
            }
        }
    }
}

#[tokio::test]
async fn an_idle_environment_reports_zero_running_threads_without_inventing_machine_load() {
    let world = World::new().await;
    let owner = person(&world, "environment-load-owner").await;
    let root = make_org(&world, &owner, "Environment load", "environment-load")
        .await
        .base
        .id;
    let id = resource(&world, &owner, root, "environment", "Idle environment").await;
    let mut conn = world.conn().await;
    let now = Time::new(world.now());
    let principal = Principal::load(&mut conn, Caller::session(owner.id()), now)
        .await
        .unwrap();
    let measured = Eligible
        .measurements(&mut conn, &principal, &Leaf::Environment { id }, &[])
        .await
        .unwrap();
    assert_eq!(measured.loads, json!({"threads":{"score":0.0,"at":now}}));
    let missing = Eligible
        .measurements(
            &mut conn,
            &principal,
            &Leaf::Environment {
                id: orch_types::new_id(),
            },
            &[],
        )
        .await
        .unwrap();
    assert!(missing.loads.get("threads").is_none());
}

#[tokio::test]
async fn only_a_fresh_sample_from_the_environments_current_machine_session_and_attachment_ranks_load()
 {
    use orch_db::queries::{environments, resources};
    use orch_link_contract::events::EnvironmentSample;
    let world = World::new().await;
    let owner = person(&world, "attached-environment-owner").await;
    let home = make_org(
        &world,
        &owner,
        "Attached environment",
        "attached-environment",
    )
    .await
    .base
    .id;
    let environment = resource(&world, &owner, home, "environment", "Attached").await;
    let machine = resource(&world, &owner, home, "machine", "Reporting machine").await;
    let mut conn = world.conn().await;
    sqlx::query("UPDATE environments SET machine_id=$2,generation=1,status='running' WHERE id=$1")
        .bind(environment)
        .bind(machine)
        .execute(&mut *conn)
        .await
        .unwrap();
    let first = new_id();
    let second = new_id();
    let at = Time::new(world.now());
    resources::set_session_presence(&mut conn, machine, true, None, Some(first), at.get())
        .await
        .unwrap();
    let mut reported: EnvironmentSample = serde_json::from_value(json!({
        "generation":1,"cpu_interval_started_at":at, "sample":{"at":at,"cpu_percent":0.0,"cpu_limit_millis":2000,
        "memory_bytes":256,"memory_limit_bytes":1024,"disk_used_bytes":null,"disk_limit_bytes":null,
        "net_rx_bps":0,"net_tx_bps":0,"pids":3}
    }))
    .unwrap();
    let principal = Principal::load(&mut conn, Caller::session(owner.id()), at)
        .await
        .unwrap();
    let leaf = Leaf::Environment { id: environment };
    let save = environments::record_load_sample;
    assert!(
        save(&mut conn, environment, machine, first, &reported, at, 60)
            .await
            .unwrap()
    );
    let read = Eligible
        .measurements(&mut conn, &principal, &leaf, &[])
        .await
        .unwrap();
    assert_eq!(
        read.loads,
        json!({"cpu":{"score":0.0,"at":at},"memory":{"score":25.0,"at":at},
        "combined":{"score":25.0,"at":at},"processes":{"score":3.0,"at":at},"threads":{"score":0.0,"at":at}})
    );

    world.advance(chrono::Duration::seconds(1));
    let later = Time::new(world.now());
    let principal = Principal::load(&mut conn, Caller::session(owner.id()), later)
        .await
        .unwrap();
    resources::set_session_presence(&mut conn, machine, true, None, Some(second), later.get())
        .await
        .unwrap();
    let no_sample = json!({"threads":{"score":0.0,"at":later}});
    assert_eq!(
        Eligible
            .measurements(&mut conn, &principal, &leaf, &[])
            .await
            .unwrap()
            .loads,
        no_sample
    );
    reported.sample.at = later;
    assert!(
        !save(&mut conn, environment, machine, first, &reported, later, 60)
            .await
            .unwrap()
    );
    assert!(
        save(
            &mut conn,
            environment,
            machine,
            second,
            &reported,
            later,
            60
        )
        .await
        .unwrap()
    );

    let after_reconnect = Eligible
        .measurements(&mut conn, &principal, &leaf, &[])
        .await
        .unwrap();
    assert_eq!(
        after_reconnect.loads,
        json!({"memory":{"score":25.0,"at":later},
        "processes":{"score":3.0,"at":later},"threads":{"score":0.0,"at":later}}),
        "a CPU interval starting on the disconnected session cannot rank the new session"
    );

    sqlx::query("UPDATE environments SET generation=2 WHERE id=$1")
        .bind(environment)
        .execute(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        Eligible
            .measurements(&mut conn, &principal, &leaf, &[])
            .await
            .unwrap()
            .loads,
        no_sample
    );
    assert!(
        !save(
            &mut conn,
            environment,
            machine,
            second,
            &reported,
            later,
            60
        )
        .await
        .unwrap()
    );
    reported.generation = 2;
    reported.cpu_interval_started_at = None;
    assert!(
        save(
            &mut conn,
            environment,
            machine,
            second,
            &reported,
            later,
            60
        )
        .await
        .unwrap()
    );
    let baseline = Eligible
        .measurements(&mut conn, &principal, &leaf, &[])
        .await
        .unwrap();
    assert_eq!(
        baseline.loads,
        json!({"memory":{"score":25.0,"at":later},
        "processes":{"score":3.0,"at":later},"threads":{"score":0.0,"at":later}})
    );
    reported.sample.at = at;
    assert!(
        !save(
            &mut conn,
            environment,
            machine,
            second,
            &reported,
            later,
            60
        )
        .await
        .unwrap()
    );
    assert_eq!(
        Eligible
            .measurements(&mut conn, &principal, &leaf, &[])
            .await
            .unwrap()
            .loads["processes"]["at"],
        json!(later)
    );
    world.advance(chrono::Duration::seconds(61));
    let expired = Time::new(world.now());
    let principal = Principal::load(&mut conn, Caller::session(owner.id()), expired)
        .await
        .unwrap();
    assert_eq!(
        Eligible
            .measurements(&mut conn, &principal, &leaf, &[])
            .await
            .unwrap()
            .loads,
        json!({"threads":{"score":0.0,"at":expired}})
    );
}

#[tokio::test]
async fn an_idle_host_shows_its_placement_thread_count_before_a_machine_sample_arrives() {
    let world = World::new().await;
    let owner = person(&world, "host-thread-owner").await;
    let root = make_org(&world, &owner, "Host activity", "host-activity")
        .await
        .base
        .id;
    let host = resource(&world, &owner, root, "host", "Idle host").await;
    let mut conn = world.conn().await;
    let now = Time::new(world.now());
    let principal = Principal::load(&mut conn, Caller::session(owner.id()), now)
        .await
        .unwrap();
    let read = Eligible
        .measurements(&mut conn, &principal, &Leaf::Host { id: host }, &[])
        .await
        .unwrap();
    assert_eq!(read.loads, json!({"threads":{"score":0.0,"at":now}}));
    let missing = Eligible
        .measurements(
            &mut conn,
            &principal,
            &Leaf::Host {
                id: orch_types::new_id(),
            },
            &[],
        )
        .await
        .unwrap();
    assert_eq!(missing.loads, json!({}));
}

#[tokio::test]
async fn an_observed_zero_load_is_ready_while_an_unreported_load_is_not_zero() {
    use orch_provider_contract::pools::LoadMeasure;
    let world = World::new().await;
    let owner = person(&world, "measured-pool-owner").await;
    let root = make_org(&world, &owner, "Measured hosts", "measured-hosts")
        .await
        .base
        .id;
    let (pool, leaves) = pool(
        &world,
        &owner,
        root,
        Policy::LeastLoaded {
            measure: LoadMeasure::Cpu,
        },
    )
    .await;
    let Leaf::Host { id: known } = leaves[1] else {
        panic!("expected a host fixture");
    };
    let mut tx = world.db.pool.begin().await.unwrap();
    sqlx::query("UPDATE hosts SET capacity=$2,load=$3,load_at=$4 WHERE id=$1")
        .bind(known)
        .bind(json!({"cpu_millis":1000,"memory_mb":1000}))
        .bind(json!({"cpu_percent":0,"memory_used_mb":0}))
        .bind(world.now())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
    )
    .bind(known)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        choose(&world, &owner, pool, orch_types::new_id())
            .await
            .leaf,
        Some(Leaf::Host { id: known })
    );
    // A reconnect invalidates the previous session's sample, even when it reported zero.
    world.advance(chrono::Duration::seconds(1));
    sqlx::query(
        "UPDATE machines SET session_started_at=$2 WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
    )
    .bind(known)
    .bind(world.now())
    .execute(&world.db.pool)
    .await
    .unwrap();
    assert!(
        choose(&world, &owner, pool, orch_types::new_id())
            .await
            .leaf
            .is_none()
    );
    sqlx::query("UPDATE hosts SET load_at=$2 WHERE id=$1")
        .bind(known)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        choose(&world, &owner, pool, orch_types::new_id())
            .await
            .leaf,
        Some(Leaf::Host { id: known })
    );
}

#[tokio::test]
async fn two_workers_replaying_one_weighted_choice_keep_the_same_member() {
    let world = World::new().await;
    let owner = person(&world, "concurrent-choice-owner").await;
    let root = make_org(&world, &owner, "Concurrent choices", "concurrent-choices")
        .await
        .base
        .id;
    let (pool, _) = pool(&world, &owner, root, Policy::Weighted).await;
    let id = orch_types::new_id();
    let (first, second) = tokio::join!(
        choose(&world, &owner, pool, id),
        choose(&world, &owner, pool, id)
    );
    assert_eq!(first, second);
    let mut conn = world.conn().await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pool_choices WHERE id=$1")
            .bind(id)
            .fetch_one(&mut *conn)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn an_outer_load_policy_preserves_the_inner_pools_written_choice() {
    use orch_provider_contract::pools::LoadMeasure;
    let world = World::new().await;
    let owner = person(&world, "nested-policy-owner").await;
    let root = make_org(&world, &owner, "Nested policies", "nested-policies")
        .await
        .base
        .id;
    let (inner, leaves) = pool(&world, &owner, root, Policy::Ordered).await;
    let outer = resource(&world, &owner, root, "pool", "Measured choices").await;
    let third = resource(&world, &owner, root, "host", "Middle load").await;
    let mut tx = world.db.pool.begin().await.unwrap();
    sqlx::query("UPDATE pools SET kind='host',definition=$2 WHERE id=$1").bind(outer).bind(json!({"kind":"host","enabled":true,"policy":Policy::LeastLoaded { measure: LoadMeasure::Cpu },"lend_members":false,"members":[],"config":{}})).execute(&mut *tx).await.unwrap();
    for (position, rule) in [
        json!({"type":"pool","id":inner,"kind":"host"}),
        json!({"type":"single","resource":{"kind":"host","id":third}}),
    ]
    .iter()
    .enumerate()
    {
        sqlx::query(
            "INSERT INTO pool_members(pool_id,id,position,rule,enabled) VALUES($1,$2,$3,$4,true)",
        )
        .bind(outer)
        .bind(orch_types::new_id())
        .bind(i64::try_from(position).unwrap())
        .bind(rule)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    let Leaf::Host { id: first } = leaves[0] else {
        panic!("expected a host")
    };
    let Leaf::Host { id: second } = leaves[1] else {
        panic!("expected a host")
    };
    for (id, cpu) in [(first, 85), (second, 20), (third, 80)] {
        sqlx::query("UPDATE hosts SET capacity=$2,load=$3,load_at=$4 WHERE id=$1")
            .bind(id)
            .bind(json!({"cpu_millis":1000}))
            .bind(json!({"cpu_percent":cpu}))
            .bind(world.now())
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE machines SET online=true WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)",
        )
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    assert_eq!(
        choose(&world, &owner, outer, orch_types::new_id())
            .await
            .leaf,
        Some(Leaf::Host { id: third })
    );
}

#[tokio::test]
async fn a_temporarily_disabled_member_loses_its_accumulated_rotation_credit() {
    let world = World::new().await;
    let owner = person(&world, "returning-member-owner").await;
    let root = make_org(&world, &owner, "Returning members", "returning-members")
        .await
        .base
        .id;
    let (pool, leaves) = pool(&world, &owner, root, Policy::RoundRobin).await;
    choose(&world, &owner, pool, orch_types::new_id()).await;
    let mut tx = world.db.pool.begin().await.unwrap();
    sqlx::query("UPDATE pool_members SET enabled=false WHERE pool_id=$1 AND rule->'resource'=$2")
        .bind(pool)
        .bind(json!(leaves[1]))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        choose(&world, &owner, pool, orch_types::new_id())
            .await
            .leaf,
        Some(leaves[0].clone())
    );
    let mut conn = world.conn().await;
    let credit: String = sqlx::query_scalar(
        "SELECT current_weight FROM pool_rotation WHERE pool_id=$1 AND identity->'leaf'=$2",
    )
    .bind(pool)
    .bind(json!(leaves[1]))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(credit, "0");
}

#[tokio::test]
async fn pool_lending_keeps_the_actual_sharing_defaults_of_each_resource_home() {
    use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
    let world = World::new().await;
    let owner = person(&world, "sharing-policy-owner").await;
    let root = make_org(&world, &owner, "Sharing policy", "sharing-policy")
        .await
        .base
        .id;
    let project = make_project(&world, &owner, root, "Different pool home")
        .await
        .base
        .id;
    let provider = resource(&world, &owner, root, "provider", "Member home").await;
    let pool = resource(&world, &owner, project, "pool", "Pool home").await;
    world
        .app
        .call(NodeSettingSet, &[&project, &"sharing.allow_holders"])
        .token(&owner.token)
        .body(&NodeSettingSetBody {
            model_choice: None,
            value: json!(false),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let mut conn = world.conn().await;
    let policies = orch_engine::access::resource_policies_among(&mut conn, &[provider, pool])
        .await
        .unwrap();
    assert!(
        policies.sharing_at(Some(root)).holders_share,
        "the shipped holder-sharing default needs no owner setup"
    );
    assert!(
        !policies.sharing_at(Some(project)).holders_share,
        "the pool home's own stricter policy remains in force"
    );
}

#[tokio::test]
async fn expired_choice_details_leave_replay_rotation_and_forever_details_intact_across_pages() {
    let world = World::new().await;
    world.set("pools.read_batch_size", json!(1)).await;
    world.set("pools.choice_detail_days", json!(1)).await;
    let owner = person(&world, "choice-retention-owner").await;
    let root = make_org(&world, &owner, "Choice retention", "choice-retention")
        .await
        .base
        .id;
    let (pool_id, leaves) = pool(&world, &owner, root, Policy::RoundRobin).await;
    let (forever, _) = pool(&world, &owner, root, Policy::RoundRobin).await;
    sqlx::query("UPDATE pools SET definition=jsonb_set(definition,'{config,pools.choice_detail_days}','0') WHERE id=$1")
        .bind(forever).execute(&world.db.pool).await.unwrap();
    let first = choose(&world, &owner, pool_id, orch_types::new_id()).await;
    let second = choose(&world, &owner, pool_id, orch_types::new_id()).await;
    let recent = choose(&world, &owner, pool_id, orch_types::new_id()).await;
    let kept = choose(&world, &owner, forever, orch_types::new_id()).await;
    let policies: Json = sqlx::query_scalar("SELECT policies FROM pool_choices WHERE id=$1")
        .bind(first.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        policies,
        json!([{"pool_id":pool_id,"revision":first.path[0].revision,"policy":{"type":"round_robin"}}])
    );
    let old = vec![first.id, second.id, kept.id];
    sqlx::query("UPDATE pool_choices SET created_at=$2,updated_at=$2 WHERE id=ANY($1)")
        .bind(&old)
        .bind(world.now() - chrono::Duration::days(2))
        .execute(&world.db.pool)
        .await
        .unwrap();
    world.app.app.engine.run_retention().await.unwrap();
    world.app.app.engine.run_retention().await.unwrap();
    let retained: Json = sqlx::query_scalar("SELECT policies FROM pool_choices WHERE id=$1")
        .bind(first.id)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        retained, policies,
        "essential selected policies outlive expansion details"
    );
    for choice in [&first, &second, &recent, &kept] {
        let detail: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM pool_choice_nodes WHERE choice_id=$1)+(SELECT count(*) FROM pool_choice_members WHERE choice_id=$1)+(SELECT count(*) FROM pool_choice_skips WHERE choice_id=$1)")
            .bind(choice.id).fetch_one(&world.db.pool).await.unwrap();
        if [first.id, second.id].contains(&choice.id) {
            assert_eq!(
                detail, 0,
                "each expired page releases its expansion details"
            );
        } else {
            assert!(detail > 0, "recent and explicitly forever details remain");
        }
        assert_eq!(
            choose(
                &world,
                &owner,
                if choice.id == kept.id {
                    forever
                } else {
                    pool_id
                },
                choice.id
            )
            .await,
            *choice
        );
    }
    assert_eq!(
        choose(&world, &owner, pool_id, orch_types::new_id())
            .await
            .leaf,
        Some(leaves[1].clone()),
        "retention and replay do not consume the next round-robin share"
    );
}
