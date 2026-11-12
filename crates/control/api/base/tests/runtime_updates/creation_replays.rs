//! Accepted update jobs replay their original status and identity under current own Read.
use super::*;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::IDEMPOTENCY_HEADER;
use orch_api_core::{
    middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash},
    testing::RawReply,
};
use orch_db::queries::identity;

pub(super) fn trace_native_failures() {
    static LOGS: std::sync::Once = std::sync::Once::new();
    LOGS.call_once(|| {
        // Static boundary phases identify sanitized refusals without request values or credentials.
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(
                "orch_api_core::middleware::requests=error,orch_api::native_operations=debug",
            ))
            .with_test_writer()
            .try_init();
    });
}

async fn read_key(world: &World, user: Id, kind: &str, id: Id, op: &str) -> String {
    read_key_rules(world,user,vec![json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],"allow":[{"operation":op,"actions":["read"]}]})]).await
}
async fn read_key_rules(world: &World, user: Id, rules: Vec<serde_json::Value>) -> String {
    let scope: KeyScope = serde_json::from_value(json!({"rules":rules})).unwrap();
    assert!(scope.validate().is_empty());
    let token = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Current accepted job Read",
            token_hash: &token_hash(&token),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    token
}
async fn call(
    world: &World,
    provider: bool,
    id: Id,
    token: &str,
    key: Id,
    body: &ForkCreateBody,
) -> RawReply {
    if provider {
        world
            .app
            .call(provider_api::ProviderRuntimeUpdateCreate, &[&id])
            .token(token)
            .header(IDEMPOTENCY_HEADER, &key.to_string())
            .body(body)
            .send_bytes()
            .await
    } else {
        world
            .app
            .call(api::EnvironmentUpdateCreate, &[&id])
            .token(token)
            .header(IDEMPOTENCY_HEADER, &key.to_string())
            .body(body)
            .send_bytes()
            .await
    }
}
async fn review(world: &World, team: &Team, provider: bool) -> Id {
    if provider {
        world
            .app
            .call(
                provider_api::ProviderRuntimeUpdatePreflight,
                &[&team.provider],
            )
            .token(&team.c.admin.token)
            .body(&api::UpdatePreflightBody::default())
            .send()
            .await
            .ok()
            .preflight_id
    } else {
        world
            .app
            .call(api::EnvironmentUpdatePreflight, &[&team.environment])
            .token(&team.c.admin.token)
            .body(&api::UpdatePreflightBody::default())
            .send()
            .await
            .ok()
            .preflight_id
    }
}
#[tokio::test]
async fn environment_and_provider_update_keys_replay_current_native_jobs_without_spent_write_or_preflight()
 {
    for provider in [false, true] {
        let world = World::new().await;
        let team = team(&world).await;
        let (kind, id, op, other) = if provider {
            (
                "provider",
                team.provider,
                "ProviderRuntimeUpdateCreate",
                "ProviderGet",
            )
        } else {
            (
                "environment",
                team.environment,
                "EnvironmentUpdateCreate",
                "EnvironmentGet",
            )
        };
        let preflight = review(&world, &team, provider).await;
        let body = ForkCreateBody {
            preflight_id: preflight,
            options: None,
            notify: Some(false),
        };
        let key = new_id();
        let first = call(&world, provider, id, &team.c.admin.token, key, &body).await;
        assert_eq!(first.status, 202, "{}", String::from_utf8_lossy(&first.raw));
        let first: orch_api_contract::workspace::Job = serde_json::from_slice(&first.raw).unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind=$1")
            .bind(&first.kind)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
        let review = orch_db::queries::jobs::forks::preflight(&mut *world.conn().await, preflight)
            .await
            .unwrap();
        assert!(review.is_none(), "the creating input was consumed");
        let recorded: serde_json::Value =
            sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
                .bind(team.c.admin.id())
                .bind(key.to_string())
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        assert_eq!(recorded["outcome"]["id"], json!(first.id));
        assert!(recorded["outcome"]["data"].is_null());
        assert!(recorded.get("progress").is_none());
        // End the original browser identity. The account's new Read key cannot repeat a Write.
        sqlx::query("UPDATE sessions SET expires_at=$2 WHERE user_id=$1")
            .bind(team.c.admin.id())
            .bind(world.now() - chrono::Duration::seconds(1))
            .execute(&world.db.pool)
            .await
            .unwrap();
        let own = read_key(&world, team.c.admin.id(), kind, id, op).await;
        sqlx::query("UPDATE jobs SET state='running',started_at=$2,progress=jsonb_set(progress,'{percent}','17') WHERE id=$1").bind(first.id).bind(world.now()).execute(&world.db.pool).await.unwrap();
        let replay = call(&world, provider, id, &own, key, &body).await;
        assert_eq!(
            replay.status,
            202,
            "{}",
            String::from_utf8_lossy(&replay.raw)
        );
        let current: orch_api_contract::workspace::Job =
            serde_json::from_slice(&replay.raw).unwrap();
        assert_eq!(current.id, first.id);
        assert_ne!(current.state, first.state);
        assert_eq!(current.progress.percent, 17);
        let unrelated = read_key(&world, team.c.admin.id(), kind, id, other).await;
        let receipt = call(&world, provider, id, &unrelated, key, &body).await;
        assert_eq!(receipt.status, 202);
        let value: serde_json::Value = serde_json::from_slice(&receipt.raw).unwrap();
        assert_eq!(value, json!({"projection":"receipt","job_id":first.id}));
        let refused = call(&world, provider, id, &own, new_id(), &body).await;
        assert!(
            !refused.status.is_success(),
            "a new UUID still requires the real first Write"
        );
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind=$1")
            .bind(&first.kind)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(after, count);
    }
}

