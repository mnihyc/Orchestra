//! An initial inventory queues through the real measurement mechanism with preparation provenance.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_authz::{
    CapturedOperation, TokenGrant,
    http_origin::{Credential, HttpPreparation},
};
use orch_db::queries::{identity, jobs, link};
use orch_engine::{
    commands::CommandAuthority, jobs::lifecycle_commands::host_inventory::Preparation,
};
use orch_link_contract::commands::{Command, CommandResult};
use orch_types::{Clock, Hash, Id, ManualClock, Time, new_id};
use std::sync::Arc;

async fn source(
    world: &World,
    host: &ScriptedHost,
    selected: Id,
    operation: &str,
) -> (Id, Result<Preparation, orch_engine::EngineError>) {
    let key = new_id();
    let hash = Hash::of(key.as_bytes());
    let expires = Time::new(world.clock.now() + chrono::Duration::hours(1));
    let scope:KeyScope=serde_json::from_value(json!({"rules":[{"target":{"kind":"host","id":selected,"reach":"self"},"allow":[{"operation":operation,"actions":[if operation=="HostGet" {"read"}else{"manage"}]}]}]})).unwrap();
    assert!(scope.validate().is_empty());
    let mut tx = world.pool.begin().await.unwrap();
    identity::create_api_token_scoped(
        &mut tx,
        &identity::NewApiToken {
            id: key,
            user_id: world.ada,
            name: "Native Host preparation",
            token_hash: hash.as_bytes(),
            scopes: &[],
            node_id: None,
            at: world.clock.now(),
            expires_at: Some(expires.get()),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    let caller = Caller::token(
        world.ada,
        TokenGrant::from_stored_scope(&[], None, Some(scope), Some(expires))
            .unwrap()
            .for_token(key),
    )
    .prepare_unresolved();
    assert!(CapturedOperation::capture(&caller).is_err());
    assert!(caller.admitted_required_roles().is_none());
    let raw = HttpPreparation::capture(
        "HostDeletePreflight",
        &caller,
        Credential::Token {
            id: key,
            hash: hash.as_bytes().to_vec(),
        },
        false,
        false,
        Some(expires),
    )
    .unwrap();
    let result = Preparation::new(&mut tx, raw, host.id, host.machine, world.clock.as_ref()).await;
    tx.commit().await.unwrap();
    (key, result)
}

#[tokio::test]
async fn inventory_prefix_uses_actual_empty_body_and_one_native_measurement_without_a_complete_capture()
 {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "Prepared inventory", 16_384).await;
    let expected = orch_driver_contract::resources::ResourceInventory {
        observed_at: Time::new(world.clock.now()),
        resources: vec![],
        unavailable: vec![],
    };
    let answer = expected.clone();
    host.answer(CommandKind::NodeResourcesList, move |command| {
        assert!(matches!(command, Command::NodeResourcesList(_)));
        assert_eq!(command.body().unwrap(), json!({}));
        Ok(CommandResult::NodeResourcesList(answer.clone()))
    });
    let (key, source) = source(&world, &host, host.id, "HostDeletePreflight").await;
    let source = source.unwrap();
    let (pending, reference) = world
        .engine()
        .host_custody_inventory_prepared(&source)
        .await
        .unwrap();
    assert!(pending.is_none());
    assert!(reference.measured_at.is_none());
    let (state, _, error) = hosting::wait_over(&world, reference.job_id).await;
    assert_eq!(state, "succeeded", "{error:?}");
    let native = source.measurement().unwrap();
    let count = world
        .engine()
        .measure_with_preparation(source.request(), &native)
        .await
        .unwrap();
    assert_eq!(count.result, Some(json!(expected)));
    assert_eq!(count.reference.job_id, reference.job_id);
    assert!(count.reference.measured_at.is_some());
    assert!(
        count.source().is_none(),
        "a prefix result never supplies a complete captured business operation"
    );
    let mut conn = world.pool.acquire().await.unwrap();
    let row = jobs::job(&mut conn, reference.job_id)
        .await
        .unwrap()
        .unwrap();
    let retained = &row.plan["measurement_source"]["origin"];
    assert!(retained.get("operation").is_none());
    assert!(serde_json::from_value::<CapturedOperation>(retained.clone()).is_err());
    let unfiled: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT resource_inventory FROM hosts WHERE id=$1")
            .bind(host.id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert!(
        unfiled.is_none(),
        "a generic count does not file a host cache"
    );
    drop(conn);
    // Replanning through the host supplier files the same completed observation.
    let (completed, next) = world
        .engine()
        .host_custody_inventory_prepared(&source)
        .await
        .unwrap();
    assert_eq!(completed, Some(expected.clone()));
    assert_eq!(next.job_id, reference.job_id);
    assert_eq!(next.measured_at, count.reference.measured_at);
    let mut conn = world.pool.acquire().await.unwrap();
    let cached: serde_json::Value =
        sqlx::query_scalar("SELECT resource_inventory FROM hosts WHERE id=$1")
            .bind(host.id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(cached, json!(expected));
    assert_eq!(host.count(CommandKind::NodeResourcesList), 1);
    assert_eq!(host.count(CommandKind::NodeResourcesRemove), 0);
    // A stored result and the host cache never replace this caller's current key.
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(key)
        .bind(world.clock.now())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(
        matches!(
            world
                .engine()
                .host_custody_inventory_prepared(&source)
                .await,
            Err(orch_engine::EngineError::BeforeEffect(_))
        ),
        "a revoked original key cannot read the completed inventory through its preparation"
    );
    let measurements: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='measurement' AND plan->'subject'->>'host_id'=$1::uuid::text",
    )
    .bind(host.id)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        measurements, 1,
        "refusal does not queue another observation"
    );
    let retained_cache: serde_json::Value =
        sqlx::query_scalar("SELECT resource_inventory FROM hosts WHERE id=$1")
            .bind(host.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(retained_cache, cached, "refusal retains the accepted facts");
    assert_eq!(host.count(CommandKind::NodeResourcesList), 1);
    assert_eq!(host.count(CommandKind::NodeResourcesRemove), 0);
    world.engine().stop();
}

#[tokio::test]
async fn inventory_prefix_refuses_another_host_and_an_unrelated_read_namespace_before_queueing() {
    let world = World::new().await;
    let host = ScriptedHost::on(&world, "Scoped inventory", 16_384).await;
    for (id, operation) in [(new_id(), "HostDeletePreflight"), (host.id, "HostGet")] {
        let (_, result) = source(&world, &host, id, operation).await;
        assert!(matches!(
            result,
            Err(orch_engine::EngineError::BeforeEffect(_))
        ));
    }
    let (_, valid) = source(&world, &host, host.id, "HostDeletePreflight").await;
    let mut wrong = json!(valid.unwrap().retained());
    wrong["request_operation"] = json!("HostGet");
    let wrong: HttpPreparation = serde_json::from_value(wrong).unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    assert!(
        Preparation::new(&mut tx, wrong, host.id, host.machine, world.clock.as_ref())
            .await
            .is_err(),
        "a different raw request namespace cannot borrow the inventory supplier"
    );
    tx.rollback().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='measurement'")
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(host.count(CommandKind::NodeResourcesList), 0);
    world.engine().stop();
}

#[tokio::test]
async fn inventory_prefix_durable_issue_keeps_exact_command_raw_key_and_an_observed_shorter_lifetime()
 {
    let clock = ManualClock::new(chrono::Utc::now());
    let world = World::with_clock(Arc::new(clock.clone())).await;
    let host = ScriptedHost::on(&world, "Durable inventory", 16_384).await;
    world.engine().stop();
    let (key, source) = source(&world, &host, host.id, "HostDeletePreflight").await;
    let source = source.unwrap();
    let preparation = source.measurement().unwrap();
    let pending = world
        .engine()
        .measure_with_preparation(source.request(), &preparation)
        .await
        .unwrap();
    let short = clock.now() + chrono::Duration::seconds(1);
    sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
        .bind(key)
        .bind(short)
        .execute(world.pool.pool())
        .await
        .unwrap();
    let again = world
        .engine()
        .measure_with_preparation(source.request(), &preparation)
        .await
        .unwrap();
    assert_eq!(again.reference.job_id, pending.reference.job_id);
    sqlx::query("UPDATE api_tokens SET expires_at=$2 WHERE id=$1")
        .bind(key)
        .bind(clock.now() + chrono::Duration::hours(2))
        .execute(world.pool.pool())
        .await
        .unwrap();
    let mut tx = world.pool.begin().await.unwrap();
    assert!(
        jobs::claim(
            &mut tx,
            pending.reference.job_id,
            new_id(),
            clock.now() + chrono::Duration::minutes(10),
            clock.now()
        )
        .await
        .unwrap()
    );
    let row = jobs::job(&mut tx, pending.reference.job_id)
        .await
        .unwrap()
        .unwrap();
    let authority: CommandAuthority =
        serde_json::from_value(row.plan["measurement_source"]["seeds"]["measure"].clone()).unwrap();
    let frame = &row.plan["measurement_source"]["seeds"]["measure"]["authority"]["canonical"];
    let id: Id = serde_json::from_value(frame["id"].clone()).unwrap();
    let body = json!({});
    let wire = link::NewCommand {
        id,
        machine_id: host.machine,
        kind: "node.resources.list",
        body: &body,
        serial_key: None,
        deadline_at: clock.now() + chrono::Duration::seconds(1),
    };
    assert!(
        authority
            .current_issue_callers(&mut tx, &wire, &clock, world.access.as_ref())
            .await
            .unwrap()
            .is_some()
    );
    let mut wrong = wire;
    wrong.serial_key = Some("unrelated");
    assert!(
        authority
            .current_issue_callers(&mut tx, &wrong, &clock, world.access.as_ref())
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();
    clock.advance(chrono::Duration::seconds(2));
    let mut tx = world.pool.begin().await.unwrap();
    let wire = link::NewCommand {
        id,
        machine_id: host.machine,
        kind: "node.resources.list",
        body: &body,
        serial_key: None,
        deadline_at: clock.now() + chrono::Duration::seconds(1),
    };
    assert!(
        authority
            .current_issue_callers(&mut tx, &wire, &clock, world.access.as_ref())
            .await
            .unwrap()
            .is_none(),
        "extending the current key never extends its observed native minimum"
    );
    tx.rollback().await.unwrap();
    assert_eq!(host.count(CommandKind::NodeResourcesList), 0);
}
