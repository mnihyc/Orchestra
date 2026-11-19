//! A caller's engine decision commits before machine work and rolls back without work when it cannot be recorded.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use async_trait::async_trait;
use common::World;
use orch_authz::{Caller, Principal};
use orch_db::audit::Origin;
use orch_engine::commands::{Commands, CommandsError, Info, Outcome, Outgoing};
use orch_engine::environments::StopReason;
use orch_engine::{Engine, EngineError, EngineParts};
use orch_link_contract::commands::{CommandKind, CommandResult, ProviderRefreshResult};
use orch_types::{Id, Time};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Counts only work attempted by the engine whose caller transaction is under test.
struct DecisionCommands {
    commands: Arc<dyn Commands>,
    attempted: AtomicUsize,
}

#[async_trait]
impl Commands for DecisionCommands {
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        self.attempted.fetch_add(1, Ordering::SeqCst);
        self.commands.issue(command).await
    }

    async fn observe(&self, command: Outgoing) -> Result<Outcome, CommandsError> {
        self.attempted.fetch_add(1, Ordering::SeqCst);
        self.commands.observe(command).await
    }

    async fn retained_body(&self, id: Id) -> Result<Option<orch_types::Json>, CommandsError> {
        self.commands.retained_body(id).await
    }
    async fn retained_command(
        &self,
        id: Id,
    ) -> Result<Option<orch_engine::commands::RetainedCommand>, CommandsError> {
        self.commands.retained_command(id).await
    }

    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        self.commands.status(id).await
    }

    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        self.commands.wait(id).await
    }

    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        self.attempted.fetch_add(1, Ordering::SeqCst);
        self.commands.cancel(id).await
    }
}

/// A host declares the driver of the world's template with units that are started and stopped from here, so
/// that the world's environment is one whose stop is decided here rather than a directory on a machine.
async fn hosted_with_units(world: &World) {
    let mut conn = world.pool.acquire().await.unwrap();
    let machine =
        orch_db::queries::resources::testing::insert_machine(&mut conn, "node", "managed")
            .await
            .unwrap();
    let host = orch_types::new_id();
    orch_db::queries::resources::insert_host(
        &mut conn,
        &orch_db::queries::resources::NewHost {
            id: host,
            machine_id: machine,
            owner_id: Some(world.ada),
            home_id: Some(world.root),
            name: "node",
        },
        world.clock.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE hosts SET drivers = $2 WHERE id = $1")
        .bind(host)
        .bind(json!({ "test": { "name": "test", "title": "Test", "lifecycle": "unit" } }))
        .execute(&mut *conn)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_failed_engine_decision_rolls_back_and_sends_no_command() {
    let world = World::new().await;
    // PostgreSQL refuses this audit metadata at the same accepted-decision writer.
    let origin = Origin {
        request_id: Some("\0".to_owned()),
        ..Origin::default()
    };
    let db = world.pool.clone();
    let commands = Arc::new(DecisionCommands {
        commands: world.commands.as_engine(),
        attempted: AtomicUsize::new(0),
    });
    // The world's supervisor independently attaches its connected directory. These decisions
    // use a separate, unstarted engine so that the assertion observes their own transport.
    let engine = Engine::new(EngineParts {
        db: db.clone(),
        clock: world.clock.clone(),
        config: common::config(),
        commands: commands.clone(),
        access: world.access.clone(),
        live: Arc::new(orch_engine::live::Discard),
    });
    for operation in [
        "refresh",
        "refresh_retry",
        "restart",
        "logout",
        "start",
        "stop",
        "cancel",
        "return",
        "stop_environment",
    ] {
        if operation == "stop_environment" {
            hosted_with_units(&world).await;
        }
        let mut tx = orch_db::queries::transaction_guard::scope(true, db.begin())
            .await
            .unwrap();
        let principal = Principal::load(
            &mut tx,
            Caller::session(world.ada),
            Time::new(world.clock.now()),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE providers SET name = 'Should roll back' WHERE id = $1")
            .bind(world.provider)
            .execute(&mut *tx)
            .await
            .unwrap();
        let error = match operation {
            "refresh" => engine
                .refresh_provider_in(tx, &principal, &origin, world.provider)
                .await
                .unwrap_err(),
            "refresh_retry" => engine
                .refresh_provider_for_retry_in(tx, &principal, &origin, world.provider)
                .await
                .unwrap_err(),
            "restart" => engine
                .restart_harness_in(tx, &principal, &origin, world.provider, world.agent)
                .await
                .unwrap_err(),
            "logout" => engine
                .sign_out_in(tx, &principal, &origin, world.provider)
                .await
                .unwrap_err(),
            "cancel" => engine
                .cancel_sign_in_in(tx, &principal, &origin, world.provider)
                .await
                .unwrap_err(),
            "start" => match engine
                .start_provider_runtime_in(tx, &principal, &origin, world.provider)
                .await
                .unwrap_err()
            {
                orch_engine::provider_runtime::RuntimeError::Internal(error) => error,
                error => panic!("the decision must fail first: {error}"),
            },
            "stop" => match engine
                .stop_provider_runtime_in(tx, &principal, &origin, world.provider, false)
                .await
                .unwrap_err()
            {
                orch_engine::provider_runtime::RuntimeError::Internal(error) => error,
                error => panic!("the decision must fail first: {error}"),
            },
            "return" => match engine
                .return_credential_in(tx, &principal, &origin, world.provider, false, true)
                .await
                .unwrap_err()
            {
                orch_engine::credentials::handover::ReturnError::Internal(error) => error,
                error => panic!("the decision must fail first: {error}"),
            },
            "stop_environment" => match engine
                .stop_environment_in(
                    tx,
                    &principal,
                    &origin,
                    world.environment,
                    false,
                    StopReason::Person(world.ada),
                )
                .await
                .unwrap_err()
            {
                orch_engine::environments::StopError::Internal(error) => error,
                error => panic!("the decision must fail first: {error}"),
            },
            _ => unreachable!(),
        };
        assert!(matches!(error, EngineError::Db(_)), "{operation}: {error}");
        let name: String = sqlx::query_scalar("SELECT name FROM providers WHERE id = $1")
            .bind(world.provider)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
        assert_eq!(
            name, "Fake subscription",
            "{operation} rolls back its caller's uncommitted work"
        );
    }
    assert_eq!(commands.attempted.load(Ordering::SeqCst), 0);
    let decisions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE details->>'decision' = 'accepted'",
    )
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(decisions, 0);
}

struct AfterCommit {
    pool: sqlx::PgPool,
    provider: Id,
    commands: Arc<dyn Commands>,
}

#[async_trait]
impl Commands for AfterCommit {
    async fn retained_body(&self, id: Id) -> Result<Option<orch_types::Json>, CommandsError> {
        self.commands.retained_body(id).await
    }
    async fn retained_command(
        &self,
        id: Id,
    ) -> Result<Option<orch_engine::commands::RetainedCommand>, CommandsError> {
        self.commands.retained_command(id).await
    }
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        let committed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM providers p JOIN audit_log a
            ON a.object_id = p.id WHERE p.id = $1 AND p.name = 'Authorized' AND a.details->>'decision' = 'accepted')")
            .bind(self.provider).fetch_one(&self.pool).await.unwrap();
        assert!(
            committed,
            "another connection sees both the caller's write and the decision before a command"
        );
        self.commands.issue(command).await
    }

    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        self.commands.status(id).await
    }

    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        self.commands.wait(id).await
    }

    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        self.commands.cancel(id).await
    }
}

