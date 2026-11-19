//! Real key boundaries use current typed inputs and placement at the shared checkpoints.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod support;

use axum::body::Body;
use http::{Request, StatusCode};
use orch_access_contract::key_scope::{KeyObjectRef, KeyResolvedTarget, KeyScope, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::nodes::{DeletedInspectionQuery, NodeGet};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_api_core::middleware::requests::ApiError;
use orch_api_core::middleware::{authorization, transaction_guard};
use orch_api_core::registry::{Ctx, OperationFuture, OperationPlan, Registry};
use orch_db::queries::{access, hierarchy, identity};
use orch_types::{Id, new_id};
use serde::Deserialize;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use support::World;
use tower::ServiceExt;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeProbeBody {
    node: Id,
}

orch_api_contract::endpoints! {
    /// A test operation whose real destination is selected by its typed body.
    NodeOperationProbe: POST "/operation-node-probe", orch_api_contract::routes::Access::User,
        none, json NodeProbeBody, none, 204,
        operation orch_api_contract::operations::OperationSpec::single(
            orch_access_contract::key_scope::KeyAction::Write,
            &[KeyTargetKind::Object(ObjectKind::Topic)],
            orch_api_contract::operations::TargetResolver::Resolved("destination"));
    /// A normal static permission-object read, with no registered resolver callback.
    NodeStaticReadProbe: GET "/topics/{id}/operation-static-read-probe",
        orch_api_contract::routes::Access::Permission("topic.view"),
        none, none, json serde_json::Value, 200,
        operation orch_api_contract::operations::OperationSpec::single(
            orch_access_contract::key_scope::KeyAction::Read,
            &[KeyTargetKind::Object(ObjectKind::Topic)],
            orch_api_contract::operations::TargetResolver::PermissionObject);
}

fn node_probe<'a>(
    ctx: &'a Ctx,
    conn: &'a mut orch_db::Conn,
    _: &'a orch_authz::Caller,
    (): (),
    body: NodeProbeBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let graph = orch_authz::engine::Graph::load(conn, &[body.node])
            .await
            .map_err(ApiError::internal)?;
        let node = graph.get(body.node).ok_or_else(ApiError::not_found)?;
        if node.kind != orch_access_contract::roles::NodeKind::Topic {
            return Err(ApiError::not_found());
        }
        let mut ancestors = Vec::new();
        for id in &node.path {
            let node = graph.get(*id).ok_or_else(ApiError::not_found)?;
            ancestors.push(KeyObjectRef {
                kind: KeyTargetKind::Object(node.kind.into()),
                id: Some(*id),
            });
        }
        let target = KeyResolvedTarget {
            object: KeyObjectRef {
                kind: KeyTargetKind::Object(ObjectKind::Topic),
                id: Some(body.node),
            },
            node_ancestors: ancestors,
            document_ancestors: Vec::new(),
            credential_account: None,
        };
        let mut plan = OperationPlan::new(ctx.route, "NodeOperationProbe")?;
        plan.bind(0, target.object.kind, &[target])?;
        Ok(plan)
    })
}

async fn scoped_key(world: &World, user: Id, policy: serde_json::Value) -> (Id, String) {
    let scope: KeyScope = serde_json::from_value(policy).unwrap();
    assert!(scope.validate().is_empty());
    let value = serde_json::to_value(scope).unwrap();
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let id = new_id();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id,
            user_id: user,
            name: "Typed operation control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&value),
    )
    .await
    .unwrap();
    (id, secret)
}