#[tokio::test]
async fn global_update_batch_replays_current_jobs_and_preserves_its_exact_receipt_without_a_second_review()
 {
    trace_native_failures();
    let world = World::new().await;
    let team = team(&world).await;
    let review = world
        .app
        .call(api::EnvironmentUpdateBatchPreflight, &[])
        .token(&team.c.admin.token)
        .body(&api::NodeEnvironmentUpdatePreflightBody {
            environment_ids: vec![team.environment, team.environment],
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the full preflight plan")
        .preflight_id;
    let body = ForkCreateBody {
        preflight_id: review,
        options: None,
        notify: Some(false),
    };
    let key = new_id();
    let first = world
        .app
        .call(api::EnvironmentUpdateBatchCreate, &[])
        .token(&team.c.admin.token)
        .header(IDEMPOTENCY_HEADER, &key.to_string())
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(first.status, 202, "{}", String::from_utf8_lossy(&first.raw));
    let first: api::EnvironmentBatch = serde_json::from_slice(&first.raw).unwrap();
    assert_eq!(first.jobs.len(), 1);
    let job = first.jobs[0].job.id;
    assert!(
        orch_db::queries::jobs::forks::preflight(&mut *world.conn().await, review)
            .await
            .unwrap()
            .is_none()
    );
    let recorded: serde_json::Value =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(team.c.admin.id())
            .bind(key.to_string())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(recorded["outcome"]["id"], json!(first.batch_id));
    assert_eq!(
        recorded["outcome"]["data"],
        json!({"jobs":[{"environment_id":team.environment,"job_id":job}]})
    );
    sqlx::query("UPDATE sessions SET expires_at=$2 WHERE user_id=$1")
        .bind(team.c.admin.id())
        .bind(world.now() - chrono::Duration::seconds(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET state='running',started_at=$2,progress=jsonb_set(progress,'{percent}','29') WHERE id=$1")
        .bind(job).bind(world.now()).execute(&world.db.pool).await.unwrap();
    let reader = read_key(
        &world,
        team.c.admin.id(),
        "environment",
        team.environment,
        "EnvironmentUpdateBatchCreate",
    )
    .await;
    let replay = world
        .app
        .call(api::EnvironmentUpdateBatchCreate, &[])
        .token(&reader)
        .header(IDEMPOTENCY_HEADER, &key.to_string())
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(
        replay.status,
        202,
        "{}",
        String::from_utf8_lossy(&replay.raw)
    );
    let current: api::EnvironmentBatch = serde_json::from_slice(&replay.raw).unwrap();
    assert_eq!(current.batch_id, first.batch_id);
    assert_eq!(current.jobs.len(), 1);
    assert_eq!(current.jobs[0].job.id, job);
    assert_eq!(current.jobs[0].job.progress.percent, 29);
    let unrelated = read_key(
        &world,
        team.c.admin.id(),
        "environment",
        team.environment,
        "EnvironmentGet",
    )
    .await;
    let receipt = world
        .app
        .call(api::EnvironmentUpdateBatchCreate, &[])
        .token(&unrelated)
        .header(IDEMPOTENCY_HEADER, &key.to_string())
        .body(&body)
        .send_bytes()
        .await;
    assert_eq!(receipt.status, 202);
    let receipt: serde_json::Value = serde_json::from_slice(&receipt.raw).unwrap();
    assert_eq!(
        receipt,
        json!({"projection":"receipt","batch_id":first.batch_id,"jobs":[{"environment_id":team.environment,"job_id":job}]})
    );
    let refused = world
        .app
        .call(api::EnvironmentUpdateBatchCreate, &[])
        .token(&reader)
        .header(IDEMPOTENCY_HEADER, &new_id().to_string())
        .body(&body)
        .send_bytes()
        .await;
    assert!(!refused.status.is_success());
    let rows = orch_db::queries::jobs::forks::of_batch(&mut *world.conn().await, first.batch_id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, job);
}

async fn node_update(
    world: &World,
    node: Id,
    token: &str,
    key: Id,
    body: &ForkCreateBody,
) -> RawReply {
    world
        .app
        .call(api::NodeEnvironmentUpdateCreate, &[&node])
        .token(token)
        .header(IDEMPOTENCY_HEADER, &key.to_string())
        .body(body)
        .send_bytes()
        .await
}

#[tokio::test]
async fn node_update_batch_replays_only_accepted_members_under_current_own_read() {
    let world = World::new().await;
    let team = team(&world).await;
    let review = world
        .app
        .call(api::NodeEnvironmentUpdatePreflight, &[&team.c.acme])
        .token(&team.c.admin.token)
        .body(&api::NodeEnvironmentUpdatePreflightBody {
            environment_ids: vec![team.environment],
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the administrator receives the full node update plan")
        .preflight_id;
    let body = ForkCreateBody {
        preflight_id: review,
        options: None,
        notify: Some(false),
    };
    let key = new_id();
    let first = node_update(&world, team.c.acme, &team.c.admin.token, key, &body).await;
    assert_eq!(first.status, 202, "{}", String::from_utf8_lossy(&first.raw));
    let first: api::EnvironmentBatch = serde_json::from_slice(&first.raw).unwrap();
    assert_eq!(first.jobs.len(), 1);
    let job = first.jobs[0].job.id;
    assert!(
        orch_db::queries::jobs::forks::preflight(&mut *world.conn().await, review)
            .await
            .unwrap()
            .is_none()
    );
    let record: serde_json::Value =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(team.c.admin.id())
            .bind(key.to_string())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(record["outcome"]["id"], json!(first.batch_id));
    assert_eq!(
        record["outcome"]["data"],
        json!({"node":team.c.acme,"jobs":[[team.environment,job]]})
    );
    sqlx::query("UPDATE sessions SET expires_at=$2 WHERE user_id=$1")
        .bind(team.c.admin.id())
        .bind(world.now() - chrono::Duration::seconds(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET state='running',started_at=$2,progress=jsonb_set(progress,'{percent}','31') WHERE id=$1")
        .bind(job).bind(world.now()).execute(&world.db.pool).await.unwrap();
    let reader = read_key(
        &world,
        team.c.admin.id(),
        "environment",
        team.environment,
        "NodeEnvironmentUpdateCreate",
    )
    .await;
    let replay = node_update(&world, team.c.acme, &reader, key, &body).await;
    assert_eq!(
        replay.status,
        202,
        "{}",
        String::from_utf8_lossy(&replay.raw)
    );
    let current: api::EnvironmentBatch = serde_json::from_slice(&replay.raw).unwrap();
    assert_eq!(current.batch_id, first.batch_id);
    assert_eq!(current.jobs.len(), 1);
    assert_eq!(current.jobs[0].environment.id, team.environment);
    assert_eq!(current.jobs[0].job.id, job);
    assert_eq!(current.jobs[0].job.progress.percent, 31);
    let unrelated = read_key(
        &world,
        team.c.admin.id(),
        "environment",
        team.environment,
        "EnvironmentGet",
    )
    .await;
    let receipt = node_update(&world, team.c.acme, &unrelated, key, &body).await;
    assert_eq!(receipt.status, 202);
    let receipt: serde_json::Value = serde_json::from_slice(&receipt.raw).unwrap();
    assert_eq!(
        receipt,
        json!({"projection":"receipt","node_id":team.c.acme,"batch_id":first.batch_id,"job_ids":[job]})
    );
    assert!(
        !node_update(&world, team.c.acme, &reader, new_id(), &body)
            .await
            .status
            .is_success()
    );
    let rows = orch_db::queries::jobs::forks::of_batch(&mut *world.conn().await, first.batch_id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, job);
}

async fn provider_batch(world: &World, token: &str, key: Id, body: &ForkCreateBody) -> RawReply {
    world
        .app
        .call(provider_api::ProviderRuntimeUpdateBatchCreate, &[])
        .token(token)
        .header(IDEMPOTENCY_HEADER, &key.to_string())
        .query(&orch_api_contract::forks::UpdateConfirmQuery {
            notify: Some(false),
        })
        .body(body)
        .send_bytes()
        .await
}
#[tokio::test]
async fn provider_update_batch_replays_current_native_rows_without_creating_another_batch() {
    let world = World::new().await;
    let team = team(&world).await;
    let review = world
        .app
        .call(provider_api::ProviderRuntimeUpdateBatchPreflight, &[])
        .token(&team.c.admin.token)
        .body(&provider_api::ProviderRuntimeUpdateBatchPreflightBody {
            provider_ids: vec![team.provider, team.provider],
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the administrator receives the full provider update plan")
        .preflight_id;
    let body = ForkCreateBody {
        preflight_id: review,
        options: None,
        notify: Some(false),
    };
    let key = new_id();
    let first = provider_batch(&world, &team.c.admin.token, key, &body).await;
    assert_eq!(first.status, 202, "{}", String::from_utf8_lossy(&first.raw));
    let first: provider_api::ProviderBatch = serde_json::from_slice(&first.raw).unwrap();
    assert_eq!(first.jobs.len(), 1);
    let job = first.jobs[0].job.id;
    assert!(
        orch_db::queries::jobs::forks::preflight(&mut *world.conn().await, review)
            .await
            .unwrap()
            .is_none()
    );
    let record: serde_json::Value =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(team.c.admin.id())
            .bind(key.to_string())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(record["outcome"]["id"], json!(first.batch_id));
    assert_eq!(
        record["outcome"]["data"],
        json!({"jobs":[[team.provider,job]]})
    );
    sqlx::query("UPDATE sessions SET expires_at=$2 WHERE user_id=$1")
        .bind(team.c.admin.id())
        .bind(world.now() - chrono::Duration::seconds(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET state='running',started_at=$2,progress=jsonb_set(progress,'{percent}','37') WHERE id=$1")
        .bind(job).bind(world.now()).execute(&world.db.pool).await.unwrap();
    let reader = read_key(
        &world,
        team.c.admin.id(),
        "provider",
        team.provider,
        "ProviderRuntimeUpdateBatchCreate",
    )
    .await;
    let replay = provider_batch(&world, &reader, key, &body).await;
    assert_eq!(
        replay.status,
        202,
        "{}",
        String::from_utf8_lossy(&replay.raw)
    );
    let current: provider_api::ProviderBatch = serde_json::from_slice(&replay.raw).unwrap();
    assert_eq!(current.batch_id, first.batch_id);
    assert_eq!(current.jobs.len(), 1);
    assert_eq!(current.jobs[0].provider.id, team.provider);
    assert_eq!(current.jobs[0].job.id, job);
    assert_eq!(current.jobs[0].job.progress.percent, 37);
    let unrelated = read_key(
        &world,
        team.c.admin.id(),
        "provider",
        team.provider,
        "ProviderGet",
    )
    .await;
    let receipt = provider_batch(&world, &unrelated, key, &body).await;
    assert_eq!(receipt.status, 202);
    let receipt: serde_json::Value = serde_json::from_slice(&receipt.raw).unwrap();
    assert_eq!(
        receipt,
        json!({"projection":"receipt","batch_id":first.batch_id,"jobs":[{"provider_id":team.provider,"job_id":job}]})
    );
    assert!(
        !provider_batch(&world, &reader, new_id(), &body)
            .await
            .status
            .is_success()
    );
    let jobs = orch_db::queries::jobs::forks::of_batch(&mut *world.conn().await, first.batch_id)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, job);
}

async fn node_move(
    world: &World,
    node: Id,
    token: &str,
    key: Id,
    body: &ForkCreateBody,
) -> RawReply {
    world
        .app
        .call(api::NodeEnvironmentMoveCreate, &[&node])
        .token(token)
        .header(IDEMPOTENCY_HEADER, &key.to_string())
        .body(body)
        .send_bytes()
        .await
}
#[tokio::test]
async fn an_actual_empty_node_move_acceptance_replays_its_identity_without_reopening_the_review() {
    trace_native_failures();
    let world = World::new().await;
    let team = team(&world).await;
    let review = world
        .app
        .call(api::NodeEnvironmentMovePreflight, &[&team.c.acme])
        .token(&team.c.admin.token)
        .body(&api::NodeEnvironmentMovePreflightBody {
            environment_ids: Vec::new(),
            host_id: None,
            machine_id: None,
            template_id: None,
            copy_directories: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the administrator receives the full empty node move plan")
        .preflight_id;
    let body = ForkCreateBody {
        preflight_id: review,
        options: None,
        notify: Some(false),
    };
    let key = new_id();
    let first = node_move(&world, team.c.acme, &team.c.admin.token, key, &body).await;
    assert_eq!(first.status, 202, "{}", String::from_utf8_lossy(&first.raw));
    let first: api::EnvironmentBatch = serde_json::from_slice(&first.raw).unwrap();
    assert!(first.jobs.is_empty());
    assert!(
        orch_db::queries::jobs::forks::preflight(&mut *world.conn().await, review)
            .await
            .unwrap()
            .is_none()
    );
    let record: serde_json::Value =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(team.c.admin.id())
            .bind(key.to_string())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(record["outcome"]["id"], json!(first.batch_id));
    assert_eq!(
        record["outcome"]["data"],
        json!({"node":team.c.acme,"jobs":[]})
    );
    sqlx::query("UPDATE sessions SET expires_at=$2 WHERE user_id=$1")
        .bind(team.c.admin.id())
        .bind(world.now() - chrono::Duration::seconds(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    let resources = [
        "environment",
        "host",
        "machine",
        "template",
        "mount",
        "mcp_server",
        "org",
        "project",
        "topic",
        "thread",
    ];
    let scope:KeyScope=serde_json::from_value(json!({"rules":[{"target":{"kind":"org","id":team.c.acme,"reach":"subtree"},"resources":resources,"allow":[{"operation":"NodeEnvironmentMoveCreate","actions":["read"]}]}]})).unwrap();
    assert!(scope.validate().is_empty());
    let reader = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: team.c.admin.id(),
            name: "Current empty accepted move Read",
            token_hash: &token_hash(&reader),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    let replay = node_move(&world, team.c.acme, &reader, key, &body).await;
    assert_eq!(
        replay.status,
        202,
        "{}",
        String::from_utf8_lossy(&replay.raw)
    );
    let current: api::EnvironmentBatch = serde_json::from_slice(&replay.raw).unwrap();
    assert_eq!(current.batch_id, first.batch_id);
    assert!(current.jobs.is_empty());
    let environment_reader = read_key(
        &world,
        team.c.admin.id(),
        "environment",
        team.environment,
        "NodeEnvironmentMoveCreate",
    )
    .await;
    let without_context = node_move(&world, team.c.acme, &environment_reader, key, &body).await;
    assert_eq!(without_context.status, 202);
    let without_context: serde_json::Value = serde_json::from_slice(&without_context.raw).unwrap();
    assert_eq!(
        without_context,
        json!({"projection":"receipt","node_id":team.c.acme,"batch_id":first.batch_id,"job_ids":[]}),
        "an empty batch still belongs to its actual saved node, not an arbitrary environment"
    );
    let unrelated = read_key(
        &world,
        team.c.admin.id(),
        "environment",
        team.environment,
        "EnvironmentGet",
    )
    .await;
    let receipt = node_move(&world, team.c.acme, &unrelated, key, &body).await;
    assert_eq!(receipt.status, 202);
    let receipt: serde_json::Value = serde_json::from_slice(&receipt.raw).unwrap();
    assert_eq!(
        receipt,
        json!({"projection":"receipt","node_id":team.c.acme,"batch_id":first.batch_id,"job_ids":[]})
    );
    assert!(
        !node_move(&world, team.c.acme, &reader, new_id(), &body)
            .await
            .status
            .is_success()
    );
    assert!(
        orch_db::queries::jobs::forks::of_batch(&mut *world.conn().await, first.batch_id)
            .await
            .unwrap()
            .is_empty()
    );
}

async fn environment_move(
    world: &World,
    environment: Id,
    token: &str,
    act: Id,
    body: &ForkCreateBody,
) -> RawReply {
    let call = world
        .app
        .call(api::EnvironmentMoveCreate, &[&environment])
        .token(token)
        .header(IDEMPOTENCY_HEADER, &act.to_string())
        .body(body);
    call.send_bytes().await
}
#[tokio::test]
async fn a_completed_environment_move_replays_current_job_without_its_spent_review_or_original_writer()
 {
    trace_native_failures();
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    let answers = answer_move_plans(&world, &team, target).await;
    let request = api::EnvironmentMovePreflightBody {
        host_id: Some(target),
        machine_id: None,
        template_id: None,
        copy_directories: None,
        home: None,
        private_views: None,
        files: None,
        check_fingerprints: None,
        fingerprint_request_id: None,
        mounts: None,
    };
    let review = world
        .app
        .call(api::EnvironmentMovePreflight, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&request)
        .send()
        .await
        .full_move()
        .ok();
    finish_move_count(&world, &team, &review.files).await;
    let review = world
        .app
        .call(api::EnvironmentMovePreflight, &[&team.environment])
        .token(&team.c.admin.token)
        .body(&request)
        .send()
        .await
        .full_move()
        .ok();
    assert!(review.blockers.is_empty(), "{:?}", review.blockers);
    let body = ForkCreateBody {
        preflight_id: review.preflight_id,
        options: None,
        notify: Some(false),
    };
    let act = new_id();
    let first = environment_move(&world, team.environment, &team.c.admin.token, act, &body).await;
    assert_eq!(first.status, 202, "{}", String::from_utf8_lossy(&first.raw));
    let first: orch_api_contract::workspace::Job =
        serde_json::from_slice::<api::EnvironmentMoveCreateResponse>(&first.raw)
            .unwrap()
            .into_full()
            .unwrap();
    assert_eq!(first.environment_id, Some(team.environment));
    assert!(
        orch_db::queries::jobs::forks::preflight(&mut *world.conn().await, review.preflight_id)
            .await
            .unwrap()
            .is_none()
    );
    let record: serde_json::Value =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(team.c.admin.id())
            .bind(act.to_string())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(record["outcome"]["id"], json!(first.id));
    assert_eq!(
        record["outcome"]["data"],
        json!({"environment":team.environment})
    );
    assert!(record.get("progress").is_none());
    sqlx::query("UPDATE sessions SET expires_at=$2 WHERE user_id=$1")
        .bind(team.c.admin.id())
        .bind(world.now() - chrono::Duration::seconds(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET state='running',started_at=$2,progress=jsonb_set(progress,'{percent}','17') WHERE id=$1")
        .bind(first.id).bind(world.now()).execute(&world.db.pool).await.unwrap();
    let own = read_key(
        &world,
        team.c.admin.id(),
        "environment",
        team.environment,
        "EnvironmentMoveCreate",
    )
    .await;
    let repeated = environment_move(&world, team.environment, &own, act, &body).await;
    assert_eq!(
        repeated.status,
        202,
        "{}",
        String::from_utf8_lossy(&repeated.raw)
    );
    assert_eq!(repeated.headers.get("cache-control").unwrap(), "no-store");
    let current: orch_api_contract::workspace::Job =
        serde_json::from_slice::<api::EnvironmentMoveCreateResponse>(&repeated.raw)
            .unwrap()
            .into_full()
            .unwrap();
    assert_eq!(current.id, first.id);
    assert_eq!(current.progress.percent, 17);
    assert_ne!(current.state, first.state);
    let unrelated = read_key(
        &world,
        team.c.admin.id(),
        "environment",
        team.environment,
        "EnvironmentGet",
    )
    .await;
    let receipt = environment_move(&world, team.environment, &unrelated, act, &body).await;
    assert_eq!(receipt.status, 202);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&receipt.raw).unwrap(),
        json!({"projection":"receipt","environment_id":team.environment,"job_id":first.id})
    );
    let fresh = environment_move(&world, team.environment, &own, new_id(), &body).await;
    assert!(
        fresh.status.is_client_error(),
        "a new creation still requires its real current Manage and review"
    );
    let changed = environment_move(
        &world,
        team.environment,
        &own,
        act,
        &ForkCreateBody {
            notify: Some(true),
            ..body
        },
    )
    .await;
    assert_eq!(
        changed.status, 409,
        "the completed key cannot accept a different canonical body"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM jobs WHERE kind='environment_move' AND environment_id=$1"
        )
        .bind(team.environment)
        .fetch_one(&world.db.pool)
        .await
        .unwrap(),
        1
    );
    answers.abort();
    let _ = answers.await;
}

async fn recovery_reader(world: &World, user: Id, op: &str) -> String {
    let mut objects: Vec<(String, Id)> =
        sqlx::query_as("SELECT kind,id FROM nodes WHERE deleted_at IS NULL")
            .fetch_all(&world.db.pool)
            .await
            .unwrap();
    for kind in ["environment", "host", "machine", "template"] {
        // The query statements are closed fixture constants; resource identities remain bound facts.
        let ids: Vec<Id> = match kind {
            "environment" => sqlx::query_scalar("SELECT id FROM environments")
                .fetch_all(&world.db.pool)
                .await
                .unwrap(),
            "host" => sqlx::query_scalar("SELECT id FROM hosts")
                .fetch_all(&world.db.pool)
                .await
                .unwrap(),
            "machine" => sqlx::query_scalar("SELECT id FROM machines")
                .fetch_all(&world.db.pool)
                .await
                .unwrap(),
            "template" => sqlx::query_scalar("SELECT id FROM env_templates")
                .fetch_all(&world.db.pool)
                .await
                .unwrap(),
            _ => unreachable!("the fixture has only its four native resource families"),
        };
        objects.extend(ids.into_iter().map(|id| (kind.into(), id)));
    }
    read_key_rules(world,user,objects.into_iter().map(|(kind,id)|json!({"target":{"kind":kind,"id":id,"reach":"self"},"resources":[kind],"allow":[{"operation":op,"actions":["read"]}]})).collect()).await
}
async fn recovery_call(world: &World, token: &str, act: Id, body: &ForkCreateBody) -> RawReply {
    let call = world
        .app
        .call(orch_api_contract::loss::EnvironmentRecoverCreate, &[])
        .token(token)
        .header(IDEMPOTENCY_HEADER, &act.to_string())
        .body(body);
    call.send_bytes().await
}
#[tokio::test]
async fn a_completed_recovery_replays_its_accepted_jobs_with_new_read_without_repeating_native_inspection()
 {
    trace_native_failures();
    use orch_api_contract::loss::{
        EnvironmentRecoverCreateResponse, EnvironmentRecoverPreflight,
        EnvironmentRecoverPreflightBody,
    };
    let world = World::new().await;
    let team = team(&world).await;
    let target = movable(&world, &team).await;
    sqlx::query("UPDATE runtimes SET lost_at=$2,state='stopped' WHERE id=$1")
        .bind(team.runtime)
        .bind(world.now())
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE environments SET status='lost' WHERE id=$1")
        .bind(team.environment)
        .execute(&world.db.pool)
        .await
        .unwrap();
    let machine: Id = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(target)
        .fetch_one(&world.db.pool)
        .await
        .unwrap();
    let pool = world.db.pool.clone();
    let answers = tokio::spawn(async move {
        loop {
            if sqlx::query("UPDATE commands SET state='succeeded',result=$2 WHERE machine_id=$1 AND kind='node.volume.inspect' AND state IN ('queued','sent','accepted')")
            .bind(machine).bind(json!({"exists":false,"bytes":null})).execute(&pool).await.is_err() {return;}
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });
    let plan = world
        .app
        .call(EnvironmentRecoverPreflight, &[])
        .token(&team.c.admin.token)
        .body(&EnvironmentRecoverPreflightBody {
            environment_ids: vec![team.environment],
            host_id: Some(target),
            choices: Vec::new(),
            environment: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .unwrap();
    assert_eq!(plan.executable, 1);
    let body = ForkCreateBody {
        preflight_id: plan.preflight_id,
        options: None,
        notify: Some(false),
    };
    let act = new_id();
    let first = recovery_call(&world, &team.c.admin.token, act, &body).await;
    assert_eq!(first.status, 202, "{}", String::from_utf8_lossy(&first.raw));
    let first = serde_json::from_slice::<EnvironmentRecoverCreateResponse>(&first.raw)
        .unwrap()
        .into_full()
        .unwrap();
    assert_eq!(first.jobs.len(), 1);
    let job = first.jobs[0].job.id;
    assert!(
        orch_db::queries::jobs::forks::preflight(&mut *world.conn().await, plan.preflight_id)
            .await
            .unwrap()
            .is_none()
    );
    let record: serde_json::Value =
        sqlx::query_scalar("SELECT response FROM idempotency_keys WHERE user_id=$1 AND key=$2")
            .bind(team.c.admin.id())
            .bind(act.to_string())
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert_eq!(record["outcome"]["id"], json!(first.batch_id));
    assert_eq!(
        record["outcome"]["data"],
        json!({"jobs":[{"id":job,"kind":first.jobs[0].job.kind,"environment":team.environment,"node":null}]})
    );
    let probes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM commands WHERE kind='node.volume.inspect'")
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
    assert!(
        probes > 0,
        "the original producer really inspected the native volumes"
    );
    sqlx::query("UPDATE sessions SET expires_at=$2 WHERE user_id=$1")
        .bind(team.c.admin.id())
        .bind(world.now() - chrono::Duration::seconds(1))
        .execute(&world.db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET state='running',started_at=$2,progress=jsonb_set(progress,'{percent}','23') WHERE id=$1").bind(job).bind(world.now()).execute(&world.db.pool).await.unwrap();
    let reader = recovery_reader(&world, team.c.admin.id(), "EnvironmentRecoverCreate").await;
    let replay = recovery_call(&world, &reader, act, &body).await;
    assert_eq!(
        replay.status,
        202,
        "{}",
        String::from_utf8_lossy(&replay.raw)
    );
    assert_eq!(replay.headers.get("cache-control").unwrap(), "no-store");
    let current = serde_json::from_slice::<EnvironmentRecoverCreateResponse>(&replay.raw)
        .unwrap()
        .into_full()
        .unwrap();
    assert_eq!(current.batch_id, first.batch_id);
    assert_eq!(current.jobs.len(), 1);
    assert_eq!(current.jobs[0].job.id, job);
    assert_eq!(current.jobs[0].job.progress.percent, 23);
    let other = recovery_reader(&world, team.c.admin.id(), "EnvironmentRecoverGet").await;
    let receipt = recovery_call(&world, &other, act, &body).await;
    assert_eq!(receipt.status, 202);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&receipt.raw).unwrap(),
        json!({"projection":"receipt","batch_id":first.batch_id,"job_ids":[job]})
    );
    assert!(
        recovery_call(&world, &reader, new_id(), &body)
            .await
            .status
            .is_client_error()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM commands WHERE kind='node.volume.inspect'"
        )
        .fetch_one(&world.db.pool)
        .await
        .unwrap(),
        probes
    );
    assert_eq!(
        orch_db::queries::jobs::forks::of_batch(&mut *world.conn().await, first.batch_id)
            .await
            .unwrap()
            .len(),
        1
    );
    sqlx::query("UPDATE jobs SET batch_id=$2 WHERE id=$1")
        .bind(job)
        .bind(new_id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    let changed = recovery_call(&world, &reader, act, &body).await;
    assert_eq!(changed.status, 202);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&changed.raw).unwrap(),
        json!({"projection":"receipt","batch_id":first.batch_id,"job_ids":[job]}),
        "a reassociated job never borrows its old batch's content"
    );
    answers.abort();
    let _ = answers.await;
}
