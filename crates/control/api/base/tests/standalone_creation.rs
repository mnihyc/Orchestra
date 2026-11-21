//! Actual scoped standalone provisioning keeps its own current origin without a created Thread.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#[path = "../../../engine/tests/common/mod.rs"]
mod common;
#[path = "../../../engine/tests/hosting/mod.rs"]
mod hosting;

use orch_api_contract::{
    binding::NewEnvironment,
    environment::{EnvironmentCreate, EnvironmentCreateResponse},
};
use orch_api_core::{
    middleware::authentication::{new_api_token, token_hash},
    registry::Registry,
    testing::TestApp,
};
use orch_db::queries::identity::{self, NewApiToken};
use orch_link_contract::commands::CommandKind;
use orch_types::{Id, new_id};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn standalone_scope_provisions_the_real_unit_and_a_closed_origin_cannot_start_more_work() {
    for closed in [false, true] {
        let clock = orch_testkit::clock::manual();
        let world = common::World::with_clock(Arc::new(clock.clone())).await;
        hosting::serve(&world);
        let host = hosting::ScriptedHost::on(&world, "Scoped standalone", 32768).await;
        let template = hosting::template(&world).await;
        let mut registry = Registry::new();
        orch_api_core::routes::mount(&mut registry);
        let app = TestApp::with_config(
            orch_db::Db::from_pool(world.pool.pool().clone()),
            clock.clone(),
            registry,
            orch_config::ServerConfig::default(),
        );
        let id = new_id();
        let secret = new_api_token(id);
        let scope = json!({"rules":[
            {"target":{"kind":"org","id":world.root,"reach":"self"},"resources":["environment"],"allow":[{"operation":"EnvironmentCreate","actions":["manage"]}]},
            {"target":{"kind":"template","id":template,"reach":"self"},"resources":["template"],"allow":[{"operation":"EnvironmentCreate","actions":["read"]}]},
            {"target":{"kind":"host","id":host.id,"reach":"self"},"resources":["host"],"allow":[{"operation":"EnvironmentCreate","actions":["write"]}]}
        ]});
        identity::create_api_token_scoped(
            &mut world.pool.acquire().await.unwrap(),
            &NewApiToken {
                id,
                user_id: world.ada,
                name: "Standalone native source",
                token_hash: &token_hash(&secret),
                scopes: &[],
                node_id: None,
                at: world.clock.now(),
                expires_at: Some(world.clock.now() + chrono::Duration::hours(1)),
            },
            Some(&scope),
        )
        .await
        .unwrap();
        if closed {
            host.added
                .machine
                .handler
                .hold(CommandKind::NodeRuntimeCreate);
        }
        let reply = app
            .call(EnvironmentCreate, &[])
            .token(&secret)
            .body(&NewEnvironment {
                name: Some("Standalone native proof".into()),
                home_node_id: Some(world.root),
                template_id: Some(template),
                host_id: Some(Some(orch_provider_contract::pools::Target::host(host.id))),
                ..NewEnvironment::default()
            })
            .send()
            .await;
        assert_eq!(
            reply.status.as_u16(),
            201,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        let made = match reply.ok() {
            EnvironmentCreateResponse::Receipt(receipt) => receipt,
            EnvironmentCreateResponse::Full(_) => {
                panic!("Manage alone cannot disclose the new singleton")
            }
        };
        let stored: serde_json::Value = world
            .scalar("SELECT plan FROM jobs WHERE id=$1", made.job_id)
            .await;
        assert_eq!(stored["creator_provision"]["standalone"], true);
        assert!(stored["creator_provision"].get("thread").is_none());
        assert_eq!(
            stored["creator_provision"]["origin"]["operation"],
            "EnvironmentCreate"
        );
        if closed {
            host.wait_began(CommandKind::NodeRuntimeCreate, 1).await;
            let mut source = world.pool.begin().await.unwrap();
            let row = orch_db::queries::jobs::job(&mut source, made.job_id)
                .await
                .unwrap()
                .unwrap();
            let checkpoint = orch_engine::jobs::provision::creator_checkpoint(
                &mut source,
                &row,
                world.clock.as_ref(),
                world.access.as_ref(),
            )
            .await
            .unwrap()
            .expect("standalone creation uses the same native creator checkpoint");
            assert_eq!(checkpoint.principal().user(), world.ada);
            checkpoint.check(world.clock.as_ref()).unwrap();
            source.rollback().await.unwrap();
            sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                .bind(id)
                .bind(world.clock.now())
                .execute(world.pool.pool())
                .await
                .unwrap();
            let mut source = world.pool.begin().await.unwrap();
            let denied = orch_engine::jobs::provision::creator_checkpoint(
                &mut source,
                &row,
                world.clock.as_ref(),
                world.access.as_ref(),
            )
            .await;
            assert!(matches!(
                denied,
                Err(orch_engine::EngineError::BeforeEffect(_))
            ));
            source.rollback().await.unwrap();
            host.added.machine.handler.release();
            let (state, _, error) = hosting::wait_over(&world, made.job_id).await;
            assert_eq!(state, "failed", "{error:?}");
            assert_eq!(
                error.as_ref().and_then(|error| error["code"].as_str()),
                Some("access.denied")
            );
            assert_eq!(
                host.count(CommandKind::NodeRuntimeStart),
                0,
                "the closed origin cannot launch its next native effect"
            );
        } else {
            tokio::select! {
                () = host.wait_for(CommandKind::NodeRuntimeStart, 1) => {},
                (state, _, error) = hosting::wait_over(&world, made.job_id) => {
                    panic!("standalone provisioning ended before its native start: {state}, {error:?}");
                }
            }
            let runtime = host.started().unwrap();
            let _unit = hosting::enroll(&world, runtime).await;
            let (state, _, error) = hosting::wait_over(&world, made.job_id).await;
            assert_eq!(state, "succeeded", "{error:?}");
            let actual: Id = world
                .scalar(
                    "SELECT machine_id FROM environments WHERE id=$1",
                    made.environment_id,
                )
                .await;
            assert!(!actual.is_nil());
        }
    }
}

#[tokio::test]
async fn losing_a_host_pool_reference_cannot_turn_an_accepted_standalone_creation_into_direct_host_use()
 {
    use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
    use orch_authz::ownership::{NewAccess, start_resource};
    use orch_provider_contract::pools::{Kind, Target};
    let clock = orch_testkit::clock::manual();
    let world = common::World::with_clock(Arc::new(clock.clone())).await;
    hosting::serve(&world);
    let host = hosting::ScriptedHost::on(&world, "Retained standalone host", 32768).await;
    let template = hosting::template(&world).await;
    world.engine().stop();
    let mut conn = world.pool.acquire().await.unwrap();
    let pool = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "pool",
        Some(world.ada),
        Some(world.root),
        "Retained standalone pool",
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
        world.clock.time(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE pools SET kind='host',definition=$2 WHERE id=$1").bind(pool)
        .bind(json!({"kind":"host","enabled":true,"policy":{"type":"round_robin"},"lend_members":false,"members":[],"config":{}}))
        .execute(&mut *conn).await.unwrap();
    sqlx::query(
        "INSERT INTO pool_members(pool_id,id,position,rule,enabled) VALUES($1,$2,0,$3,true)",
    )
    .bind(pool)
    .bind(new_id())
    .bind(json!({"type":"single","resource":{"kind":"host","id":host.id}}))
    .execute(&mut *conn)
    .await
    .unwrap();
    let token = new_id();
    let secret = new_api_token(token);
    identity::create_api_token_scoped(&mut conn, &NewApiToken {
        id: token, user_id: world.ada, name: "Retained standalone source", token_hash: &token_hash(&secret), scopes: &[], node_id: None,
        at: world.clock.now(), expires_at: Some(world.clock.now() + chrono::Duration::hours(1)),
    }, Some(&json!({"rules":[{"target":{"kind":"org","id":world.root,"reach":"subtree"},"resources":["environment","template","host","pool"],"allow":[{"operation":"EnvironmentCreate","actions":["manage","read","write"]}]}]}))).await.unwrap();
    drop(conn);
    let mut registry = Registry::new();
    orch_api_core::routes::mount(&mut registry);
    let app = TestApp::with_config(
        orch_db::Db::from_pool(world.pool.pool().clone()),
        clock,
        registry,
        orch_config::ServerConfig::default(),
    );
    let made = app
        .call(EnvironmentCreate, &[])
        .token(&secret)
        .body(&NewEnvironment {
            name: Some("Retained standalone source".into()),
            home_node_id: Some(world.root),
            template_id: Some(template),
            host_id: Some(Some(Target::Pool {
                id: pool,
                kind: Kind::Host,
            })),
            ..NewEnvironment::default()
        })
        .send()
        .await
        .ok();
    let environment = match made {
        EnvironmentCreateResponse::Full(environment) => environment.base.id,
        EnvironmentCreateResponse::Receipt(receipt) => receipt.environment_id,
    };
    let job: Id = world
        .scalar(
            "SELECT id FROM jobs WHERE environment_id=$1 AND kind='environment_provision'",
            environment,
        )
        .await;
    let original = orch_db::queries::jobs::job(&mut world.pool.acquire().await.unwrap(), job)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original.plan["pool_choice"], json!(environment));
    for remove_required_marker in [false, true] {
        sqlx::query("UPDATE jobs SET plan=$2 WHERE id=$1")
            .bind(job)
            .bind(&original.plan)
            .execute(world.pool.pool())
            .await
            .unwrap();
        let mut tx = world.pool.begin().await.unwrap();
        let row = orch_db::queries::jobs::job(&mut tx, job)
            .await
            .unwrap()
            .unwrap();
        let before = orch_engine::jobs::provision::creator_checkpoint(
            &mut tx,
            &row,
            world.clock.as_ref(),
            world.access.as_ref(),
        )
        .await
        .unwrap()
        .expect("the real accepted creator source is live");
        before.check(world.clock.as_ref()).unwrap();
        tx.rollback().await.unwrap();
        let mut broken = original.plan.clone();
        broken.as_object_mut().unwrap().remove("pool_choice");
        if remove_required_marker {
            broken
                .as_object_mut()
                .unwrap()
                .remove("pool_choice_required");
        }
        sqlx::query("UPDATE jobs SET plan=$2 WHERE id=$1")
            .bind(job)
            .bind(broken)
            .execute(world.pool.pool())
            .await
            .unwrap();
        let mut tx = world.pool.begin().await.unwrap();
        let row = orch_db::queries::jobs::job(&mut tx, job)
            .await
            .unwrap()
            .unwrap();
        let closed = orch_engine::jobs::provision::creator_checkpoint(
            &mut tx,
            &row,
            world.clock.as_ref(),
            world.access.as_ref(),
        )
        .await;
        assert!(
            closed.is_err(),
            "missing pool evidence cannot borrow direct Host Use; required marker removed: {remove_required_marker}"
        );
        tx.rollback().await.unwrap();
    }
    assert_eq!(host.count(CommandKind::NodeRuntimeCreate), 0);
}

/// Answer the command reconstructed by the API, including exactly the headers sent to the host.
struct QueuedHost(Arc<orch_testkit::doubles::machine::ScriptedHandler>);

#[async_trait::async_trait]
impl orch_api_core::testing::Machine for QueuedHost {
    async fn run(
        &self,
        cmd: &orch_link_contract::commands::Cmd,
    ) -> Result<orch_types::Json, orch_link_contract::commands::CommandError> {
        use orch_link::commands::{CommandContext, CommandHandler};
        let context = CommandContext {
            id: cmd.id,
            key: cmd.key.clone(),
            deadline: cmd.deadline,
            cancel: Default::default(),
            native_target: cmd.native_target.clone(),
            native_members: cmd.native_members.clone(),
            native_revision: cmd.native_revision,
            authority_deadline: None,
            native_fences: None,
        };
        self.0
            .handle(context, cmd.command().unwrap())
            .await
            .map(|reply| reply.body().unwrap())
    }
}

#[tokio::test]
async fn managed_create_persists_the_receipt_from_the_dispatched_host_command() {
    let world = common::World::new().await;
    let app = TestApp::new(
        world.pool.clone(),
        orch_types::ManualClock::new(world.clock.now()),
        Registry::new(),
    );
    world
        .restart_api_configured(
            |_| {
                Arc::new(orch_api_core::thread_engine::DispatchCommands::new(
                    app.app.link.dispatcher(),
                ))
            },
            |engine| engine.attach_host_services(Arc::new(hosting::Services)),
        )
        .await;
    let host = hosting::ScriptedHost::on(&world, "Managed create host", 32768).await;
    let _played = app.play(
        host.machine,
        Arc::new(QueuedHost(host.added.machine.handler.clone())),
    );
    let template = hosting::template(&world).await;
    let made = hosting::make_with(
        &world,
        template,
        "Managed create",
        Some(host.id),
        &json!({}),
    )
    .await;
    let runtime = hosting::runtime_started_for_job(&world, &host, made.job.id).await;
    let bound: bool = world
        .scalar(
            "SELECT p.outcome #>> '{source,incarnation}' = p.operands->>'opening'
         FROM runtime_control_phases p JOIN commands c ON c.id=p.id
         WHERE c.kind='node.runtime.create' AND c.body->>'runtime_id'=$1::uuid::text",
            runtime,
        )
        .await;
    assert!(
        bound,
        "the persisted create receipt must identify the opening admitted for this job"
    );
}