#[tokio::test]
async fn a_fresh_guard_resolves_body_selected_placement_again_after_a_move() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let org = places::make_org(&world, &owner, "Workspace", "workspace").await;
    let before = places::make_project(&world, &owner, org.base.id, "Before").await;
    let after = places::make_project(&world, &owner, org.base.id, "After").await;
    let topic = places::make_topic(&world, &owner, before.base.id, "Work").await;
    let (_, secret) = scoped_key(
        &world,
        owner.id(),
        json!({"rules":[{
            "target":{"kind":"project","id":before.base.id,"reach":"subtree"},
            "resources":["topic"],"allow":[{"operation":"NodeOperationProbe","actions":["write"]}]
        }]}),
    )
    .await;
    let pause = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(tokio::sync::Notify::new());
    let continue_ = Arc::new(tokio::sync::Notify::new());
    let effects = Arc::new(AtomicUsize::new(0));
    let mut registry = Registry::new();
    let (paused, ready, continued, writes) = (
        pause.clone(),
        entered.clone(),
        continue_.clone(),
        effects.clone(),
    );
    registry.on_with_operation(
        NodeOperationProbe,
        node_probe,
        move |ctx, (), body: NodeProbeBody| {
            let (paused, ready, continued, writes) = (
                paused.clone(),
                ready.clone(),
                continued.clone(),
                writes.clone(),
            );
            async move {
                if paused.load(Ordering::SeqCst) {
                    ready.notify_one();
                    continued.notified().await;
                }
                let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
                authorization::require(
                    &mut tx,
                    guard.principal(),
                    body.node,
                    authorization::exact(Permission::TopicManage),
                    orch_authz::Act::Use,
                    orch_authz::Policy::default(),
                )
                .await?;
                assert!(
                    guard.principal().caller().grant().is_some(),
                    "the original key remains the caller"
                );
                writes.fetch_add(1, Ordering::SeqCst);
                tx.commit().await.map_err(orch_db::DbError::from)?;
                Ok::<(), ApiError>(())
            }
        },
    );
    let router = registry
        .into_router_for(orch_api_contract::surface::HttpSurface::Key)
        .with_state(world.app.app.clone());
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/operation-node-probe")
            .header("authorization", format!("Bearer {secret}"))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({"node":topic.base.id})).unwrap(),
            ))
            .unwrap()
    };
    assert_eq!(
        router.clone().oneshot(request()).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    pause.store(true, Ordering::SeqCst);
    let running = tokio::spawn(router.oneshot(request()));
    // Same bounded wait as the existing action-guard controls; no external work occurs.
    tokio::time::timeout(std::time::Duration::from_secs(30), entered.notified())
        .await
        .expect("the request reached its typed preparation checkpoint");
    // A separate actor's placement change commits after initial admission and before the
    // key's actual action guard. Both parents are real projects in the same existing tree.
    let mut moved = world.app.app.db.begin().await.unwrap();
    hierarchy::move_under(&mut moved, topic.base.id, after.base.id, world.now())
        .await
        .unwrap();
    access::bump_epoch(&mut moved).await.unwrap();
    moved.commit().await.unwrap();
    continue_.notify_one();
    let refused = running.await.unwrap().unwrap();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        effects.load(Ordering::SeqCst),
        1,
        "fresh placement refused before any effect"
    );
}

#[tokio::test]
async fn a_legacy_node_read_records_its_real_canonical_selection_and_safe_facet() {
    let world = World::new().await;
    let owner = places::person(&world, "reader").await;
    let org = places::make_org(&world, &owner, "Workspace", "workspace").await;
    let project = places::make_project(&world, &owner, org.base.id, "Visible").await;
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let token = new_id();
    identity::create_api_token(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: token,
            user_id: owner.id(),
            name: "Legacy read control",
            token_hash: &hash,
            scopes: &["project.view".to_owned()],
            node_id: Some(project.base.id),
            at: world.now(),
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
    )
    .await
    .unwrap();
    let reply = world
        .app
        .call(NodeGet, &[&project.base.id])
        .token(&secret)
        .query(&DeletedInspectionQuery::default())
        .send()
        .await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.body);
    let record: (String, Option<String>, serde_json::Value, bool) = sqlx::query_as(
        "SELECT operation,selection,facets,replayed FROM api_token_activity WHERE token_id=$1 ORDER BY at DESC,id DESC LIMIT 1")
        .bind(token).fetch_one(&world.db.pool).await.unwrap();
    assert_eq!(record.0, "NodeGet");
    assert_eq!(record.1.as_deref(), Some("NodeGet.read"));
    assert_eq!(record.2[0]["action"], "read");
    assert_eq!(record.2[0]["target"]["id"], json!(project.base.id));
    assert!(record.2[0].get("name").is_none());
    assert!(
        !record.3,
        "preparation/checkpoint repetition is not an idempotent replay"
    );
}