#[tokio::test]
async fn a_refresh_commits_the_callers_transaction_before_a_machine_receives_work() {
    let world = World::new().await;
    world.machine.handler.on(CommandKind::ProviderRefresh, |_| {
        Ok(CommandResult::ProviderRefresh(ProviderRefreshResult {
            source: None,
            status: "ready".to_owned(),
            facts: serde_json::from_value(json!({"plan":"confirmed","windows":[]})).unwrap(),
        }))
    });
    let db = world.pool.clone();
    let engine = Engine::new(EngineParts {
        db: db.clone(),
        clock: world.clock.clone(),
        config: common::config(),
        commands: Arc::new(AfterCommit {
            pool: world.pool.pool().clone(),
            provider: world.provider,
            commands: world.commands.as_engine(),
        }),
        access: world.access.clone(),
        live: Arc::new(orch_engine::live::Discard),
    });
    let mut tx = orch_db::queries::transaction_guard::scope(true, db.begin())
        .await
        .unwrap();
    let principal = Principal::load(
        &mut tx,
        Caller::session(world.ada),
        Time::new(world.clock.now()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE providers SET name = 'Authorized' WHERE id = $1")
        .bind(world.provider)
        .execute(&mut *tx)
        .await
        .unwrap();
    let answer = engine
        .refresh_provider_in(tx, &principal, &Origin::default(), world.provider)
        .await
        .unwrap();
    assert!(matches!(
        answer,
        orch_engine::provider_setup::Refreshed::Answered { .. }
    ));
    let plan: String = sqlx::query_scalar("SELECT facts->>'plan' FROM providers WHERE id = $1")
        .bind(world.provider)
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(plan, "confirmed");
}
