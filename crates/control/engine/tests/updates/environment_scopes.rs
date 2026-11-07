//! The real replacement runner consumes independently captured review and confirmation sources.
use super::*;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyScope, KeyTargetKind},
};
use orch_api_contract::{
    environment::{EnvironmentUpdateBatchCreate, EnvironmentUpdateBatchPreflight},
    environment_sources::update_batch::{self as declarations, Source},
    routes::Endpoint,
};
use orch_authz::TokenGrant;
use orch_db::queries::identity;
use orch_engine::{
    operation_scope::{
        request_origin::Credential,
        sources::{self, Facts},
        targets,
    },
    updates::operation::Input,
};
use orch_types::{Hash, new_id};

async fn origin(
    world: &World,
    environment: Id,
    confirmation: bool,
    reviewed: Option<Id>,
) -> (orch_engine::updates::Draft, Input, Id) {
    let operation = if confirmation {
        declarations::GLOBAL_CREATE.operation
    } else {
        declarations::GLOBAL_PREFLIGHT.operation
    };
    let scope:KeyScope=serde_json::from_value(json!({"rules":[
        {"target":{"kind":"environment","id":environment,"reach":"self"},"resources":["environment"],"allow":[{"operation":operation,"actions":["manage"]}]}
    ]})).unwrap();
    assert!(scope.validate().is_empty());
    let id = new_id();
    let hash = Hash::of(id.as_bytes());
    let end = Time::new(world.clock.now() + chrono::Duration::hours(1));
    let mut conn = world.pool.acquire().await.unwrap();
    identity::create_api_token_scoped(
        &mut conn,
        &identity::NewApiToken {
            id,
            user_id: world.ada,
            name: operation,
            token_hash: hash.as_bytes(),
            scopes: &[],
            node_id: None,
            at: world.clock.now(),
            expires_at: Some(end.get()),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    let target = targets::resource_identity(
        &mut conn,
        ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Environment,
            id: environment,
        },
    )
    .await
    .unwrap();
    let (route, declaration) = if confirmation {
        (
            EnvironmentUpdateBatchCreate::ROUTE,
            &declarations::GLOBAL_CREATE,
        )
    } else {
        (
            EnvironmentUpdateBatchPreflight::ROUTE,
            &declarations::GLOBAL_PREFLIGHT,
        )
    };
    let known = sources::bind_known(
        route,
        declaration,
        &[Facts::collection(
            Source::Environments,
            vec![(
                KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Environment),
                vec![target],
            )],
        )],
    )
    .unwrap();
    let caller = Caller::token(
        world.ada,
        TokenGrant::from_stored_scope(&[], None, Some(scope), Some(end))
            .unwrap()
            .for_token(id),
    );
    let caller = known.preparation_caller(&caller).unwrap();
    let credential = Credential::Token {
        id,
        hash: hash.as_bytes().to_vec(),
    };
    let mut source = Input::capture_global(
        &mut conn,
        &caller,
        credential,
        Some(end),
        environment,
        confirmation,
    )
    .await
    .unwrap();
    if let Some(review) = reviewed {
        source = source.reviewing(&mut conn, review).await.unwrap();
    }
    let principal = Principal::load(&mut conn, caller, world.clock.time())
        .await
        .unwrap();
    let (draft, _observation) = world
        .engine()
        .observe_update(
            &mut conn,
            &principal,
            Target::Environment(environment),
            UpdatePreflightReason::Update,
            None,
            &mut source,
        )
        .await;
    (draft.unwrap(), source, id)
}

#[tokio::test]
async fn a_native_environment_replacement_keeps_both_scoped_origins_through_its_real_commands() {
    for revoke in [false, true] {
        let world = World::new().await;
        serve(&world);
        let host = ScriptedHost::on(&world, "scoped-update", 16_384).await;
        let template = template(&world).await;
        let (environment, _, _unit) = running(&world, &host, template, "Scoped replacement").await;
        newer_image(&world, template).await;
        let (reviewed, review, review_key) = origin(&world, environment, false, None).await;
        let preflight = new_id();
        jobs::forks::save_preflight(
            &mut *world.pool.acquire().await.unwrap(),
            &jobs::forks::PreflightRow {
                id: preflight,
                kind: "update_batch".into(),
                node_id: None,
                environment_id: None,
                created_by: world.ada,
                plan: json!({"recipes":[reviewed.recipe],"providers":false}),
                expires_at: orch_engine::updates::expires(world.clock.now()),
                created_at: world.clock.now(),
            },
        )
        .await
        .unwrap();
        let (draft, confirmation, _) = origin(&world, environment, true, Some(preflight)).await;
        let mut tx = world.pool.begin().await.unwrap();
        let batch = new_id();
        let row =
            orch_engine::jobs::recreate::queue(&mut tx, draft, Some(batch), world.clock.now())
                .await
                .unwrap();
        orch_engine::jobs::recreate::environment_authority::retain(
            &mut tx,
            &row,
            batch,
            Some(review),
            confirmation,
            world.clock.time(),
        )
        .await
        .unwrap();
        jobs::forks::forget_preflight(&mut tx, preflight)
            .await
            .unwrap();
        if revoke {
            // The accepted queue exists; its original key ends before any worker can see that queue.
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                .bind(review_key)
                .bind(json!(KeyScope::default()))
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
        world.engine().kick();
        let result = wait_over(&world, row.id).await;
        if revoke {
            assert_eq!(result.0, "failed");
            assert_eq!(
                host.count(CommandKind::NodeRuntimeStop),
                0,
                "withdrawn review cannot stop the old unit"
            );
        } else {
            assert_eq!(result.0, "succeeded", "{result:?}");
            assert_eq!(host.count(CommandKind::NodeRuntimeStop), 1);
        }
    }
}
