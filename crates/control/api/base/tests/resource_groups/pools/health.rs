//! Shared failure facts are scoped, while each pool keeps its own suppression policy.
use super::*;
use orch_engine::resource_choice::health::{self, FailureOrigin, Scope, Suppression};
use orch_provider_contract::failure::{Classification, Evidence};

#[tokio::test]
async fn a_local_refusal_does_not_cool_another_persons_shared_credential() {
    let world = World::new().await;
    let owner = person(&world, "health-owner").await;
    let root = make_org(&world, &owner, "Health", "health").await.base.id;
    let pool = resource(&world, &owner, root, "pool", "Request policy").await;
    let scope = Scope::credential(orch_types::new_id(), json!({"generation":"one"}));
    let mut tx = world.db.begin().await.unwrap();
    let accepted = health::failure(
        &mut tx,
        &scope,
        orch_types::new_id(),
        FailureOrigin::Local,
        &Classification {
            code: "provider_unauthorized".into(),
            reason: None,
            evidence: Evidence::Status,
        },
        world.now(),
        None,
    )
    .await
    .unwrap();
    assert!(!accepted);
    assert_eq!(
        health::state(&mut tx, pool, &scope, world.now())
            .await
            .unwrap(),
        Suppression::Ready
    );
}
#[tokio::test]
async fn a_late_success_cannot_clear_a_newer_failure_and_a_pool_clear_is_local() {
    let world = World::new().await;
    let owner = person(&world, "epoch-owner").await;
    let root = make_org(&world, &owner, "Failure epochs", "failure-epochs")
        .await
        .base
        .id;
    let first = resource(&world, &owner, root, "pool", "First policy").await;
    let second = resource(&world, &owner, root, "pool", "Second policy").await;
    let scope = Scope::credential(orch_types::new_id(), json!({"generation":"one"}));
    let classification = Classification {
        code: "provider_unauthorized".into(),
        reason: None,
        evidence: Evidence::Status,
    };
    let mut tx = world.db.begin().await.unwrap();
    health::failure(
        &mut tx,
        &scope,
        orch_types::new_id(),
        FailureOrigin::Vendor,
        &classification,
        world.now(),
        None,
    )
    .await
    .unwrap();
    let epoch = health::epoch(&mut tx, &scope).await.unwrap();
    health::failure(
        &mut tx,
        &scope,
        orch_types::new_id(),
        FailureOrigin::Vendor,
        &classification,
        world.now(),
        None,
    )
    .await
    .unwrap();
    assert!(
        !health::success(&mut tx, &scope, epoch, orch_types::new_id(), world.now())
            .await
            .unwrap()
    );
    assert!(matches!(
        health::state(&mut tx, first, &scope, world.now())
            .await
            .unwrap(),
        Suppression::Cooling { .. }
    ));
    health::clear(&mut tx, first, &scope, world.now())
        .await
        .unwrap();
    assert_eq!(
        health::state(&mut tx, first, &scope, world.now())
            .await
            .unwrap(),
        Suppression::Ready
    );
    assert!(matches!(
        health::state(&mut tx, second, &scope, world.now())
            .await
            .unwrap(),
        Suppression::Cooling { .. }
    ));
}
#[tokio::test]
async fn replacing_a_credential_does_not_inherit_the_previous_generations_failure() {
    let world = World::new().await;
    let owner = person(&world, "rotation-health-owner").await;
    let root = make_org(&world, &owner, "Credential health", "credential-health")
        .await
        .base
        .id;
    let pool = resource(&world, &owner, root, "pool", "Model policy").await;
    let provider = orch_types::new_id();
    let old = Scope::credential(provider, json!({"generation":"old"}));
    let new = Scope::credential(provider, json!({"generation":"new"}));
    let mut tx = world.db.begin().await.unwrap();
    health::failure(
        &mut tx,
        &old,
        orch_types::new_id(),
        FailureOrigin::Vendor,
        &Classification {
            code: "provider_unauthorized".into(),
            reason: None,
            evidence: Evidence::Status,
        },
        world.now(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        health::state(&mut tx, pool, &new, world.now())
            .await
            .unwrap(),
        Suppression::Ready
    );
}

#[tokio::test]
async fn ten_simultaneous_recovery_attempts_share_one_fenced_trial() {
    let world = World::new().await;
    let owner = person(&world, "trial-owner").await;
    let root = make_org(&world, &owner, "Recovery trials", "recovery-trials")
        .await
        .base
        .id;
    let pool = resource(&world, &owner, root, "pool", "One trial").await;
    let scope = Scope::credential(orch_types::new_id(), json!({"generation":"one"}));
    let mut tx = world.db.begin().await.unwrap();
    health::failure(
        &mut tx,
        &scope,
        orch_types::new_id(),
        FailureOrigin::Vendor,
        &Classification {
            code: "provider_unauthorized".into(),
            reason: None,
            evidence: Evidence::Status,
        },
        world.now(),
        None,
    )
    .await
    .unwrap();
    let epoch = health::epoch(&mut tx, &scope).await.unwrap();
    tx.commit().await.unwrap();
    let at = world.now() + chrono::Duration::hours(2);
    let attempts = (0..10).map(|_| orch_types::new_id()).collect::<Vec<_>>();
    let claims = futures::future::join_all(attempts.iter().map(|attempt| async {
        let mut tx = world.db.begin().await.unwrap();
        let claimed = health::claim(&mut tx, pool, &scope, *attempt, epoch, at)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        claimed
    }))
    .await;
    assert_eq!(claims.iter().filter(|claimed| **claimed).count(), 1);
    let winner = attempts[claims.iter().position(|claimed| *claimed).unwrap()];
    let mut tx = world.db.begin().await.unwrap();
    assert!(
        health::claim(&mut tx, pool, &scope, winner, epoch, at)
            .await
            .unwrap()
    );
    assert!(matches!(
        health::state(&mut tx, pool, &scope, at).await.unwrap(),
        Suppression::TrialInProgress { .. }
    ));
    health::success(&mut tx, &scope, epoch, winner, at)
        .await
        .unwrap();
    assert_eq!(
        health::state(&mut tx, pool, &scope, at).await.unwrap(),
        Suppression::Ready
    );
}

#[tokio::test]
async fn try_again_now_requires_pool_management_and_keeps_another_pools_failure() {
    use orch_api_contract::pools::{PoolRecover, PoolRecoveryBody};
    let world = World::new().await;
    let owner = person(&world, "local-clear-owner").await;
    let visitor = person(&world, "local-clear-visitor").await;
    let root = make_org(&world, &owner, "Local recovery", "local-recovery")
        .await
        .base
        .id;
    let first = resource(&world, &owner, root, "pool", "First pool").await;
    let second = resource(&world, &owner, root, "pool", "Second pool").await;
    let scope = Scope::credential(orch_types::new_id(), json!({"generation":"one"}));
    let mut tx = world.db.begin().await.unwrap();
    health::failure(
        &mut tx,
        &scope,
        orch_types::new_id(),
        FailureOrigin::Vendor,
        &Classification {
            code: "provider_unauthorized".into(),
            reason: None,
            evidence: Evidence::Status,
        },
        world.now(),
        None,
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO pool_scope_observations(pool_id,scope_id) SELECT $1,id FROM pool_failure_scopes")
        .bind(first).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        world
            .app
            .call(PoolRecover, &[&first])
            .token(&visitor.token)
            .body(&PoolRecoveryBody {
                expected_revision: 1
            })
            .send_bytes()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    world
        .app
        .call(PoolRecover, &[&first])
        .token(&owner.token)
        .body(&PoolRecoveryBody {
            expected_revision: 1,
        })
        .send()
        .await
        .ok();
    let mut conn = world.conn().await;
    assert_eq!(
        health::state(&mut conn, first, &scope, world.now())
            .await
            .unwrap(),
        Suppression::Ready
    );
    assert!(matches!(
        health::state(&mut conn, second, &scope, world.now())
            .await
            .unwrap(),
        Suppression::Cooling { .. }
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pool_failures")
            .fetch_one(&mut *conn)
            .await
            .unwrap(),
        1
    );
}