#[tokio::test]
async fn the_real_typed_node_reader_requires_both_the_exact_key_target_and_user_authority() {
    let world = World::new().await;
    let owner = places::person(&world, "owner").await;
    let outsider = places::person(&world, "outsider").await;
    let org = places::make_org(&world, &owner, "Workspace", "workspace").await;
    let selected = places::make_project(&world, &owner, org.base.id, "Selected").await;
    let other = places::make_project(&world, &owner, org.base.id, "Other").await;
    let policy = json!({"rules":[{
        "target":{"kind":"project", "id":selected.base.id, "reach":"self"},
        "allow":[{"operation":"NodeGet.read", "actions":["read"]}]
    }]});
    let (_, secret) = scoped_key(&world, owner.id(), policy.clone()).await;
    let read = world
        .app
        .call(NodeGet, &[&selected.base.id])
        .token(&secret)
        .query(&DeletedInspectionQuery::default())
        .send()
        .await;
    assert_eq!(read.status, StatusCode::OK, "{:?}", read.body);
    let outside = world
        .app
        .call(NodeGet, &[&other.base.id])
        .token(&secret)
        .query(&DeletedInspectionQuery::default())
        .send()
        .await;
    assert_eq!(
        outside.status,
        StatusCode::FORBIDDEN,
        "the owner's authority cannot enlarge this key"
    );
    let (_, secret) = scoped_key(&world, outsider.id(), policy).await;
    let no_authority = world
        .app
        .call(NodeGet, &[&selected.base.id])
        .token(&secret)
        .query(&DeletedInspectionQuery::default())
        .send()
        .await;
    assert!(
        !no_authority.status.is_success(),
        "a selected operation never grants the account access"
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StaticReadPolicyChange {
    Unchanged,
    ScopedToLegacy,
    LegacyToScoped,
}

#[tokio::test]
async fn a_static_read_rebinds_actual_placement_and_keeps_current_and_accepted_scope_on_conversion()
{
    for change in [
        StaticReadPolicyChange::Unchanged,
        StaticReadPolicyChange::ScopedToLegacy,
        StaticReadPolicyChange::LegacyToScoped,
    ] {
        let world = World::new().await;
        let owner = places::person(&world, "static-reader").await;
        let org = places::make_org(&world, &owner, "Workspace", "workspace").await;
        let before = places::make_project(&world, &owner, org.base.id, "Before").await;
        let after = places::make_project(&world, &owner, org.base.id, "After").await;
        let topic =
            places::make_topic(&world, &owner, before.base.id, "Current protected topic").await;
        let policy = json!({"rules":[{
            "target":{"kind":"project","id":before.base.id,"reach":"subtree"},
            "resources":["topic"],"allow":[{"operation":"NodeStaticReadProbe","actions":["read"]}]
        }]});
        let (key_id, secret) = scoped_key(&world, owner.id(), policy.clone()).await;
        if change == StaticReadPolicyChange::LegacyToScoped {
            sqlx::query("UPDATE api_tokens SET operation_scope=NULL, scopes=ARRAY['topic.view']::text[] WHERE id=$1")
                .bind(key_id).execute(&world.db.pool).await.unwrap();
        }
        let legacy = new_token(API_TOKEN_PREFIX);
        identity::create_api_token(
            &mut *world.conn().await,
            &identity::NewApiToken {
                id: new_id(),
                user_id: owner.id(),
                name: "Ordinary legacy read control",
                token_hash: &token_hash(&legacy),
                scopes: &["topic.view".to_owned()],
                node_id: None,
                at: world.now(),
                expires_at: Some(world.now() + chrono::Duration::hours(1)),
            },
        )
        .await
        .unwrap();
        let pause = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(tokio::sync::Notify::new());
        let continue_ = Arc::new(tokio::sync::Notify::new());
        let reads = Arc::new(AtomicUsize::new(0));
        let (paused, ready, continued, materialized) = (
            pause.clone(),
            entered.clone(),
            continue_.clone(),
            reads.clone(),
        );
        let mut registry = Registry::new();
        registry.on(NodeStaticReadProbe, move |ctx, (), ()| {
            let (paused, ready, continued, materialized) = (
                paused.clone(),
                ready.clone(),
                continued.clone(),
                materialized.clone(),
            );
            async move {
                if paused.load(Ordering::SeqCst) {
                    ready.notify_one();
                    continued.notified().await;
                }
                let mut conn = ctx.app.db.acquire().await?;
                // This is the same checkpoint used by ordinary static readers. Its
                // no-resolver fallback must reload current target facts on this connection.
                let principal = ctx.operation_principal(&mut conn).await?;
                let id = ctx.id("id")?;
                authorization::require(
                    &mut conn,
                    &principal,
                    id,
                    authorization::exact(Permission::TopicView),
                    orch_authz::Act::Use,
                    orch_authz::Policy::default(),
                )
                .await?;
                let name = orch_db::queries::threads::node_name(&mut conn, id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                materialized.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ApiError>(json!({"name":name}))
            }
        });
        let (browser, key) = registry.into_surface_routers();
        let browser = browser.with_state(world.app.app.clone());
        let key = key.with_state(world.app.app.clone());
        let router = |token: &str| match orch_api_contract::surface::HttpSurface::for_token(token)
            .expect("known fixture credential")
        {
            orch_api_contract::surface::HttpSurface::Browser => browser.clone(),
            orch_api_contract::surface::HttpSurface::Key => key.clone(),
        };
        let request = |token: &str| {
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/topics/{}/operation-static-read-probe",
                    topic.base.id
                ))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap()
        };
        for token in [&secret, &owner.token, &legacy] {
            assert_eq!(
                router(token)
                    .oneshot(request(token))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
        }
        let before_reads = reads.load(Ordering::SeqCst);
        pause.store(true, Ordering::SeqCst);
        let running = tokio::spawn(router(&secret).oneshot(request(&secret)));
        // Use the existing action-guard control's bounded watchdog, never a timed race.
        tokio::time::timeout(std::time::Duration::from_secs(30), entered.notified())
            .await
            .expect("the static reader reached the post-admission barrier");
        let mut moved = world.app.app.db.begin().await.unwrap();
        hierarchy::move_under(&mut moved, topic.base.id, after.base.id, world.now())
            .await
            .unwrap();
        if change == StaticReadPolicyChange::ScopedToLegacy {
            // Converting the stored policy cannot erase the request's accepted scope ceiling.
            sqlx::query(
                "UPDATE api_tokens SET operation_scope=NULL, scopes=ARRAY[]::text[] WHERE id=$1",
            )
            .bind(key_id)
            .execute(&mut *moved)
            .await
            .unwrap();
        } else if change == StaticReadPolicyChange::LegacyToScoped {
            // A current operation policy must be seen even though the initial legacy read
            // installed no operation proof. Its original topic.view ceiling still applies.
            sqlx::query(
                "UPDATE api_tokens SET operation_scope=$2, scopes=ARRAY[]::text[] WHERE id=$1",
            )
            .bind(key_id)
            .bind(&policy)
            .execute(&mut *moved)
            .await
            .unwrap();
        }
        access::bump_epoch(&mut moved).await.unwrap();
        moved.commit().await.unwrap();
        continue_.notify_one();
        let response = running.await.unwrap().unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "fresh static read must refuse the moved target, including conversion={change:?}"
        );
        assert_eq!(
            reads.load(Ordering::SeqCst),
            before_reads,
            "out-of-scope content is not materialized after the move"
        );
        pause.store(false, Ordering::SeqCst);
        // The owner and ordinary unbounded legacy reader still view the moved topic.
        // The repair must not turn a scoped-key refusal into a new business restriction.
        for token in [&owner.token, &legacy] {
            assert_eq!(
                router(token)
                    .oneshot(request(token))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
        }
    }
}
