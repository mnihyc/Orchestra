//! Usage rows bind their native scopes instead of request or cap identifiers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod resources;
mod support;
mod usage_world;

use http::StatusCode;
use orch_access_contract::key_scope::KeyScope;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::usage::{
    CapInput, CapMeter, CapPeriod, CapSplit, CapSubjectInput, UsageCapCreate, UsageCapDelete,
    UsageHealthGet, UsageHealthGetQuery,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::testing::RawReply;
use orch_db::queries::identity::{self, NewApiToken};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use support::World;

async fn key(world: &World, user: Id, kind: &str, id: Id, operation: &str, action: &str) -> String {
    key_with_scope(world,user,json!({"rules":[{"target":{"kind":kind,"id":id,"reach":"self"},"allow":[{"operation":operation,"actions":[action]}]}]})).await
}
async fn key_with_scope(world: &World, user: Id, value: Json) -> String {
    let scope: KeyScope = serde_json::from_value(value).unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Usage native scope",
            token_hash: &token_hash(&secret),
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&serde_json::to_value(scope).unwrap()),
    )
    .await
    .unwrap();
    secret
}
async fn health(world: &World, token: &str, provider: Id) -> RawReply {
    world
        .app
        .call(UsageHealthGet, &[])
        .token(token)
        .query(&UsageHealthGetQuery {
            provider_id: provider,
        })
        .send_bytes()
        .await
}
fn value(reply: RawReply) -> Json {
    assert_eq!(
        reply.status,
        StatusCode::OK,
        "native usage response: {}",
        String::from_utf8_lossy(&reply.raw)
    );
    serde_json::from_slice(&reply.raw).unwrap()
}
async fn cap(world: &World, team: &usage_world::Team) -> Id {
    let reply = world
        .app
        .call(UsageCapCreate, &[])
        .token(&team.boss.token)
        .body(&cap_input(Some(team.project), None))
        .send_bytes()
        .await;
    assert_eq!(
        reply.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let reply: Json = serde_json::from_slice(&reply.raw).unwrap();
    serde_json::from_value(reply["id"].clone()).unwrap()
}

#[tokio::test]
async fn health_query_binds_the_actual_provider_and_preserves_native_health() {
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    let expected = value(health(&world, &t.boss.token, t.provider).await);
    let selected = key(
        &world,
        t.boss.id(),
        "provider",
        t.provider,
        "UsageHealthGet",
        "read",
    )
    .await;
    assert_eq!(value(health(&world, &selected, t.provider).await), expected);
    let another = new_id();
    let wrong = key(
        &world,
        t.boss.id(),
        "provider",
        another,
        "UsageHealthGet",
        "read",
    )
    .await;
    assert_eq!(
        health(&world, &wrong, t.provider).await.problem().code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn health_publication_keeps_its_actual_provider_read_current_after_activity_wait() {
    for change in ["unchanged", "expiry", "scope"] {
        let world = World::new().await;
        let t = usage_world::team(&world).await;
        let expected = value(health(&world, &t.boss.token, t.provider).await);
        let secret = key(
            &world,
            t.boss.id(),
            "provider",
            t.provider,
            "UsageHealthGet",
            "read",
        )
        .await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let provider = t.provider;
        let token = secret.clone();
        let action = tokio::spawn(async move {
            app.call(UsageHealthGet, &[])
                .token(&token)
                .query(&UsageHealthGetQuery {
                    provider_id: provider,
                })
                .send_bytes()
                .await
        });
        blocked(&world, pid, &action, "%INSERT INTO api_token_activity%").await;
        match change {
            "expiry" => world.advance(chrono::Duration::hours(2)),
            "scope" => {
                sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE token_hash=$1")
                    .bind(token_hash(&secret))
                    .bind(json!({"rules":[{"target":{"kind":"provider","id":new_id(),"reach":"self"},"allow":[{"operation":"UsageHealthGet","actions":["read"]}]}]}))
                    .execute(&world.db.pool)
                    .await
                    .unwrap();
            }
            "unchanged" => {}
            _ => unreachable!(),
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT_NATIVE, action)
            .await
            .unwrap()
            .unwrap();
        if change == "unchanged" {
            assert_eq!(value(reply), expected);
        } else {
            let (status, code) = if change == "expiry" {
                (StatusCode::UNAUTHORIZED, ErrorCode::AuthInvalidToken)
            } else {
                (StatusCode::FORBIDDEN, ErrorCode::AuthzDenied)
            };
            assert_eq!(
                reply.status,
                status,
                "{change}: {}",
                String::from_utf8_lossy(&reply.raw)
            );
            assert_eq!(reply.problem().code, code);
        }
    }
}

#[tokio::test]
async fn health_scope_does_not_lend_native_provider_view_or_another_operations_read() {
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    let stranger = key(
        &world,
        t.stranger.id(),
        "provider",
        t.provider,
        "UsageHealthGet",
        "read",
    )
    .await;
    assert_eq!(
        health(&world, &stranger, t.provider).await.problem().code,
        ErrorCode::AuthzDenied
    );
    let unrelated = key(
        &world,
        t.boss.id(),
        "provider",
        t.provider,
        "ProviderGet",
        "read",
    )
    .await;
    assert_eq!(
        health(&world, &unrelated, t.provider).await.problem().code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn cap_delete_uses_the_stored_home_and_returns_the_native_empty_acknowledgement() {
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    let id = cap(&world, &t).await;
    let fake_subject = key(
        &world,
        t.boss.id(),
        "project",
        id,
        "UsageCapDelete",
        "manage",
    )
    .await;
    let wrong = world
        .app
        .call(UsageCapDelete, &[&id])
        .token(&fake_subject)
        .send_bytes()
        .await;
    assert_eq!(
        wrong.problem().code,
        ErrorCode::AuthzDenied,
        "a cap UUID is not its defining Project"
    );
    assert!(
        orch_db::usage::caps::get(&mut *world.conn().await, id)
            .await
            .unwrap()
            .is_some()
    );
    let selected = key(
        &world,
        t.boss.id(),
        "project",
        t.project,
        "UsageCapDelete",
        "manage",
    )
    .await;
    let deleted = world
        .app
        .call(UsageCapDelete, &[&id])
        .token(&selected)
        .send_bytes()
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert!(deleted.raw.is_empty());
    assert!(
        orch_db::usage::caps::get(&mut *world.conn().await, id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn cap_delete_scope_does_not_supply_native_usage_management() {
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    let id = cap(&world, &t).await;
    let viewer = key(
        &world,
        t.viewer.id(),
        "project",
        t.project,
        "UsageCapDelete",
        "manage",
    )
    .await;
    assert_eq!(
        world
            .app
            .call(UsageCapDelete, &[&id])
            .token(&viewer)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert!(
        orch_db::usage::caps::get(&mut *world.conn().await, id)
            .await
            .unwrap()
            .is_some()
    );
}

async fn org_key(world: &World, t: &usage_world::Team, operation: &str) -> String {
    key_with_scope(world,t.boss.id(),json!({"rules":[{"target":{"kind":"org","id":t.acme,"reach":"subtree"},"allow":[{"operation":operation,"actions":["read"]}]}]})).await
}

#[tokio::test]
async fn cap_lists_select_native_homes_and_preserve_ordinary_standings() {
    use orch_api_contract::usage::{UsageCapList, UsageCapListQuery};
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    let id = cap(&world, &t).await;
    let query = UsageCapListQuery {
        node_id: Some(t.project),
        provider_id: None,
        system: None,
        covering: None,
    };
    let ordinary = value(
        world
            .app
            .call(UsageCapList, &[])
            .token(&t.boss.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary.as_array().unwrap().len(), 1);
    assert_eq!(ordinary[0]["cap"]["id"], json!(id));
    let selected = org_key(&world, &t, "UsageCapList").await;
    let actual = value(
        world
            .app
            .call(UsageCapList, &[])
            .token(&selected)
            .query(&query)
            .send_bytes()
            .await,
    );
    // Scope constrains use of management flags; the standings, cap and native order stay the same.
    assert_eq!(actual[0]["cap"]["id"], ordinary[0]["cap"]["id"]);
    assert_eq!(actual[0]["used"], ordinary[0]["used"]);
    assert_eq!(actual[0]["cap"]["node"], ordinary[0]["cap"]["node"]);
    let wrong = key(
        &world,
        t.boss.id(),
        "project",
        new_id(),
        "UsageCapList",
        "read",
    )
    .await;
    assert_eq!(
        world
            .app
            .call(UsageCapList, &[])
            .token(&wrong)
            .query(&query)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let stranger = key(
        &world,
        t.stranger.id(),
        "project",
        t.project,
        "UsageCapList",
        "read",
    )
    .await;
    assert_eq!(
        world
            .app
            .call(UsageCapList, &[])
            .token(&stranger)
            .query(&query)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn limits_bind_the_native_node_and_applicable_caps_without_lending_anothers_share() {
    use orch_api_contract::usage::{UsageLimitsGet, UsageLimitsGetQuery};
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    let id = cap(&world, &t).await;
    let query = UsageLimitsGetQuery {
        node_id: t.project,
        user_id: None,
    };
    let ordinary = value(
        world
            .app
            .call(UsageLimitsGet, &[])
            .token(&t.boss.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary["caps"][0]["cap"]["id"], json!(id));
    let selected = org_key(&world, &t, "UsageLimitsGet").await;
    let actual = value(
        world
            .app
            .call(UsageLimitsGet, &[])
            .token(&selected)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(
        actual["caps"][0]["cap"]["id"],
        ordinary["caps"][0]["cap"]["id"]
    );
    assert_eq!(actual["caps"][0]["used"], ordinary["caps"][0]["used"]);
    let viewer = key(
        &world,
        t.viewer.id(),
        "project",
        t.project,
        "UsageLimitsGet",
        "read",
    )
    .await;
    let others = UsageLimitsGetQuery {
        node_id: t.project,
        user_id: Some(t.boss.id()),
    };
    assert_eq!(
        world
            .app
            .call(UsageLimitsGet, &[])
            .token(&viewer)
            .query(&others)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn cap_waiting_binds_the_stored_cap_home_and_keeps_an_empty_native_queue() {
    use orch_api_contract::usage::UsageCapWaitingList;
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    let id = cap(&world, &t).await;
    let ordinary = value(
        world
            .app
            .call(UsageCapWaitingList, &[&id])
            .token(&t.boss.token)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary, json!({"items":[]}));
    let selected = key(
        &world,
        t.boss.id(),
        "project",
        t.project,
        "UsageCapWaitingList",
        "read",
    )
    .await;
    assert_eq!(
        value(
            world
                .app
                .call(UsageCapWaitingList, &[&id])
                .token(&selected)
                .send_bytes()
                .await
        ),
        ordinary
    );
    let fake = key(
        &world,
        t.boss.id(),
        "project",
        id,
        "UsageCapWaitingList",
        "read",
    )
    .await;
    assert_eq!(
        world
            .app
            .call(UsageCapWaitingList, &[&id])
            .token(&fake)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

#[tokio::test]
async fn rate_table_variants_bind_native_providers_and_keep_system_models_private() {
    use orch_api_contract::resources::{ProviderModelCreate, ProviderModelCreateBody};
    use orch_api_contract::usage::{UsageRateTableGet, UsageRateTableGetQuery};
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    world
        .app
        .call(ProviderModelCreate, &[&t.provider])
        .token(&t.boss.token)
        .body(&ProviderModelCreateBody {
            model_name: "usage-selected-model".into(),
            display_name: None,
            parameters: None,
            groups: None,
        })
        .send()
        .await
        .ok();
    let query = UsageRateTableGetQuery {
        node_id: Some(t.project),
        system: None,
    };
    let ordinary = value(
        world
            .app
            .call(UsageRateTableGet, &[])
            .token(&t.boss.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary["models"].as_array().unwrap().len(), 1);
    let node_only = key(
        &world,
        t.boss.id(),
        "project",
        t.project,
        "UsageRateTableGet.node",
        "read",
    )
    .await;
    assert!(
        value(
            world
                .app
                .call(UsageRateTableGet, &[])
                .token(&node_only)
                .query(&query)
                .send_bytes()
                .await
        )["models"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a node rule does not disclose an unselected provider's native model source"
    );
    let selected = org_key(&world, &t, "UsageRateTableGet.node").await;
    assert_eq!(
        value(
            world
                .app
                .call(UsageRateTableGet, &[])
                .token(&selected)
                .query(&query)
                .send_bytes()
                .await
        ),
        ordinary
    );
    let system = UsageRateTableGetQuery {
        node_id: None,
        system: Some(true),
    };
    let system_key=key_with_scope(&world,t.sysadmin.id(),json!({"rules":[{"target":{"kind":"system","reach":"self"},"allow":[{"operation":"UsageRateTableGet.system","actions":["manage"]}]}]})).await;
    let expected = value(
        world
            .app
            .call(UsageRateTableGet, &[])
            .token(&t.sysadmin.token)
            .query(&system)
            .send_bytes()
            .await,
    );
    assert_eq!(
        value(
            world
                .app
                .call(UsageRateTableGet, &[])
                .token(&system_key)
                .query(&system)
                .send_bytes()
                .await
        ),
        expected
    );
    assert!(expected["models"].as_array().unwrap().is_empty());
    assert_eq!(
        world
            .app
            .call(UsageRateTableGet, &[])
            .token(&selected)
            .query(&system)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

fn cap_input(node: Option<Id>, provider: Option<Id>) -> CapInput {
    CapInput {
        node_id: node,
        provider_id: provider,
        subject: CapSubjectInput::Everyone,
        split: CapSplit::Shared,
        models: None,
        services: None,
        purposes: None,
        tiers: None,
        harness_kinds: None,
        meter: CapMeter::Tokens,
        period: Some(CapPeriod::Day),
        counted_from: None,
        amount: Some("1000".into()),
        at_limit: None,
        warn_percent: None,
        uncounted: None,
        note: Some("cap content sentinel".into()),
    }
}
#[tokio::test]
async fn cap_creation_keeps_the_native_effect_with_a_typed_receipt_and_own_operation_read() {
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    let manage = key(
        &world,
        t.boss.id(),
        "project",
        t.project,
        "UsageCapCreate",
        "manage",
    )
    .await;
    let receipt = world
        .app
        .call(UsageCapCreate, &[])
        .token(&manage)
        .body(&cap_input(Some(t.project), None))
        .send_bytes()
        .await;
    assert_eq!(
        receipt.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&receipt.raw)
    );
    let receipt: Json = serde_json::from_slice(&receipt.raw).unwrap();
    assert_eq!(receipt["projection"], "receipt");
    assert!(receipt.get("note").is_none());
    let id: Id = serde_json::from_value(receipt["id"].clone()).unwrap();
    let row = orch_db::usage::caps::get(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.node_id, Some(t.project));
    assert_eq!(row.note, "cap content sentinel");
    let selected=key_with_scope(&world,t.boss.id(),json!({"rules":[{"target":{"kind":"org","id":t.acme,"reach":"subtree"},"allow":[{"operation":"UsageCapCreate","actions":["manage","read"]}]}]})).await;
    let mut input = cap_input(Some(t.topic), None);
    input.amount = Some("1700".into());
    let reply = world
        .app
        .call(UsageCapCreate, &[])
        .token(&selected)
        .body(&input)
        .send_bytes()
        .await;
    assert_eq!(
        reply.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&reply.raw)
    );
    let full: orch_api_contract::usage::UsageCap = serde_json::from_slice(&reply.raw).unwrap();
    assert_eq!(full.note, "cap content sentinel");
    assert_eq!(full.amount.as_deref(), Some("1700"));
    assert_eq!(full.node.unwrap().id, t.topic);
    let provider_only = key(
        &world,
        t.boss.id(),
        "provider",
        t.provider,
        "UsageCapCreate",
        "manage",
    )
    .await;
    assert_eq!(
        world
            .app
            .call(UsageCapCreate, &[])
            .token(&provider_only)
            .body(&cap_input(Some(t.project), Some(t.provider)))
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied,
        "a provider cap's named node remains an actual native View prerequisite"
    );
}
#[tokio::test]
async fn cap_updates_use_the_locked_stored_home_and_bound_stale_revision_content() {
    use orch_api_contract::usage::UsageCapUpdate;
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    let id = cap(&world, &t).await;
    let manage = key(
        &world,
        t.boss.id(),
        "project",
        t.project,
        "UsageCapUpdate",
        "manage",
    )
    .await;
    let changed = world
        .app
        .call(UsageCapUpdate, &[&id])
        .token(&manage)
        .raw_body(&json!({"note":"accepted changed note","expected_revision":1}))
        .send_bytes()
        .await;
    assert_eq!(
        changed.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&changed.raw)
    );
    let changed: Json = serde_json::from_slice(&changed.raw).unwrap();
    assert_eq!(
        changed,
        json!({"projection":"receipt","id":id,"revision":2})
    );
    let row = orch_db::usage::caps::get(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.note, "accepted changed note");
    assert_eq!(row.revision, 2);
    let stale = world
        .app
        .call(UsageCapUpdate, &[&id])
        .token(&manage)
        .raw_body(&json!({"note":"should not apply","expected_revision":1}))
        .send_bytes()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert!(stale.extra.get("current").is_none());
    let full=key_with_scope(&world,t.boss.id(),json!({"rules":[{"target":{"kind":"org","id":t.acme,"reach":"subtree"},"allow":[{"operation":"UsageCapUpdate","actions":["manage","read"]}]}]})).await;
    let stale = world
        .app
        .call(UsageCapUpdate, &[&id])
        .token(&full)
        .raw_body(&json!({"note":"should not apply","expected_revision":1}))
        .send_bytes()
        .await
        .problem();
    assert_eq!(stale.code, ErrorCode::ConflictStaleRevision);
    assert_eq!(stale.extra["current"]["note"], "accepted changed note");
    let wrong = key(
        &world,
        t.boss.id(),
        "project",
        id,
        "UsageCapUpdate",
        "manage",
    )
    .await;
    assert_eq!(
        world
            .app
            .call(UsageCapUpdate, &[&id])
            .token(&wrong)
            .raw_body(&json!({"note":"wrong home","expected_revision":2}))
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}

async fn enqueue(world: &World, t: &usage_world::Team) {
    let mut conn = world.conn().await;
    let agent = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut conn,
        "agent",
        Some(t.boss.id()),
        Some(t.acme),
        "Native queue agent",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE threads SET provider_id=$2,agent_id=$3,state='blocked',blocked_reason='quota_exceeded',blocked_detail='full' WHERE node_id=$1").bind(t.thread).bind(t.provider).bind(agent).execute(&mut *conn).await.unwrap();
    sqlx::query(
        "INSERT INTO settings(node_id,key,value) VALUES($1,'agent.model','\"gpt-test\"'::jsonb)",
    )
    .bind(t.thread)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query("INSERT INTO thread_inputs(id,thread_id,author_id,source,content,delivery,state,position,seq_updated,created_at) VALUES($1,$2,$3,'user','[]','queue','pending',1,1,$4)").bind(new_id()).bind(t.thread).bind(t.viewer.id()).bind(world.now()-chrono::Duration::minutes(10)).execute(&mut *conn).await.unwrap();
}
#[tokio::test]
async fn cap_queue_keeps_anonymous_positions_and_binds_native_thread_labels_separately() {
    use orch_api_contract::usage::UsageCapWaitingList;
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    let input = CapInput {
        meter: CapMeter::Running,
        period: None,
        amount: Some("0".into()),
        ..cap_input(Some(t.project), None)
    };
    let cap = world
        .app
        .call(UsageCapCreate, &[])
        .token(&t.boss.token)
        .body(&input)
        .send_bytes()
        .await;
    assert_eq!(cap.status, StatusCode::CREATED);
    let cap: Json = serde_json::from_slice(&cap.raw).unwrap();
    let id: Id = serde_json::from_value(cap["id"].clone()).unwrap();
    enqueue(&world, &t).await;
    let ordinary = value(
        world
            .app
            .call(UsageCapWaitingList, &[&id])
            .token(&t.boss.token)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary["items"].as_array().unwrap().len(), 1);
    assert_eq!(ordinary["items"][0]["thread"]["id"], json!(t.thread));
    let home_only = key(
        &world,
        t.boss.id(),
        "project",
        t.project,
        "UsageCapWaitingList",
        "read",
    )
    .await;
    let narrowed = value(
        world
            .app
            .call(UsageCapWaitingList, &[&id])
            .token(&home_only)
            .send_bytes()
            .await,
    );
    assert_eq!(narrowed["items"].as_array().unwrap().len(), 1);
    assert!(narrowed["items"][0]["thread"].is_null());
    assert_eq!(narrowed["items"][0]["author"]["id"], json!(t.viewer.id()));
    assert_eq!(narrowed["items"][0]["since"], ordinary["items"][0]["since"]);
    let full = org_key(&world, &t, "UsageCapWaitingList").await;
    assert_eq!(
        value(
            world
                .app
                .call(UsageCapWaitingList, &[&id])
                .token(&full)
                .send_bytes()
                .await
        ),
        ordinary
    );
}

// Reuse the 30-second database-lock premise budget of the other native boundary controls.
const WAIT_NATIVE: std::time::Duration = std::time::Duration::from_secs(30);
async fn blocked<T>(world: &World, pid: i32, action: &tokio::task::JoinHandle<T>, pattern: &str) {
    let observed=tokio::time::timeout(WAIT_NATIVE,async {
        loop {
            if action.is_finished(){return false}
            let found:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2)").bind(pid).bind(pattern).fetch_one(&world.db.pool).await.unwrap();
            if found{return true} tokio::task::yield_now().await;
        }
    }).await.unwrap();
    assert!(
        observed,
        "the actual native lock must be reached: {pattern}"
    );
}
#[tokio::test]
async fn cap_effects_recheck_the_actual_key_after_native_identity_and_row_waits() {
    use orch_api_contract::usage::UsageCapUpdate;
    for update in [false, true] {
        let world = World::new().await;
        let t = usage_world::team(&world).await;
        let id = if update {
            Some(cap(&world, &t).await)
        } else {
            None
        };
        let op = if update {
            "UsageCapUpdate"
        } else {
            "UsageCapCreate"
        };
        let secret = key(&world, t.boss.id(), "project", t.project, op, "manage").await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        if let Some(id) = id {
            sqlx::query("SELECT id FROM usage_caps WHERE id=$1 FOR UPDATE")
                .bind(id)
                .execute(&mut *blocker)
                .await
                .unwrap();
        } else {
            // Hold the native insert's exact footprint, including service and purpose filters.
            orch_db::usage::caps::lock_insert(
                &mut blocker,
                &orch_db::usage::caps::NewCap {
                    id: new_id(),
                    node_id: Some(t.project),
                    provider_id: None,
                    subject_kind: "everyone",
                    subject_id: None,
                    subject_role: None,
                    split: "shared",
                    models: &[],
                    services: &[],
                    purposes: &[],
                    tiers: &[],
                    harness_kinds: &[],
                    meter: "tokens",
                    period: Some("day"),
                    counted_from: None,
                    amount: Some("1000"),
                    at_limit: "stop",
                    warn_percent: None,
                    uncounted: "wait",
                    note: "cap content sentinel",
                    created_by: t.boss.id(),
                    at: world.now(),
                },
            )
            .await
            .unwrap();
        }
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let project = t.project;
        let action = tokio::spawn(async move {
            if let Some(id) = id {
                let call = app.call(UsageCapUpdate, &[&id]);
                call.token(&secret)
                    .raw_body(&json!({"note":"must not apply","expected_revision":1}))
                    .send_bytes()
                    .await
            } else {
                let request = app
                    .call(UsageCapCreate, &[])
                    .token(&secret)
                    .body(&cap_input(Some(project), None));
                request.send_bytes().await
            }
        });
        blocked(
            &world,
            pid,
            &action,
            if update {
                "%usage_caps%FOR UPDATE%"
            } else {
                "%pg_advisory_xact_lock%"
            },
        )
        .await;
        world.advance(chrono::Duration::hours(2));
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT_NATIVE, action)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
        assert!(!String::from_utf8_lossy(&reply.raw).contains("cap content sentinel"));
        if let Some(id) = id {
            let row = orch_db::usage::caps::get(&mut *world.conn().await, id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(row.revision, 1);
            assert_eq!(row.note, "cap content sentinel");
        } else {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM usage_caps WHERE node_id=$1")
                .bind(t.project)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
            assert_eq!(count, 0);
        }
    }
}
#[tokio::test]
async fn cap_publication_after_activity_wait_keeps_the_committed_effect_and_exact_receipt() {
    for expire in [false, true] {
        let world = World::new().await;
        let t = usage_world::team(&world).await;
        let secret=key_with_scope(&world,t.boss.id(),json!({"rules":[{"target":{"kind":"org","id":t.acme,"reach":"subtree"},"allow":[{"operation":"UsageCapCreate","actions":["manage","read"]}]}]})).await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let project = t.project;
        let action = tokio::spawn(async move {
            let request = app
                .call(UsageCapCreate, &[])
                .token(&secret)
                .body(&cap_input(Some(project), None));
            request.send_bytes().await
        });
        blocked(&world, pid, &action, "%INSERT INTO api_token_activity%").await;
        let (id, before): (Id, String) =
            sqlx::query_as("SELECT id,xmin::text FROM usage_caps WHERE node_id=$1")
                .bind(t.project)
                .fetch_one(&world.db.pool)
                .await
                .unwrap();
        if expire {
            world.advance(chrono::Duration::hours(2));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT_NATIVE, action)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&reply.raw)
        );
        let reply: Json = serde_json::from_slice(&reply.raw).unwrap();
        if expire {
            assert_eq!(reply, json!({"projection":"receipt","id":id,"revision":1}));
        } else {
            assert_eq!(reply["id"], json!(id));
            assert_eq!(reply["note"], "cap content sentinel");
        }
        let after: String = sqlx::query_scalar("SELECT xmin::text FROM usage_caps WHERE id=$1")
            .bind(id)
            .fetch_one(&world.db.pool)
            .await
            .unwrap();
        assert_eq!(
            after, before,
            "header publication never replays the committed cap writer"
        );
    }
}

#[tokio::test]
async fn cap_provider_updates_keep_manage_only_when_the_stored_named_node_is_unchanged() {
    use orch_api_contract::usage::UsageCapUpdate;
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    let created = world
        .app
        .call(UsageCapCreate, &[])
        .token(&t.boss.token)
        .body(&cap_input(Some(t.project), Some(t.provider)))
        .send_bytes()
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let created: Json = serde_json::from_slice(&created.raw).unwrap();
    let id: Id = serde_json::from_value(created["id"].clone()).unwrap();
    let manage = key(
        &world,
        t.boss.id(),
        "provider",
        t.provider,
        "UsageCapUpdate",
        "manage",
    )
    .await;
    let reply = value(
        world
            .app
            .call(UsageCapUpdate, &[&id])
            .token(&manage)
            .raw_body(&json!({"amount":"1800","expected_revision":1}))
            .send_bytes()
            .await,
    );
    assert_eq!(reply, json!({"projection":"receipt","id":id,"revision":2}));
    let row = orch_db::usage::caps::get(&mut *world.conn().await, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.node_id, Some(t.project));
    assert_eq!(row.provider_id, Some(t.provider));
    assert_eq!(row.amount.as_deref(), Some("1800"));
}
#[tokio::test]
async fn system_cap_deletion_keeps_current_administration_without_private_admin_mode() {
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    let made = world
        .app
        .call(UsageCapCreate, &[])
        .token(&t.sysadmin.token)
        .body(&cap_input(None, None))
        .send_bytes()
        .await;
    assert_eq!(made.status, StatusCode::CREATED);
    let made: Json = serde_json::from_slice(&made.raw).unwrap();
    let id: Id = serde_json::from_value(made["id"].clone()).unwrap();
    let rule = json!({"rules":[{"target":{"kind":"system","reach":"self"},"allow":[{"operation":"UsageCapDelete","actions":["manage"]}]}]});
    let unprivileged = key_with_scope(&world, t.boss.id(), rule.clone()).await;
    assert_eq!(
        world
            .app
            .call(UsageCapDelete, &[&id])
            .token(&unprivileged)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let administrator = key_with_scope(&world, t.sysadmin.id(), rule).await;
    let reply = world
        .app
        .call(UsageCapDelete, &[&id])
        .token(&administrator)
        .send_bytes()
        .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert!(reply.raw.is_empty());
}

#[tokio::test]
async fn cap_list_publication_cannot_replace_the_original_cap_with_another_at_the_same_home() {
    use orch_api_contract::usage::{UsageCapList, UsageCapListQuery};
    for replace in [false, true] {
        let world = World::new().await;
        let t = usage_world::team(&world).await;
        let original = cap(&world, &t).await;
        let secret = org_key(&world, &t, "UsageCapList").await;
        let mut blocker = world.db.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
            .execute(&mut *blocker)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let app = world.app.clone();
        let project = t.project;
        let action = tokio::spawn(async move {
            let request = app
                .call(UsageCapList, &[])
                .token(&secret)
                .query(&UsageCapListQuery {
                    node_id: Some(project),
                    provider_id: None,
                    system: None,
                    covering: None,
                });
            request.send_bytes().await
        });
        blocked(&world, pid, &action, "%INSERT INTO api_token_activity%").await;
        if replace {
            world
                .app
                .call(UsageCapDelete, &[&original])
                .token(&t.boss.token)
                .send()
                .await
                .ok();
            let replacement = world
                .app
                .call(UsageCapCreate, &[])
                .token(&t.boss.token)
                .body(&CapInput {
                    note: Some("replacement cap sentinel".into()),
                    ..cap_input(Some(t.project), None)
                })
                .send_bytes()
                .await;
            assert_eq!(replacement.status, StatusCode::CREATED);
            let replacement: Json = serde_json::from_slice(&replacement.raw).unwrap();
            assert_ne!(replacement["id"], json!(original));
        }
        blocker.commit().await.unwrap();
        let reply = tokio::time::timeout(WAIT_NATIVE, action)
            .await
            .unwrap()
            .unwrap();
        if replace {
            assert_eq!(
                reply.status,
                StatusCode::FORBIDDEN,
                "a replacement cap at the same Home cannot validate the old cap body"
            );
            assert!(!String::from_utf8_lossy(&reply.raw).contains("cap content sentinel"));
        } else {
            let reply = value(reply);
            assert_eq!(reply[0]["cap"]["id"], json!(original));
            assert_eq!(reply[0]["cap"]["note"], "cap content sentinel");
        }
    }
}

#[tokio::test]
async fn cap_validation_preserves_native_authorization_before_body_faults() {
    use orch_api_contract::usage::UsageCapUpdate;
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    let id = cap(&world, &t).await;
    let bad = CapInput {
        meter: CapMeter::Running,
        ..cap_input(Some(t.project), None)
    };
    assert_eq!(
        world
            .app
            .call(UsageCapCreate, &[])
            .token(&t.stranger.token)
            .body(&bad)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        world
            .app
            .call(UsageCapCreate, &[])
            .token(&t.boss.token)
            .body(&bad)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let create = key(
        &world,
        t.boss.id(),
        "project",
        t.project,
        "UsageCapCreate",
        "manage",
    )
    .await;
    assert_eq!(
        world
            .app
            .call(UsageCapCreate, &[])
            .token(&create)
            .body(&bad)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let empty = json!({"expected_revision":1});
    assert_eq!(
        world
            .app
            .call(UsageCapUpdate, &[&id])
            .token(&t.stranger.token)
            .raw_body(&empty)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    assert_eq!(
        world
            .app
            .call(UsageCapUpdate, &[&id])
            .token(&t.boss.token)
            .raw_body(&empty)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
    let update = key(
        &world,
        t.boss.id(),
        "project",
        t.project,
        "UsageCapUpdate",
        "manage",
    )
    .await;
    assert_eq!(
        world
            .app
            .call(UsageCapUpdate, &[&id])
            .token(&update)
            .raw_body(&empty)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::RequestInvalid
    );
}

#[tokio::test]
async fn write_only_cap_validation_does_not_publish_existing_meter_details() {
    use orch_api_contract::usage::UsageCapUpdate;
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    let made = world
        .app
        .call(UsageCapCreate, &[])
        .token(&t.boss.token)
        .body(&CapInput {
            meter: CapMeter::Running,
            period: None,
            amount: Some("0".into()),
            ..cap_input(Some(t.project), None)
        })
        .send_bytes()
        .await;
    assert_eq!(made.status, StatusCode::CREATED);
    let made: Json = serde_json::from_slice(&made.raw).unwrap();
    let id: Id = serde_json::from_value(made["id"].clone()).unwrap();
    let body = json!({"at_limit":"stop","expected_revision":1});
    let ordinary = world
        .app
        .call(UsageCapUpdate, &[&id])
        .token(&t.boss.token)
        .raw_body(&body)
        .send_bytes()
        .await
        .problem();
    assert_eq!(ordinary.code, ErrorCode::RequestInvalid);
    assert!(ordinary.extra.get("errors").is_some());
    let manage = key(
        &world,
        t.boss.id(),
        "project",
        t.project,
        "UsageCapUpdate",
        "manage",
    )
    .await;
    let reply = world
        .app
        .call(UsageCapUpdate, &[&id])
        .token(&manage)
        .raw_body(&body)
        .send_bytes()
        .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert!(!String::from_utf8_lossy(&reply.raw).contains("running"));
    assert!(reply.problem().extra.is_empty());
}

async fn private_usage_thread(world: &World, t: &usage_world::Team) {
    let mut conn = world.conn().await;
    // This raw hierarchy fixture has no creation page until its actual maker starts it.
    orch_authz::pages::start(
        &mut conn,
        orch_access_contract::pages::StoredKind::Node,
        t.thread,
        t.boss.id(),
        world.now().into(),
    )
    .await
    .unwrap();
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(t.boss.id()),
        world.now().into(),
    )
    .await
    .unwrap();
    let change = serde_json::from_value(json!({"whole":"private"})).unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    orch_authz::pages::change(
        &mut tx,
        &principal,
        orch_authz::pages::Target::Live(t.thread),
        &change,
        orch_authz::Policy::default(),
        &orch_db::audit::Origin::default(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}
fn usage_query(scope: String) -> orch_api_contract::usage::UsageGetQuery {
    serde_json::from_value(
        json!({"scope":scope,"from":usage_world::FROM,"to":usage_world::TODAY,"bucket":"day"}),
    )
    .unwrap()
}
fn event_query(scope: String) -> orch_api_contract::usage::UsageEventListQuery {
    serde_json::from_value(
        json!({"scope":scope,"from":usage_world::FROM,"to":usage_world::TODAY,"limit":1}),
    )
    .unwrap()
}

#[tokio::test]
async fn system_usage_requires_current_administration_and_each_exact_operations_system_read() {
    use orch_api_contract::usage::{UsageEventList, UsageGet};
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    private_usage_thread(&world, &t).await;
    usage_world::spend(&world, &t, &usage_world::Use::of(&world, t.boss.id(), 70)).await;
    let rules = |op: &str, action: &str| json!({"rules":[{"target":{"kind":"system","reach":"self"},"allow":[{"operation":op,"actions":[action]}]}]});
    let key = key_with_scope(&world, t.sysadmin.id(), rules("UsageGet", "read")).await;
    let response = world
        .app
        .call(UsageGet, &[])
        .token(&key)
        .query(&usage_query("system".into()))
        .send()
        .await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    assert_eq!(response.ok().totals.tokens.total_tokens, 70);
    let wrong = world
        .app
        .call(UsageEventList, &[])
        .token(&key)
        .query(&event_query("system".into()))
        .send()
        .await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    let events_key = key_with_scope(&world, t.sysadmin.id(), rules("UsageEventList", "read")).await;
    let response = world
        .app
        .call(UsageEventList, &[])
        .token(&events_key)
        .query(&event_query("system".into()))
        .send()
        .await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    let record = response.ok().items.into_iter().next().unwrap();
    assert_eq!(record.tokens.total_tokens, 70);
    assert!(
        record.thread.as_ref().is_none_or(|thread| thread.redacted),
        "system statistics do not give a key the browser's private-page override"
    );
    for (user, op, action) in [
        (t.boss.id(), "UsageGet", "read"),
        (t.sysadmin.id(), "UsageGet", "write"),
        (t.sysadmin.id(), "UsageEventList", "read"),
    ] {
        let secret = key_with_scope(&world, user, rules(op, action)).await;
        let result = world
            .app
            .call(UsageGet, &[])
            .token(&secret)
            .query(&usage_query("system".into()))
            .send()
            .await;
        assert!(
            matches!(result.status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
            "{:?}",
            result.body
        );
    }
    sqlx::query("UPDATE users SET is_system_admin=false WHERE id=$1")
        .bind(t.sysadmin.id())
        .execute(&world.db.pool)
        .await
        .unwrap();
    assert_eq!(
        world
            .app
            .call(UsageGet, &[])
            .token(&key)
            .query(&usage_query("system".into()))
            .send()
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn usage_subject_read_keeps_native_aggregate_totals_without_ledger_thread_key_targets() {
    use orch_api_contract::usage::UsageGet;
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    private_usage_thread(&world, &t).await;
    usage_world::spend(&world, &t, &usage_world::Use::of(&world, t.viewer.id(), 40)).await;
    usage_world::spend(&world, &t, &usage_world::Use::of(&world, t.admin.id(), 100)).await;
    let query = usage_query(format!("node:{}", t.project));
    for user in [&t.boss, &t.viewer] {
        let ordinary = value(
            world
                .app
                .call(UsageGet, &[])
                .token(&user.token)
                .query(&query)
                .send_bytes()
                .await,
        );
        assert_eq!(ordinary["totals"]["tokens"]["total_tokens"], 140);
        let selected = key(&world, user.id(), "project", t.project, "UsageGet", "read").await;
        assert_eq!(
            value(
                world
                    .app
                    .call(UsageGet, &[])
                    .token(&selected)
                    .query(&query)
                    .send_bytes()
                    .await
            ),
            ordinary,
            "the native scope governs aggregates, including the person's own privacy/depth cut"
        );
        let expected = world
            .app
            .call(UsageGet, &[])
            .token(&user.token)
            .header("accept", "text/csv")
            .query(&query)
            .send_bytes()
            .await;
        let actual = world
            .app
            .call(UsageGet, &[])
            .token(&selected)
            .header("accept", "text/csv")
            .query(&query)
            .send_bytes()
            .await;
        assert_eq!(actual.status, StatusCode::OK);
        assert_eq!(actual.raw, expected.raw);
    }
    let wrong = key(
        &world,
        t.boss.id(),
        "project",
        t.project,
        "ProjectGet",
        "read",
    )
    .await;
    assert_eq!(
        world
            .app
            .call(UsageGet, &[])
            .token(&wrong)
            .query(&query)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}
#[tokio::test]
async fn usage_own_account_and_provider_scopes_keep_native_projection_and_authority() {
    use orch_api_contract::usage::UsageGet;
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    usage_world::spend(&world, &t, &usage_world::Use::of(&world, t.viewer.id(), 40)).await;
    usage_world::spend(&world, &t, &usage_world::Use::of(&world, t.admin.id(), 100)).await;
    let mine = usage_query("me".into());
    let account = key(
        &world,
        t.viewer.id(),
        "account",
        t.viewer.id(),
        "UsageGet",
        "read",
    )
    .await;
    let ordinary = value(
        world
            .app
            .call(UsageGet, &[])
            .token(&t.viewer.token)
            .query(&mine)
            .send_bytes()
            .await,
    );
    assert_eq!(ordinary["totals"]["tokens"]["total_tokens"], 40);
    assert_eq!(
        value(
            world
                .app
                .call(UsageGet, &[])
                .token(&account)
                .query(&mine)
                .send_bytes()
                .await
        ),
        ordinary
    );
    let other = key(
        &world,
        t.viewer.id(),
        "account",
        t.admin.id(),
        "UsageGet",
        "read",
    )
    .await;
    assert_eq!(
        world
            .app
            .call(UsageGet, &[])
            .token(&other)
            .query(&mine)
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
    let provider = usage_query(format!("provider:{}", t.provider));
    let selected = key(
        &world,
        t.boss.id(),
        "provider",
        t.provider,
        "UsageGet",
        "read",
    )
    .await;
    let expected = value(
        world
            .app
            .call(UsageGet, &[])
            .token(&t.boss.token)
            .query(&provider)
            .send_bytes()
            .await,
    );
    assert_eq!(
        value(
            world
                .app
                .call(UsageGet, &[])
                .token(&selected)
                .query(&provider)
                .send_bytes()
                .await
        ),
        expected
    );
    let stranger = key(
        &world,
        t.stranger.id(),
        "project",
        t.project,
        "UsageGet",
        "read",
    )
    .await;
    assert_eq!(
        world
            .app
            .call(UsageGet, &[])
            .token(&stranger)
            .query(&usage_query(format!("node:{}", t.project)))
            .send_bytes()
            .await
            .problem()
            .code,
        ErrorCode::AuthzDenied
    );
}
#[tokio::test]
async fn usage_events_keep_native_privacy_keysets_and_csv_under_the_subject_read() {
    use orch_api_contract::usage::UsageEventList;
    let world = World::new().await;
    let t = usage_world::team(&world).await;
    private_usage_thread(&world, &t).await;
    usage_world::spend(&world, &t, &usage_world::Use::of(&world, t.viewer.id(), 40)).await;
    usage_world::spend(
        &world,
        &t,
        &usage_world::Use::of(&world, t.admin.id(), 100)
            .at(world.now() - chrono::Duration::hours(2)),
    )
    .await;
    let query = event_query(format!("node:{}", t.project));
    let full = value(
        world
            .app
            .call(UsageEventList, &[])
            .token(&t.boss.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    assert_eq!(full["items"].as_array().unwrap().len(), 1);
    assert!(full["next_cursor"].is_string());
    for user in [&t.boss, &t.viewer] {
        let ordinary = value(
            world
                .app
                .call(UsageEventList, &[])
                .token(&user.token)
                .query(&query)
                .send_bytes()
                .await,
        );
        let selected = key(
            &world,
            user.id(),
            "project",
            t.project,
            "UsageEventList",
            "read",
        )
        .await;
        assert_eq!(
            value(
                world
                    .app
                    .call(UsageEventList, &[])
                    .token(&selected)
                    .query(&query)
                    .send_bytes()
                    .await
            ),
            ordinary
        );
        let expected = world
            .app
            .call(UsageEventList, &[])
            .token(&user.token)
            .header("accept", "text/csv")
            .query(&query)
            .send_bytes()
            .await;
        let actual = world
            .app
            .call(UsageEventList, &[])
            .token(&selected)
            .header("accept", "text/csv")
            .query(&query)
            .send_bytes()
            .await;
        assert_eq!(actual.status, StatusCode::OK);
        assert_eq!(actual.raw, expected.raw);
        assert_eq!(
            actual.headers.get("x-next-cursor"),
            expected.headers.get("x-next-cursor")
        );
    }
    let viewer = value(
        world
            .app
            .call(UsageEventList, &[])
            .token(&t.viewer.token)
            .query(&query)
            .send_bytes()
            .await,
    );
    let own = viewer["items"].as_array().unwrap();
    assert_eq!(
        own.len(),
        1,
        "a summary reader keeps their own billing record even after losing the thread"
    );
    assert_eq!(own[0]["user"]["id"], json!(t.viewer.id()));
    assert_eq!(own[0]["tokens"]["total_tokens"], 40);
    assert!(
        own[0]["thread"].is_null() || own[0]["thread"]["redacted"] == true,
        "retaining one's own billing data grants no private thread label"
    );
    // Provider viewing has a different native boundary: hidden places stay counted,
    // but only the provider's manager may list their records without ordinary View.
    resources::name_person(
        &world,
        "provider",
        t.provider,
        "provider.view",
        t.viewer.id(),
    )
    .await;
    let provider_query = event_query(format!("provider:{}", t.provider));
    let provider_key = key(
        &world,
        t.viewer.id(),
        "provider",
        t.provider,
        "UsageEventList",
        "read",
    )
    .await;
    let ordinary_hidden = value(
        world
            .app
            .call(UsageEventList, &[])
            .token(&t.viewer.token)
            .query(&provider_query)
            .send_bytes()
            .await,
    );
    assert!(ordinary_hidden["items"].as_array().unwrap().is_empty());
    assert!(
        ordinary_hidden["next_cursor"].is_null(),
        "hidden rows do not leak a keyset cursor"
    );
    assert_eq!(
        value(
            world
                .app
                .call(UsageEventList, &[])
                .token(&provider_key)
                .query(&provider_query)
                .send_bytes()
                .await
        ),
        ordinary_hidden
    );
    let ordinary_csv = world
        .app
        .call(UsageEventList, &[])
        .token(&t.viewer.token)
        .header("accept", "text/csv")
        .query(&provider_query)
        .send_bytes()
        .await;
    let key_csv = world
        .app
        .call(UsageEventList, &[])
        .token(&provider_key)
        .header("accept", "text/csv")
        .query(&provider_query)
        .send_bytes()
        .await;
    assert_eq!(key_csv.status, StatusCode::OK);
    assert_eq!(key_csv.raw, ordinary_csv.raw);
    assert_eq!(
        key_csv.headers.get("x-next-cursor"),
        ordinary_csv.headers.get("x-next-cursor")
    );
    let selected = key(
        &world,
        t.boss.id(),
        "project",
        t.project,
        "UsageEventList",
        "read",
    )
    .await;
    let mut next = query.clone();
    next.cursor = full["next_cursor"].as_str().map(str::to_owned);
    let expected = value(
        world
            .app
            .call(UsageEventList, &[])
            .token(&t.boss.token)
            .query(&next)
            .send_bytes()
            .await,
    );
    assert_eq!(
        value(
            world
                .app
                .call(UsageEventList, &[])
                .token(&selected)
                .query(&next)
                .send_bytes()
                .await
        ),
        expected
    );
}

async fn remove_usage_grant(
    world: &World,
    t: &usage_world::Team,
    node: Id,
    permission: &str,
    entry: Id,
) {
    let mut conn = world.conn().await;
    let principal = orch_authz::Principal::load(
        &mut conn,
        orch_authz::Caller::session(t.boss.id()),
        world.now().into(),
    )
    .await
    .unwrap();
    let change =
        serde_json::from_value(json!({"rows":[{"permission":permission,"remove":[entry]}]}))
            .unwrap();
    let mut tx = world.db.pool.begin().await.unwrap();
    let removed = orch_authz::pages::change(
        &mut tx,
        &principal,
        orch_authz::pages::Target::Live(node),
        &change,
        orch_authz::Policy::default(),
        &orch_db::audit::Origin::default(),
    )
    .await
    .unwrap();
    assert!(removed.changed);
    tx.commit().await.unwrap();
}
#[tokio::test]
async fn usage_publication_preserves_the_original_reader_depth_and_record_visibility() {
    use orch_api_contract::usage::{UsageEventList, UsageGet};
    for records in [false, true] {
        for narrow in [false, true] {
            let world = World::new().await;
            let t = usage_world::team(&world).await;
            private_usage_thread(&world, &t).await;
            usage_world::spend(&world, &t, &usage_world::Use::of(&world, t.viewer.id(), 40)).await;
            usage_world::spend(&world, &t, &usage_world::Use::of(&world, t.admin.id(), 100)).await;
            let (node, permission) = if records {
                (t.thread, "thread.view")
            } else {
                (t.project, "usage.view")
            };
            let grant =
                resources::name_person(&world, "node", node, permission, t.viewer.id()).await;
            if records {
                resources::name_person(
                    &world,
                    "provider",
                    t.provider,
                    "provider.view",
                    t.viewer.id(),
                )
                .await;
            }
            let op = if records {
                "UsageEventList"
            } else {
                "UsageGet"
            };
            let (kind, subject, scope) = if records {
                ("provider", t.provider, format!("provider:{}", t.provider))
            } else {
                ("project", t.project, format!("node:{}", t.project))
            };
            let secret = key(&world, t.viewer.id(), kind, subject, op, "read").await;
            if records {
                let control = value(
                    world
                        .app
                        .call(UsageEventList, &[])
                        .token(&t.viewer.token)
                        .query(&event_query(scope.clone()))
                        .send_bytes()
                        .await,
                );
                assert_eq!(control["items"].as_array().unwrap().len(), 1);
            } else {
                let control = value(
                    world
                        .app
                        .call(UsageGet, &[])
                        .token(&t.viewer.token)
                        .query(&usage_query(scope.clone()))
                        .send_bytes()
                        .await,
                );
                assert_eq!(control["depth"], "reports");
                assert_eq!(control["totals"]["tokens"]["total_tokens"], 140);
            }
            let mut blocker = world.db.pool.begin().await.unwrap();
            sqlx::query("LOCK TABLE api_token_activity IN SHARE MODE")
                .execute(&mut *blocker)
                .await
                .unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let app = world.app.clone();
            let asked = scope.clone();
            let action = tokio::spawn(async move {
                if records {
                    let request = app
                        .call(UsageEventList, &[])
                        .token(&secret)
                        .query(&event_query(asked));
                    request.send_bytes().await
                } else {
                    let request = app
                        .call(UsageGet, &[])
                        .token(&secret)
                        .query(&usage_query(asked));
                    request.send_bytes().await
                }
            });
            blocked(&world, pid, &action, "%INSERT INTO api_token_activity%").await;
            if narrow {
                remove_usage_grant(&world, &t, node, permission, grant).await;
                if records {
                    let now = value(
                        world
                            .app
                            .call(UsageEventList, &[])
                            .token(&t.viewer.token)
                            .query(&event_query(scope))
                            .send_bytes()
                            .await,
                    );
                    assert!(now["items"].as_array().unwrap().is_empty());
                } else {
                    let now = value(
                        world
                            .app
                            .call(UsageGet, &[])
                            .token(&t.viewer.token)
                            .query(&usage_query(scope))
                            .send_bytes()
                            .await,
                    );
                    assert_eq!(now["depth"], "summary");
                }
            }
            blocker.commit().await.unwrap();
            let reply = tokio::time::timeout(WAIT_NATIVE, action)
                .await
                .unwrap()
                .unwrap();
            if narrow {
                assert_eq!(
                    reply.status,
                    StatusCode::FORBIDDEN,
                    "a newly reduced native projection cannot authorize the earlier body"
                );
            } else {
                let reply = value(reply);
                if records {
                    assert_eq!(reply["items"].as_array().unwrap().len(), 1);
                } else {
                    assert_eq!(reply["depth"], "reports");
                    assert_eq!(reply["totals"]["tokens"]["total_tokens"], 140);
                }
            }
        }
    }
}
